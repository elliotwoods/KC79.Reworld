//! Serial device abstraction (port of `Router/src/SerialDevices`):
//! a COM port at 115200 baud or a TCP->RS485 gateway (default port 4196),
//! plus an in-process simulated bus for tests and `--simulate`.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use router_proto::constants::{BAUD_RATE, DEFAULT_TCP_PORT};
use serde_json::Value as Json;

pub trait SerialDevice: Send {
    fn type_name(&self) -> &'static str;
    fn address_string(&self) -> String;
    fn is_connected(&self) -> bool;
    fn transmit(&mut self, data: &[u8]) -> std::io::Result<()>;
    /// Return whatever bytes are available (non-blocking-ish; implementations
    /// use short internal timeouts). Empty vec = nothing pending.
    fn receive_available(&mut self) -> std::io::Result<Vec<u8>>;
    fn close(&mut self);
}

/// Create a device from a config descriptor:
/// `{"deviceType": "Serial"|"TCP", "address": ..., "port"?, "timeout_s"?}`.
pub fn create_device(config: &Json) -> std::io::Result<Box<dyn SerialDevice>> {
    let device_type = config
        .get("deviceType")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let address = config
        .get("address")
        .and_then(|v| v.as_str())
        .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "missing 'address'"))?;
    match device_type {
        "Serial" => Ok(Box::new(SerialPortDevice::open(address)?)),
        "TCP" => {
            let port = config.get("port").and_then(|v| v.as_u64()).unwrap_or(DEFAULT_TCP_PORT as u64) as u16;
            let timeout_s = config.get("timeout_s").and_then(|v| v.as_u64()).unwrap_or(1);
            Ok(Box::new(TcpDevice::open(address, port, Duration::from_secs(timeout_s))?))
        }
        other => Err(std::io::Error::new(
            ErrorKind::InvalidInput,
            format!("unknown deviceType '{other}'"),
        )),
    }
}

/// Enumerate available serial ports (for the GUI connect list).
pub fn list_serial_ports() -> Vec<String> {
    serialport::available_ports()
        .map(|ports| ports.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default()
}

// ------------------------------------------------------------------ Serial

pub struct SerialPortDevice {
    port: Option<Box<dyn serialport::SerialPort>>,
    address: String,
}

impl SerialPortDevice {
    pub fn open(address: &str) -> std::io::Result<Self> {
        let port = serialport::new(address, BAUD_RATE)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            // The worker already calls bytes_to_read before reading, so receive polling does
            // not depend on this being 1 ms. That value also governs writes on Windows: once
            // the driver queue fills during a firmware image, write_all can legitimately need
            // several frame times and the 1 ms deadline tears down an otherwise healthy port.
            .timeout(Duration::from_millis(100))
            .open()
            .map_err(|e| std::io::Error::new(ErrorKind::NotConnected, e.to_string()))?;
        Ok(Self {
            port: Some(port),
            address: address.to_owned(),
        })
    }
}

impl SerialDevice for SerialPortDevice {
    fn type_name(&self) -> &'static str {
        "Serial"
    }

    fn address_string(&self) -> String {
        self.address.clone()
    }

    fn is_connected(&self) -> bool {
        self.port.is_some()
    }

    fn transmit(&mut self, data: &[u8]) -> std::io::Result<()> {
        let Some(port) = self.port.as_mut() else {
            return Err(ErrorKind::NotConnected.into());
        };

        // A write that has to wait is not a write that failed.
        //
        // Sending a firmware image queues tens of kilobytes far faster than 115200 can carry
        // them, so the driver buffer fills and a write blocks until space appears. Past the
        // port's timeout that surfaces as `TimedOut`, and treating it as a disconnect tore the
        // link down mid-upload: measured on the bench 2026-08-26, a 100 kB image stopped after
        // 33 kB with the worker gone and the host reporting "sent 0 frames", because the whole
        // port had been dropped on a full buffer.
        //
        // Written as an offset loop rather than `write_all` so a partial write followed by a
        // timeout resumes where it stopped instead of re-sending bytes the far end already has.
        // Only silence gives up: a stall long enough to mean the port really is gone.
        const STALL_LIMIT: u32 = 100; // ~10 s at the port's 100 ms timeout
        let mut written = 0usize;
        let mut stalls = 0u32;
        while written < data.len() {
            match port.write(&data[written..]) {
                Ok(0) => stalls += 1,
                Ok(count) => {
                    written += count;
                    stalls = 0;
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::TimedOut | ErrorKind::Interrupted | ErrorKind::WouldBlock
                    ) =>
                {
                    stalls += 1;
                }
                Err(error) => {
                    self.port = None;
                    return Err(error);
                }
            }
            if stalls >= STALL_LIMIT {
                self.port = None;
                return Err(std::io::Error::new(
                    ErrorKind::TimedOut,
                    "the serial port accepted no bytes for ten seconds",
                ));
            }
        }
        Ok(())
    }

    fn receive_available(&mut self) -> std::io::Result<Vec<u8>> {
        let Some(port) = self.port.as_mut() else {
            return Err(ErrorKind::NotConnected.into());
        };
        let available = port.bytes_to_read().unwrap_or(0) as usize;
        if available == 0 {
            return Ok(Vec::new());
        }
        let mut buf = vec![0u8; available.min(4096)];
        match port.read(&mut buf) {
            Ok(n) => {
                buf.truncate(n);
                Ok(buf)
            }
            Err(e) if e.kind() == ErrorKind::TimedOut => Ok(Vec::new()),
            Err(e) => {
                self.port = None;
                Err(e)
            }
        }
    }

    fn close(&mut self) {
        self.port = None;
    }
}

// --------------------------------------------------------------------- TCP

pub struct TcpDevice {
    stream: Option<TcpStream>,
    address: String,
    port: u16,
}

impl TcpDevice {
    pub fn open(address: &str, port: u16, timeout: Duration) -> std::io::Result<Self> {
        let addr = (address, port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "unresolvable address"))?;
        let stream = TcpStream::connect_timeout(&addr, timeout)?;
        stream.set_nodelay(true).ok();
        stream.set_read_timeout(Some(Duration::from_millis(1)))?;
        stream.set_nonblocking(false)?;
        Ok(Self {
            stream: Some(stream),
            address: address.to_owned(),
            port,
        })
    }
}

impl SerialDevice for TcpDevice {
    fn type_name(&self) -> &'static str {
        "TCP"
    }

    fn address_string(&self) -> String {
        format!("{}:{}", self.address, self.port)
    }

    fn is_connected(&self) -> bool {
        self.stream.is_some()
    }

    fn transmit(&mut self, data: &[u8]) -> std::io::Result<()> {
        let Some(stream) = self.stream.as_mut() else {
            return Err(ErrorKind::NotConnected.into());
        };
        match stream.write_all(data) {
            Ok(()) => Ok(()),
            Err(e) => {
                self.stream = None;
                Err(e)
            }
        }
    }

    fn receive_available(&mut self) -> std::io::Result<Vec<u8>> {
        let Some(stream) = self.stream.as_mut() else {
            return Err(ErrorKind::NotConnected.into());
        };
        let mut buf = [0u8; 1024];
        match stream.read(&mut buf) {
            Ok(0) => {
                // orderly shutdown by the gateway
                self.stream = None;
                Err(ErrorKind::ConnectionReset.into())
            }
            Ok(n) => Ok(buf[..n].to_vec()),
            Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) => Ok(Vec::new()),
            Err(e) => {
                self.stream = None;
                Err(e)
            }
        }
    }

    fn close(&mut self) {
        self.stream = None;
    }
}

// -------------------------------------------------------------------- Mock

/// In-memory scripted device for unit tests: `transmit` collects frames,
/// `receive_available` yields whatever the test queued.
pub struct MockDevice {
    pub tx_log: Vec<Vec<u8>>,
    pub rx_queue: std::collections::VecDeque<Vec<u8>>,
    pub connected: bool,
}

impl Default for MockDevice {
    fn default() -> Self {
        Self {
            tx_log: Vec::new(),
            rx_queue: Default::default(),
            connected: true,
        }
    }
}

impl SerialDevice for MockDevice {
    fn type_name(&self) -> &'static str {
        "Mock"
    }

    fn address_string(&self) -> String {
        "mock".into()
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn transmit(&mut self, data: &[u8]) -> std::io::Result<()> {
        if !self.connected {
            return Err(ErrorKind::NotConnected.into());
        }
        self.tx_log.push(data.to_vec());
        Ok(())
    }

    fn receive_available(&mut self) -> std::io::Result<Vec<u8>> {
        if !self.connected {
            return Err(ErrorKind::NotConnected.into());
        }
        Ok(self.rx_queue.pop_front().unwrap_or_default())
    }

    fn close(&mut self) {
        self.connected = false;
    }
}
