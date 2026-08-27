//! Video file source (port of `Image/Sources/FilePlayer.*`, which used
//! `ofVideoPlayer`).
//!
//! Implementation: an ffmpeg process (via ffmpeg-sidecar) decodes the whole
//! file scaled to the installation resolution into memory. Frames at that
//! resolution are tiny (a 32x24 frame is 2.3 KB), so full in-memory caching
//! is practical and gives exact Loop / PingPong / None, speed, and position
//! control. Decoding re-runs when the file or resolution changes.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::time::Instant;

use serde_json::{json, Value as Json};

use super::{deserialise_base, ImageSource, RenderContext, SourceBaseParams, Style};
use crate::image::PixelsF32;

const DECODE_FPS: f32 = 30.0;
/// Frame-cache cap: ~30 minutes at 30 fps. Beyond this the tail is dropped.
const MAX_FRAMES: usize = 54_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LoopMode {
    #[default]
    Loop,
    PingPong,
    None,
}

impl LoopMode {
    pub fn as_str(self) -> &'static str {
        match self {
            LoopMode::Loop => "Loop",
            LoopMode::PingPong => "Ping Pong",
            LoopMode::None => "None",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Loop" => Some(Self::Loop),
            "Ping Pong" | "PingPong" => Some(Self::PingPong),
            "None" => Some(Self::None),
            _ => None,
        }
    }
}

struct DecodedVideo {
    /// RGB8 frames at the requested resolution.
    frames: Vec<Vec<u8>>,
}

enum DecodeState {
    Idle,
    Running(Receiver<Result<DecodedVideo, String>>),
    Done(DecodedVideo),
    Failed,
}

pub struct FilePlayer {
    pub base: SourceBaseParams,
    pub file: PathBuf,
    pub play: bool,
    pub loop_mode: LoopMode,
    pub speed: f32,
    /// Normalized playhead 0..1.
    pub position: f32,
    /// +1 forward, -1 backward (ping-pong state).
    direction: f32,
    decode: DecodeState,
    decoded_for: (PathBuf, usize, usize),
    last_advance: Option<Instant>,
    pub last_error: Option<String>,
}

impl Default for FilePlayer {
    fn default() -> Self {
        Self {
            base: SourceBaseParams {
                // C++ FilePlayer defaults to the HV_ThetaR composite style
                style: Style::HvThetaR,
                ..Default::default()
            },
            file: PathBuf::new(),
            play: true,
            loop_mode: LoopMode::Loop,
            speed: 1.0,
            position: 0.0,
            direction: 1.0,
            decode: DecodeState::Idle,
            decoded_for: (PathBuf::new(), 0, 0),
            last_advance: None,
            last_error: None,
        }
    }
}

impl FilePlayer {
    fn start_decode(&mut self, width: usize, height: usize) {
        self.decoded_for = (self.file.clone(), width, height);
        if self.file.as_os_str().is_empty() || width == 0 || height == 0 {
            self.decode = DecodeState::Idle;
            return;
        }
        let (tx, rx) = channel();
        let path = self.file.clone();
        std::thread::Builder::new()
            .name("fileplayer-decode".into())
            .spawn(move || {
                let _ = tx.send(decode_video(&path, width, height));
            })
            .ok();
        self.decode = DecodeState::Running(rx);
    }

    fn poll_decode(&mut self) {
        if let DecodeState::Running(rx) = &self.decode {
            match rx.try_recv() {
                Ok(Ok(video)) => self.decode = DecodeState::Done(video),
                Ok(Err(error)) => {
                    self.last_error = Some(error);
                    self.decode = DecodeState::Failed;
                }
                Err(_) => {}
            }
        }
    }

    fn advance(&mut self, frame_count: usize) {
        let now = Instant::now();
        let dt = self
            .last_advance
            .map(|t| now.duration_since(t).as_secs_f32())
            .unwrap_or(0.0);
        self.last_advance = Some(now);
        if !self.play || frame_count == 0 {
            return;
        }
        let duration_s = frame_count as f32 / DECODE_FPS;
        let step = dt * self.speed * self.direction / duration_s.max(1e-6);
        let mut position = self.position + step;
        match self.loop_mode {
            LoopMode::Loop => {
                position = position.rem_euclid(1.0);
            }
            LoopMode::PingPong => {
                if position > 1.0 {
                    position = 1.0 - (position - 1.0);
                    self.direction = -1.0;
                } else if position < 0.0 {
                    position = -position;
                    self.direction = 1.0;
                }
                position = position.clamp(0.0, 1.0);
            }
            LoopMode::None => {
                position = position.clamp(0.0, 1.0);
            }
        }
        self.position = position;
    }
}

impl ImageSource for FilePlayer {
    fn type_name(&self) -> &'static str {
        "FilePlayer"
    }

    fn base(&self) -> &SourceBaseParams {
        &self.base
    }

    fn base_mut(&mut self) -> &mut SourceBaseParams {
        &mut self.base
    }

    fn render(&mut self, ctx: &RenderContext, out: &mut PixelsF32) {
        // (re)start decode when file/resolution changed
        if self.decoded_for != (self.file.clone(), ctx.width, ctx.height) {
            self.start_decode(ctx.width, ctx.height);
        }
        self.poll_decode();

        if out.width != ctx.width || out.height != ctx.height {
            *out = PixelsF32::new(ctx.width, ctx.height);
        }

        let DecodeState::Done(video) = &self.decode else {
            out.clear();
            self.last_advance = Some(Instant::now());
            return;
        };
        let frame_count = video.frames.len();
        self.advance(frame_count);
        let DecodeState::Done(video) = &self.decode else {
            return;
        };
        if video.frames.is_empty() {
            out.clear();
            return;
        }
        let index = ((self.position * (video.frames.len() - 1) as f32).round() as usize)
            .min(video.frames.len() - 1);
        let frame = &video.frames[index];
        for (dst, src) in out.data.iter_mut().zip(frame.iter()) {
            *dst = *src as f32 / 255.0;
        }
    }

    fn deserialise(&mut self, json: &Json) {
        deserialise_base(&mut self.base, json);
        if let Some(v) = json.get("file").and_then(|v| v.as_str()) {
            self.file = PathBuf::from(v);
        }
        if let Some(v) = json.get("play").and_then(|v| v.as_bool()) {
            self.play = v;
        }
        if let Some(v) = json.get("loopMode").and_then(|v| v.as_str()) {
            if let Some(mode) = LoopMode::from_str(v) {
                self.loop_mode = mode;
            }
        }
        if let Some(v) = json.get("speed").and_then(|v| v.as_f64()) {
            self.speed = v as f32;
        }
        if let Some(v) = json.get("position").and_then(|v| v.as_f64()) {
            self.position = (v as f32).clamp(0.0, 1.0);
        }
    }

    fn serialise(&self) -> Json {
        json!({
            "type": "FilePlayer",
            "visible": self.base.visible,
            "renderEnabled": self.base.render_enabled,
            "alpha": self.base.alpha,
            "style": self.base.style.as_str(),
            "file": self.file.display().to_string(),
            "play": self.play,
            "loopMode": self.loop_mode.as_str(),
            "speed": self.speed,
            "position": self.position,
            "error": self.last_error,
        })
    }
}

fn decode_video(
    path: &std::path::Path,
    width: usize,
    height: usize,
) -> Result<DecodedVideo, String> {
    use ffmpeg_sidecar::command::FfmpegCommand;
    use ffmpeg_sidecar::event::FfmpegEvent;

    let mut child = FfmpegCommand::new()
        .input(path.to_string_lossy())
        .args([
            "-vf",
            &format!("scale={width}:{height}"),
            "-r",
            &format!("{DECODE_FPS}"),
        ])
        .rawvideo()
        .spawn()
        .map_err(|e| format!("ffmpeg spawn failed (is ffmpeg.exe on PATH?): {e}"))?;

    let mut frames = Vec::new();
    let iter = child.iter().map_err(|e| e.to_string())?;
    for event in iter {
        match event {
            FfmpegEvent::OutputFrame(frame) => {
                if frames.len() < MAX_FRAMES {
                    frames.push(frame.data);
                }
            }
            FfmpegEvent::Error(error) => {
                if frames.is_empty() {
                    return Err(error);
                }
            }
            _ => {}
        }
    }
    if frames.is_empty() {
        return Err("no frames decoded".into());
    }
    Ok(DecodedVideo { frames })
}
