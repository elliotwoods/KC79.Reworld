//! Procedural gradient source, port of `Image/Sources/Gradient.*`.

use serde_json::{json, Value as Json};

use crate::model::kinematics::of_map;

use super::{deserialise_base, ImageSource, RenderContext, SourceBaseParams};
use crate::image::PixelsF32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GradientType {
    #[default]
    Radial,
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Wave {
    Triangle,
    #[default]
    Sine,
    Sawtooth,
}

impl GradientType {
    pub fn as_str(self) -> &'static str {
        match self {
            GradientType::Radial => "Radial",
            GradientType::Horizontal => "Horizontal",
            GradientType::Vertical => "Vertical",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Radial" => Some(Self::Radial),
            "Horizontal" => Some(Self::Horizontal),
            "Vertical" => Some(Self::Vertical),
            _ => None,
        }
    }
}

impl Wave {
    pub fn as_str(self) -> &'static str {
        match self {
            Wave::Triangle => "Triangle",
            Wave::Sine => "Sine",
            Wave::Sawtooth => "Sawtooth",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Triangle" => Some(Self::Triangle),
            "Sine" => Some(Self::Sine),
            "Sawtooth" => Some(Self::Sawtooth),
            _ => None,
        }
    }
}

pub struct Gradient {
    pub base: SourceBaseParams,
    pub gradient_type: GradientType,
    pub wave: Wave,
    pub frequency: f32,
    pub speed: f32,
    pub value1: [f32; 2],
    pub value2: [f32; 2],
}

impl Default for Gradient {
    fn default() -> Self {
        Self {
            base: SourceBaseParams::default(),
            gradient_type: GradientType::Radial,
            wave: Wave::Sine,
            frequency: 1.0,
            speed: 0.05,
            value1: [0.0, 0.0],
            value2: [1.0, 1.0],
        }
    }
}

impl ImageSource for Gradient {
    fn type_name(&self) -> &'static str {
        "Gradient"
    }

    fn base(&self) -> &SourceBaseParams {
        &self.base
    }

    fn base_mut(&mut self) -> &mut SourceBaseParams {
        &mut self.base
    }

    fn render(&mut self, ctx: &RenderContext, out: &mut PixelsF32) {
        let (width, height) = (ctx.width, ctx.height);
        let mut data = Vec::with_capacity(width * height * 3);
        let half_min = (width.min(height) as f32) / 2.0;
        for j in 0..height {
            for i in 0..width {
                // centered, normalized coordinates
                let x = (i as f32 - width as f32 / 2.0) / half_min;
                let y = (j as f32 - height as f32 / 2.0) / half_min;
                let r = (x * x + y * y).sqrt();

                let mut thi = match self.gradient_type {
                    GradientType::Radial => r,
                    GradientType::Horizontal => x,
                    GradientType::Vertical => y,
                };

                // time offset
                thi -= self.speed * ctx.time;
                thi = thi.abs();

                let alpha = match self.wave {
                    Wave::Triangle => {
                        // BUG-COMPAT: the fold uses `thi`, not the modulo
                        // value (`Gradient.cpp:82-85`)
                        let a = (thi * self.frequency) % 2.0;
                        if a > 1.0 {
                            2.0 - thi
                        } else {
                            a
                        }
                    }
                    Wave::Sine => (thi * self.frequency * std::f32::consts::PI).sin() * 0.5 + 0.5,
                    Wave::Sawtooth => (thi * self.frequency) % 1.0,
                };

                data.push(of_map(alpha, 0.0, 1.0, self.value1[0], self.value2[0]));
                data.push(of_map(alpha, 0.0, 1.0, self.value1[1], self.value2[1]));
                data.push(0.0);
            }
        }
        out.width = width;
        out.height = height;
        out.data = data;
    }

    fn deserialise(&mut self, json: &Json) {
        deserialise_base(&mut self.base, json);
        if let Some(v) = json.get("gradientType").and_then(|v| v.as_str()) {
            if let Some(g) = GradientType::from_str(v) {
                self.gradient_type = g;
            }
        }
        if let Some(v) = json.get("wave").and_then(|v| v.as_str()) {
            if let Some(w) = Wave::from_str(v) {
                self.wave = w;
            }
        }
        if let Some(v) = json.get("frequency").and_then(|v| v.as_f64()) {
            self.frequency = v as f32;
        }
        if let Some(v) = json.get("speed").and_then(|v| v.as_f64()) {
            self.speed = v as f32;
        }
        for (key, target) in [("value1", &mut self.value1), ("value2", &mut self.value2)] {
            if let Some(v) = json.get(key).and_then(|v| v.as_array()) {
                for (i, component) in v.iter().take(2).enumerate() {
                    if let Some(f) = component.as_f64() {
                        target[i] = f as f32;
                    }
                }
            }
        }
    }

    fn serialise(&self) -> Json {
        json!({
            "type": "Gradient",
            "visible": self.base.visible,
            "renderEnabled": self.base.render_enabled,
            "alpha": self.base.alpha,
            "style": self.base.style.as_str(),
            "gradientType": self.gradient_type.as_str(),
            "wave": self.wave.as_str(),
            "frequency": self.frequency,
            "speed": self.speed,
            "value1": self.value1,
            "value2": self.value2,
        })
    }
}
