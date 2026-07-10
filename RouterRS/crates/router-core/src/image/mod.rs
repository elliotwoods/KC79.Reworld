//! Image pipeline: sources composited into a float-RGB buffer at the
//! installation's resolution; each pixel's (R, G) is a portal's (x, y) aim
//! target. Sources and composite styles are ported in `renderer.rs` /
//! `sources/` (Phase 5); this module defines the shared pixel buffer.

pub mod renderer;
pub mod sources;

pub use renderer::{RenderSettings, Renderer};
pub use sources::{ImageSource, SourceBaseParams, Style};

/// 3-channel float pixel buffer (`ofFloatPixels` equivalent).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PixelsF32 {
    pub width: usize,
    pub height: usize,
    /// RGB interleaved, row-major, length = width * height * 3.
    pub data: Vec<f32>,
}

impl PixelsF32 {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0.0; width * height * 3],
        }
    }

    pub fn clear(&mut self) {
        self.data.fill(0.0);
    }

    pub fn get(&self, x: usize, y: usize) -> Option<[f32; 3]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (x + y * self.width) * 3;
        Some([self.data[i], self.data[i + 1], self.data[i + 2]])
    }

    pub fn set(&mut self, x: usize, y: usize, rgb: [f32; 3]) {
        if x >= self.width || y >= self.height {
            return;
        }
        let i = (x + y * self.width) * 3;
        self.data[i..i + 3].copy_from_slice(&rgb);
    }
}
