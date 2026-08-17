//! Text source (port of `Image/Sources/Text.*`): rasterizes a string,
//! auto-fitted to the (tiny) installation resolution, optionally inverted.
//! Uses ab_glyph with a system font (the C++ used ofxAssets fonts).

use ab_glyph::{Font, FontVec, Glyph, ScaleFont};
use serde_json::{json, Value as Json};

use super::{deserialise_base, ImageSource, RenderContext, SourceBaseParams};
use crate::image::PixelsF32;

pub struct Text {
    pub base: SourceBaseParams,
    pub text: String,
    pub font_name: String,
    pub size: i32,
    pub border: i32,
    pub inverse: bool,
    font: Option<FontVec>,
    loaded_font_name: String,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            base: SourceBaseParams::default(),
            text: "TEST".into(),
            font_name: String::new(),
            size: 11,
            border: 0,
            inverse: false,
            font: None,
            loaded_font_name: "\u{0}unloaded".into(),
        }
    }
}

impl Text {
    fn ensure_font(&mut self) {
        if self.loaded_font_name == self.font_name && self.font.is_some() {
            return;
        }
        self.loaded_font_name = self.font_name.clone();
        self.font = load_font(&self.font_name);
    }
}

fn load_font(name: &str) -> Option<FontVec> {
    let fonts_dir = std::path::PathBuf::from(
        std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into()),
    )
    .join("Fonts");
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if !name.is_empty() {
        let base = fonts_dir.join(name);
        candidates.push(base.with_extension("ttf"));
        candidates.push(std::path::PathBuf::from(name));
    }
    candidates.push(fonts_dir.join("consola.ttf"));
    candidates.push(fonts_dir.join("segoeui.ttf"));
    candidates.push(fonts_dir.join("arial.ttf"));

    for path in candidates {
        if let Ok(bytes) = std::fs::read(&path) {
            if let Ok(font) = FontVec::try_from_vec(bytes) {
                return Some(font);
            }
        }
    }
    None
}

impl ImageSource for Text {
    fn type_name(&self) -> &'static str {
        "Text"
    }

    fn base(&self) -> &SourceBaseParams {
        &self.base
    }

    fn base_mut(&mut self) -> &mut SourceBaseParams {
        &mut self.base
    }

    fn render(&mut self, ctx: &RenderContext, out: &mut PixelsF32) {
        self.ensure_font();
        if out.width != ctx.width || out.height != ctx.height {
            *out = PixelsF32::new(ctx.width, ctx.height);
        }
        let background = if self.inverse { 1.0 } else { 0.0 };
        out.data.fill(background);

        let Some(font) = &self.font else { return };
        if self.text.is_empty() || ctx.width == 0 || ctx.height == 0 {
            return;
        }

        // Auto-fit: measure at a reference scale, then scale to the bounds
        // (minus border), like the C++ drawTextIntoBounds behavior.
        let border = self.border.max(0) as f32;
        let avail_w = (ctx.width as f32 - border * 2.0).max(1.0);
        let avail_h = (ctx.height as f32 - border * 2.0).max(1.0);

        let reference = 32.0f32;
        let (text_w, text_h) = measure(font, &self.text, reference);
        if text_w <= 0.0 || text_h <= 0.0 {
            return;
        }
        let scale = reference * (avail_w / text_w).min(avail_h / text_h);
        let (final_w, final_h) = measure(font, &self.text, scale);

        let origin_x = border + (avail_w - final_w) / 2.0;
        let origin_y = border + (avail_h - final_h) / 2.0;

        let scaled = font.as_scaled(scale);
        let ink = if self.inverse { 0.0 } else { 1.0 };
        let mut caret_x = origin_x;
        let baseline = origin_y + scaled.ascent();
        let mut previous: Option<ab_glyph::GlyphId> = None;
        for ch in self.text.chars() {
            let id = scaled.glyph_id(ch);
            if let Some(prev) = previous {
                caret_x += scaled.kern(prev, id);
            }
            previous = Some(id);
            let glyph: Glyph = id.with_scale_and_position(scale, ab_glyph::point(caret_x, baseline));
            caret_x += scaled.h_advance(id);
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                outlined.draw(|gx, gy, coverage| {
                    let px = bounds.min.x as i32 + gx as i32;
                    let py = bounds.min.y as i32 + gy as i32;
                    if px < 0 || py < 0 {
                        return;
                    }
                    let (px, py) = (px as usize, py as usize);
                    if px >= ctx.width || py >= ctx.height {
                        return;
                    }
                    let i = (px + py * ctx.width) * 3;
                    let value = background + (ink - background) * coverage;
                    // R and G channels only, like the other sources (B unused)
                    out.data[i] = out.data[i].max(value.min(1.0));
                    out.data[i + 1] = out.data[i + 1].max(value.min(1.0));
                });
            }
        }
    }

    fn deserialise(&mut self, json: &Json) {
        deserialise_base(&mut self.base, json);
        if let Some(v) = json.get("text").and_then(|v| v.as_str()) {
            self.text = v.to_string();
        }
        if let Some(v) = json.get("font").and_then(|v| v.as_str()) {
            self.font_name = v.to_string();
        }
        if let Some(v) = json.get("size").and_then(|v| v.as_i64()) {
            self.size = v as i32;
        }
        if let Some(v) = json.get("border").and_then(|v| v.as_i64()) {
            self.border = v as i32;
        }
        if let Some(v) = json.get("inverse").and_then(|v| v.as_bool()) {
            self.inverse = v;
        }
    }

    fn serialise(&self) -> Json {
        json!({
            "type": "Text",
            "visible": self.base.visible,
            "renderEnabled": self.base.render_enabled,
            "alpha": self.base.alpha,
            "style": self.base.style.as_str(),
            "text": self.text,
            "font": self.font_name,
            "size": self.size,
            "border": self.border,
            "inverse": self.inverse,
        })
    }
}

fn measure(font: &FontVec, text: &str, scale: f32) -> (f32, f32) {
    let scaled = font.as_scaled(scale);
    let mut width = 0.0f32;
    let mut previous: Option<ab_glyph::GlyphId> = None;
    for ch in text.chars() {
        let id = scaled.glyph_id(ch);
        if let Some(prev) = previous {
            width += scaled.kern(prev, id);
        }
        previous = Some(id);
        width += scaled.h_advance(id);
    }
    (width, scaled.ascent() - scaled.descent())
}
