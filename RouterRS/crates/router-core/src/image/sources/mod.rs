//! Image sources (`Router/src/Modules/Image/Sources`): FilePlayer, Gradient,
//! Text, Spout. The Gradient source lands with the renderer (Phase 5);
//! others follow.

use serde_json::Value as Json;

use super::PixelsF32;

/// Composite style (`Sources::Base::Style`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Style {
    #[default]
    Direct,
    HvThetaR,
    Centered,
}

impl Style {
    pub fn as_str(self) -> &'static str {
        match self {
            Style::Direct => "Direct",
            Style::HvThetaR => "HV_ThetaR",
            Style::Centered => "Centered",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "Direct" => Some(Style::Direct),
            "HV_ThetaR" => Some(Style::HvThetaR),
            "Centered" => Some(Style::Centered),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SourceBaseParams {
    pub visible: bool,
    pub render_enabled: bool,
    pub alpha: f32,
    pub style: Style,
}

impl Default for SourceBaseParams {
    fn default() -> Self {
        Self {
            visible: true,
            render_enabled: true,
            alpha: 1.0,
            style: Style::Direct,
        }
    }
}

pub struct RenderContext {
    pub width: usize,
    pub height: usize,
    pub time: f32,
}

pub trait ImageSource: Send {
    fn type_name(&self) -> &'static str;
    fn base(&self) -> &SourceBaseParams;
    fn base_mut(&mut self) -> &mut SourceBaseParams;
    /// Render this source's own layer at the requested resolution.
    fn render(&mut self, ctx: &RenderContext, out: &mut PixelsF32);
    fn deserialise(&mut self, json: &Json);
    fn serialise(&self) -> Json;
}

/// `Sources::Factory::createFromJson` — dispatch on `json["type"]`
/// (namespace-stripped type name).
pub fn create_from_json(json: &Json) -> Option<Box<dyn ImageSource>> {
    let type_name = json.get("type")?.as_str()?;
    // strip any namespace prefix, e.g. "Image::Sources::Gradient"
    let type_name = type_name.rsplit("::").next()?;
    let mut source = create_by_type_name(type_name)?;
    source.deserialise(json);
    Some(source)
}

pub fn create_by_type_name(type_name: &str) -> Option<Box<dyn ImageSource>> {
    let source: Box<dyn ImageSource> = match type_name {
        "Gradient" => Box::new(gradient::Gradient::default()),
        "FilePlayer" => Box::new(file_player::FilePlayer::default()),
        "Text" => Box::new(text::Text::default()),
        "Spout" => Box::new(spout::Spout::default()),
        _ => return None,
    };
    Some(source)
}

pub const SOURCE_TYPES: [&str; 4] = ["Gradient", "FilePlayer", "Text", "Spout"];

pub(crate) fn deserialise_base(base: &mut SourceBaseParams, json: &Json) {
    if let Some(v) = json.get("visible").and_then(|v| v.as_bool()) {
        base.visible = v;
    }
    if let Some(v) = json.get("renderEnabled").and_then(|v| v.as_bool()) {
        base.render_enabled = v;
    }
    if let Some(v) = json.get("alpha").and_then(|v| v.as_f64()) {
        base.alpha = v as f32;
    }
    if let Some(v) = json.get("style").and_then(|v| v.as_str()) {
        if let Some(style) = Style::from_str(v) {
            base.style = style;
        }
    }
}

pub mod file_player;
pub mod gradient;
pub mod spout;
pub mod text;
