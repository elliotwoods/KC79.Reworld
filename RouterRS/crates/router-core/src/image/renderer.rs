//! Composites image sources into one float-RGB buffer
//! (`Router/src/Modules/Image/Renderer.cpp::render`, ported verbatim
//! including its quirks — see BUG-COMPAT notes).

use serde_json::Value as Json;

use super::sources::{create_from_json, ImageSource, RenderContext, Style};
use super::PixelsF32;

#[derive(Debug, Clone, Copy)]
pub struct RenderSettings {
    pub width: usize,
    pub height: usize,
    pub time: f32,
}

#[derive(Default)]
pub struct Renderer {
    pub sources: Vec<Box<dyn ImageSource>>,
    /// Per-source rendered layers (parallel to `sources`).
    layers: Vec<PixelsF32>,
    pub pixels: PixelsF32,
}

impl Renderer {
    pub fn from_config(source_configs: &[Json]) -> Self {
        let mut renderer = Self::default();
        for config in source_configs {
            if let Some(source) = create_from_json(config) {
                renderer.sources.push(source);
            }
        }
        renderer
            .layers
            .resize_with(renderer.sources.len(), PixelsF32::default);
        renderer
    }

    pub fn add_source(&mut self, source: Box<dyn ImageSource>) {
        self.sources.push(source);
        self.layers.push(PixelsF32::default());
    }

    pub fn remove_source(&mut self, index: usize) {
        if index < self.sources.len() {
            self.sources.remove(index);
            self.layers.remove(index);
        }
    }

    pub fn render(&mut self, settings: &RenderSettings) {
        let ctx = RenderContext {
            width: settings.width,
            height: settings.height,
            time: settings.time,
        };
        self.layers
            .resize_with(self.sources.len(), PixelsF32::default);

        // Render individual source layers
        for (source, layer) in self.sources.iter_mut().zip(&mut self.layers) {
            if source.base().render_enabled {
                source.render(&ctx, layer);
            }
        }

        // Allocate + clear the result
        if self.pixels.width != settings.width || self.pixels.height != settings.height {
            self.pixels = PixelsF32::new(settings.width, settings.height);
        }
        self.pixels.clear();

        // Sum visible sources
        let (width, height) = (settings.width, settings.height);
        for (source, layer) in self.sources.iter().zip(&self.layers) {
            let base = source.base();
            if !base.visible || layer.data.len() != self.pixels.data.len() {
                continue;
            }
            let alpha = base.alpha;
            match base.style {
                Style::Direct => {
                    for (dst, src) in self.pixels.data.iter_mut().zip(&layer.data) {
                        *dst += src * alpha;
                    }
                }
                Style::HvThetaR => {
                    // BUG-COMPAT: alpha is not applied in this style (as C++)
                    for i in 0..width * height {
                        let src = &layer.data[i * 3..i * 3 + 3];
                        let (hue, _saturation, brightness) = get_hsb(src[0], src[1], src[2]);
                        // C++ uses the 0..1 hue value directly as radians
                        let (r, theta) = (brightness, hue);
                        self.pixels.data[i * 3] += theta.cos() * r;
                        self.pixels.data[i * 3 + 1] += theta.sin() * r;
                    }
                }
                Style::Centered => {
                    // BUG-COMPAT: alpha is not applied, and the input is read
                    // at `input[i]` (first row only), as in Renderer.cpp:132
                    let half_width = (width / 2) as f32;
                    let half_height = (height / 2) as f32;
                    for j in 0..height {
                        for i in 0..width {
                            let x = i as f32 - half_width;
                            let y = j as f32 - half_height;
                            let theta = y.atan2(x);
                            let src = &layer.data[i * 3..i * 3 + 3];
                            let (_h, _s, brightness) = get_hsb(src[0], src[1], src[2]);
                            let mut r = (x * x + y * y).sqrt() / half_width.max(half_height);
                            r *= brightness;
                            let out = (i + j * width) * 3;
                            self.pixels.data[out] += theta.cos() * r;
                            self.pixels.data[out + 1] += theta.sin() * r;
                        }
                    }
                }
            }
        }
    }

    pub fn serialise_sources(&self) -> Vec<Json> {
        self.sources.iter().map(|s| s.serialise()).collect()
    }
}

/// `ofFloatColor::getHsb`: hue/saturation/brightness all in 0..1.
fn get_hsb(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    if max == min {
        // grey: hue undefined -> 0
        return (0.0, 0.0, max);
    }
    let hue_sixth = if r == max {
        let mut h = (g - b) / (max - min);
        if h < 0.0 {
            h += 6.0;
        }
        h
    } else if g == max {
        2.0 + (b - r) / (max - min)
    } else {
        4.0 + (r - g) / (max - min)
    };
    let hue = hue_sixth / 6.0;
    let saturation = (max - min) / max;
    (hue, saturation, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::sources::gradient::Gradient;

    #[test]
    fn direct_composite_applies_alpha() {
        let mut renderer = Renderer::default();
        let mut gradient = Gradient::default();
        gradient.base.alpha = 0.5;
        gradient.speed = 0.0;
        renderer.add_source(Box::new(gradient));

        let settings = RenderSettings {
            width: 4,
            height: 4,
            time: 0.0,
        };
        renderer.render(&settings);
        assert_eq!(renderer.pixels.width, 4);

        let mut renderer_full = Renderer::default();
        let mut gradient = Gradient::default();
        gradient.speed = 0.0;
        renderer_full.add_source(Box::new(gradient));
        renderer_full.render(&settings);

        for (half, full) in renderer.pixels.data.iter().zip(&renderer_full.pixels.data) {
            assert!((half - full * 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn invisible_sources_are_skipped() {
        let mut renderer = Renderer::default();
        let mut gradient = Gradient::default();
        gradient.base.visible = false;
        renderer.add_source(Box::new(gradient));
        renderer.render(&RenderSettings {
            width: 4,
            height: 4,
            time: 0.0,
        });
        assert!(renderer.pixels.data.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn hsb_of_grey_is_brightness() {
        assert_eq!(get_hsb(0.5, 0.5, 0.5), (0.0, 0.0, 0.5));
        let (h, s, b) = get_hsb(1.0, 0.0, 0.0);
        assert_eq!((h, s, b), (0.0, 1.0, 1.0));
        let (h, _, _) = get_hsb(0.0, 1.0, 0.0);
        assert!((h - 1.0 / 3.0).abs() < 1e-6);
    }
}
