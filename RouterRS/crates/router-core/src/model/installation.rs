//! The Installation: owns all Columns, the arrangement, image->hardware
//! transmission, and broadcast actions.
//! Port of `Router/src/Modules/Hardware/Installation.*`.

use std::time::Instant;

use router_proto::commands::ActionKind;
use router_proto::Value;
use router_report::Reporter;

use crate::config::{ImageTransmit, InstallationConfig};
use crate::image::PixelsF32;

use super::column::{Column, ColumnSettings};

pub struct Installation {
    // arrangement parameters
    pub columns_count: usize,
    pub rows: usize,
    pub column_width: usize,
    /// Rows per panel, when a column is a stack of panels rather than a flat grid.
    pub panel_height: usize,
    pub flipped: bool,
    // messaging parameters
    pub transmit: ImageTransmit,
    pub period_s: f32,
    pub keyframe_batch_size: usize,
    pub keyframe_velocities: bool,
    // image parameter
    pub image_enabled: bool,

    pub columns: Vec<Column>,
    reporter: Reporter,
    last_transmit_keyframe: Instant,
}

impl Installation {
    pub fn new(reporter: Reporter) -> Self {
        let mut installation = Self {
            columns_count: 32,
            rows: 24,
            column_width: 1,
            panel_height: 0,
            flipped: false,
            transmit: ImageTransmit::Individual,
            period_s: 0.5,
            keyframe_batch_size: 8,
            keyframe_velocities: true,
            image_enabled: false,
            columns: Vec::new(),
            reporter,
            last_transmit_keyframe: Instant::now(),
        };
        installation.rebuild_columns();
        installation
    }

    pub fn from_config(config: &InstallationConfig, reporter: Reporter) -> Self {
        let mut installation = Self::new(reporter);
        installation.columns_count = config.arrangement.columns;
        installation.rows = config.arrangement.rows;
        installation.column_width = config.arrangement.column_width;
        installation.panel_height = config.arrangement.panel_height;
        installation.flipped = config.arrangement.flipped;
        installation.transmit = config.messaging.transmit;
        installation.period_s = config.messaging.period_s;
        installation.keyframe_batch_size = config.messaging.keyframe_batch_size;
        installation.keyframe_velocities = config.messaging.keyframe_velocities;
        installation.image_enabled = config.image_enabled;
        installation.rebuild_columns();

        // per-column settings (like C++: min(columns, config entries))
        let count = installation.columns.len().min(config.columns.len());
        for i in 0..count {
            installation.columns[i].apply_config(&config.columns[i]);
        }
        installation
    }

    pub fn rebuild_columns(&mut self) {
        self.columns = (0..self.columns_count)
            .map(|index| {
                Column::new(
                    ColumnSettings {
                        index,
                        count_x: self.column_width,
                        count_y: self.rows,
                        panel_height: self.panel_height,
                        flipped: self.flipped,
                    },
                    self.reporter.clone(),
                )
            })
            .collect();
    }

    /// Image resolution derived from the arrangement:
    /// (columns * countX, rows).
    pub fn resolution(&self) -> (usize, usize) {
        match self.columns.first() {
            Some(first) => (self.columns.len() * first.count_x, first.count_y),
            None => (0, 0),
        }
    }

    /// Per-tick: columns update, then frame transmission per the messaging
    /// mode. `pixels` is the rendered image when image sampling is enabled.
    pub fn update(&mut self, pixels: Option<&PixelsF32>) {
        if self.image_enabled {
            if let Some(pixels) = pixels {
                self.take_image(pixels);
            }
        }
        self.transmit_frame();

        let reporter = self.reporter.clone();
        for column in &mut self.columns {
            column.update(&reporter);
        }
    }

    /// `Installation::takeImage`: verify resolution and push pixel targets
    /// into every column.
    pub fn take_image(&mut self, pixels: &PixelsF32) {
        let (w, h) = self.resolution();
        if pixels.width != w || pixels.height != h {
            return;
        }
        for column in &mut self.columns {
            column.update_positions_from_image(pixels);
        }
    }

    /// `Installation::transmitFrame`.
    pub fn transmit_frame(&mut self) {
        match self.transmit {
            ImageTransmit::Keyframe => {
                if self.last_transmit_keyframe.elapsed().as_secs_f32() >= self.period_s {
                    self.last_transmit_keyframe = Instant::now();
                    let batch = self.keyframe_batch_size;
                    let velocities = self.keyframe_velocities;
                    for column in &mut self.columns {
                        column.transmit_keyframe(batch, velocities);
                    }
                }
            }
            ImageTransmit::Individual => {
                for column in &mut self.columns {
                    column.push_stale();
                }
            }
            ImageTransmit::Disabled => {}
        }
    }

    pub fn poll_all(&mut self) {
        for column in &mut self.columns {
            column.poll_all();
        }
    }

    pub fn broadcast(&self, body: &Value, collateable: bool) {
        for column in &self.columns {
            column.broadcast(body, collateable);
        }
    }

    pub fn broadcast_action(&mut self, action: ActionKind) {
        for column in &mut self.columns {
            column.broadcast_action(action);
        }
    }

    /// `homeHardwareAndZeroPositions`: broadcast the Home action 10 times,
    /// then reset every pilot.
    pub fn home_hardware_and_zero_positions(&mut self) {
        for _ in 0..10 {
            self.broadcast(&ActionKind::Home.body(), false);
        }
        for column in &mut self.columns {
            for portal in &mut column.portals {
                portal.pilot.reset();
            }
        }
    }

    pub fn column(&mut self, index: usize) -> Option<&mut Column> {
        self.columns.get_mut(index)
    }

    pub fn portal(&mut self, column_index: usize, target: u8) -> Option<&mut super::portal::Portal> {
        self.columns
            .get_mut(column_index)
            .and_then(|c| c.portal_by_target(target))
    }
}
