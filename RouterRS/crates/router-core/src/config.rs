//! `config.json` loading/saving, byte-compatible with the C++ app's
//! deserialization semantics:
//!
//! - Top-level keys are module names: `"Renderer"`, `"Installation"`,
//!   `"Receiver"` (OSC), `"Server"` (REST).
//! - Parameters are looked up by exact ofParameter name (e.g. `"Count X"`,
//!   `"Period [s]"`) or by the lowercased first word of the name (e.g.
//!   `"count"`, `"period"`) — `Utils::deserialize` in `Utils.cpp`.
//! - `Installation.columnCommonSettings` is shallow-merged into every entry
//!   of `Installation.columns[]`, with the common settings *overwriting*
//!   per-column keys (nlohmann `json::update` semantics).
//! - The transmit enum accepts the C++ typo `"Inidividual"` (and the fixed
//!   spelling as an alias).
//!
//! Unlike the C++ app (which never saves), `save()` writes the config back,
//! preserving unknown keys, using exact parameter names (always accepted by
//! the C++ loader too).

use std::path::Path;

use serde_json::{json, Map, Value as Json};

// ------------------------------------------------------------ key lookup

/// `Utils::deserialize` lookup: exact name, then lowercased first word.
pub fn param_lookup<'a>(json: &'a Json, name: &str) -> Option<&'a Json> {
    if let Some(v) = json.get(name) {
        return Some(v);
    }
    let stripped = name.split(' ').next().unwrap_or(name).to_lowercase();
    json.get(&stripped)
}

fn get_or<T: Copy>(json: &Json, name: &str, default: T, cast: fn(&Json) -> Option<T>) -> T {
    param_lookup(json, name).and_then(cast).unwrap_or(default)
}

fn get_i64(json: &Json, name: &str, default: i64) -> i64 {
    get_or(json, name, default, |v| v.as_i64())
}

fn get_f32(json: &Json, name: &str, default: f32) -> f32 {
    get_or(json, name, default, |v| v.as_f64().map(|f| f as f32))
}

fn get_bool(json: &Json, name: &str, default: bool) -> bool {
    get_or(json, name, default, |v| v.as_bool())
}

// ------------------------------------------------------------------ types

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImageTransmit {
    #[default]
    Individual,
    Keyframe,
    Disabled,
}

impl ImageTransmit {
    /// Wire string — keeps the C++ enum's typo for config compatibility.
    pub fn as_config_str(self) -> &'static str {
        match self {
            ImageTransmit::Individual => "Inidividual", // sic, matches MAKE_ENUM
            ImageTransmit::Keyframe => "Keyframe",
            ImageTransmit::Disabled => "Disabled",
        }
    }

    pub fn from_config_str(s: &str) -> Option<Self> {
        match s {
            "Inidividual" | "Individual" => Some(ImageTransmit::Individual),
            "Keyframe" => Some(ImageTransmit::Keyframe),
            "Disabled" => Some(ImageTransmit::Disabled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArrangementConfig {
    pub columns: usize,
    pub rows: usize,
    pub column_width: usize,
    /// `"Panel height"`: rows per panel when a column is a vertical stack of panels
    /// rather than one flat grid. Reworld V3 sets 3; V1/V2 leave it 0.
    pub panel_height: usize,
    pub flipped: bool,
}

impl Default for ArrangementConfig {
    fn default() -> Self {
        Self {
            columns: 32,
            rows: 24,
            column_width: 1,
            panel_height: 0,
            flipped: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessagingConfig {
    pub transmit: ImageTransmit,
    pub period_s: f32,
    pub keyframe_batch_size: usize,
    pub keyframe_velocities: bool,
}

impl Default for MessagingConfig {
    fn default() -> Self {
        Self {
            transmit: ImageTransmit::Individual,
            period_s: 0.5,
            keyframe_batch_size: 8,
            keyframe_velocities: true,
        }
    }
}

/// One column entry, after `columnCommonSettings` merging.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnConfig {
    pub count_x: usize,
    pub count_y: usize,
    /// `"Panel height"`: rows per panel on a Reworld V3 column, which is a vertical stack
    /// of 3x3 panels rather than one flat grid. Absent (0) on V1/V2, which have no panels.
    pub panel_height: usize,
    pub flipped: Option<bool>,
    /// Serial device descriptor consumed by the device factory:
    /// `{"deviceType": "Serial"|"TCP", "address": ..., "port"?, "timeout_s"?}`
    pub rs485: Option<Json>,
    pub fwupdate: Option<Json>,
    /// The full merged column JSON (for unknown-key preservation).
    pub raw: Json,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct InstallationConfig {
    pub arrangement: ArrangementConfig,
    pub messaging: MessagingConfig,
    pub image_enabled: bool,
    /// Per-column overrides (already merged with `columnCommonSettings`).
    /// May be shorter than `arrangement.columns`; extra columns use defaults
    /// derived from the arrangement, exactly like the C++.
    pub columns: Vec<ColumnConfig>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ServerConfig {
    pub enabled: bool,
    pub port: u16,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct AppConfig {
    pub installation: InstallationConfig,
    /// OSC receiver ("Receiver"); default enabled on port 4000.
    pub osc: ServerConfig,
    /// REST server ("Server"); default enabled on port 8080.
    pub rest: ServerConfig,
    /// Renderer sources (typed parsing happens in the image module).
    pub renderer_sources: Vec<Json>,
    /// The original document, for unknown-key-preserving saves.
    pub raw: Json,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 0,
        }
    }
}

pub const OSC_DEFAULT_PORT: u16 = 4000;
pub const REST_DEFAULT_PORT: u16 = 8080;

// ---------------------------------------------------------------- parsing

impl AppConfig {
    pub fn from_json(doc: Json) -> Self {
        let installation = doc
            .get("Installation")
            .map(parse_installation)
            .unwrap_or_default();
        let osc = parse_server(doc.get("Receiver"), OSC_DEFAULT_PORT);
        let rest = parse_server(doc.get("Server"), REST_DEFAULT_PORT);
        let renderer_sources = doc
            .get("Renderer")
            .and_then(|r| r.get("sources"))
            .and_then(|s| s.as_array())
            .cloned()
            .unwrap_or_default();
        Self {
            installation,
            osc,
            rest,
            renderer_sources,
            raw: doc,
        }
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        let doc: Json = serde_json::from_str(&text)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(Self::from_json(doc))
    }

    /// Serialize, preserving unknown keys from the loaded document and using
    /// exact parameter names (which the C++ loader also accepts).
    pub fn to_json(&self) -> Json {
        let mut doc = if self.raw.is_object() {
            self.raw.clone()
        } else {
            json!({})
        };
        let root = doc.as_object_mut().unwrap();

        // Installation
        {
            let inst_entry = root.entry("Installation").or_insert_with(|| json!({}));
            if !inst_entry.is_object() {
                *inst_entry = json!({});
            }
            let inst = inst_entry.as_object_mut().unwrap();
            let a = &self.installation.arrangement;
            merge_object(
                inst,
                "arrangement",
                json!({
                    "Columns": a.columns,
                    "Rows": a.rows,
                    "Column width": a.column_width,
                    "Panel height": a.panel_height,
                    "Flipped": a.flipped,
                }),
            );
            let m = &self.installation.messaging;
            merge_object(
                inst,
                "messaging",
                json!({
                    "transmit": m.transmit.as_config_str(),
                    "Period [s]": m.period_s,
                    "Keyframe batch size": m.keyframe_batch_size,
                    "Keyframe velocities": m.keyframe_velocities,
                }),
            );
            merge_object(
                inst,
                "image",
                json!({ "Enabled": self.installation.image_enabled }),
            );
            let columns: Vec<Json> = self
                .installation
                .columns
                .iter()
                .map(|c| {
                    let mut col = if c.raw.is_object() {
                        c.raw.clone()
                    } else {
                        json!({})
                    };
                    let obj = col.as_object_mut().unwrap();
                    obj.insert("Count X".into(), json!(c.count_x));
                    obj.insert("Count Y".into(), json!(c.count_y));
                    if c.panel_height != 0 {
                        obj.insert("Panel height".into(), json!(c.panel_height));
                    }
                    if let Some(f) = c.flipped {
                        obj.insert("Flipped".into(), json!(f));
                    }
                    if let Some(rs485) = &c.rs485 {
                        obj.insert("rs485".into(), rs485.clone());
                    }
                    if let Some(fw) = &c.fwupdate {
                        obj.insert("fwupdate".into(), fw.clone());
                    }
                    col
                })
                .collect();
            inst.insert("columns".into(), Json::Array(columns));
            // The per-column entries above are already merged; remove the
            // common block so a reload doesn't overwrite them differently.
            inst.remove("columnCommonSettings");
        }

        for (key, cfg) in [("Receiver", &self.osc), ("Server", &self.rest)] {
            let entry = root.entry(key).or_insert_with(|| json!({}));
            if !entry.is_object() {
                *entry = json!({});
            }
            let obj = entry.as_object_mut().unwrap();
            obj.insert("Enabled".into(), json!(cfg.enabled));
            obj.insert("Port".into(), json!(cfg.port));
        }

        if !self.renderer_sources.is_empty() || root.contains_key("Renderer") {
            let entry = root.entry("Renderer").or_insert_with(|| json!({}));
            if !entry.is_object() {
                *entry = json!({});
            }
            entry
                .as_object_mut()
                .unwrap()
                .insert("sources".into(), Json::Array(self.renderer_sources.clone()));
        }

        doc
    }

    /// Atomic save: write to a temp file then rename over the target.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(&self.to_json())?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &text)?;
        // On Windows, rename fails if the target exists
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        std::fs::rename(&tmp, path)
    }
}

fn merge_object(parent: &mut Map<String, Json>, key: &str, values: Json) {
    let entry = parent.entry(key).or_insert_with(|| json!({}));
    if !entry.is_object() {
        *entry = json!({});
    }
    let obj = entry.as_object_mut().unwrap();
    for (k, v) in values.as_object().unwrap() {
        obj.insert(k.clone(), v.clone());
    }
}

fn parse_installation(json: &Json) -> InstallationConfig {
    let mut config = InstallationConfig::default();

    if let Some(arrangement) = json.get("arrangement") {
        let d = ArrangementConfig::default();
        config.arrangement = ArrangementConfig {
            columns: get_i64(arrangement, "Columns", d.columns as i64) as usize,
            rows: get_i64(arrangement, "Rows", d.rows as i64) as usize,
            column_width: get_i64(arrangement, "Column width", d.column_width as i64) as usize,
            panel_height: get_i64(arrangement, "Panel height", d.panel_height as i64) as usize,
            flipped: get_bool(arrangement, "Flipped", d.flipped),
        };
    }

    if let Some(messaging) = json.get("messaging") {
        let d = MessagingConfig::default();
        config.messaging = MessagingConfig {
            transmit: messaging
                .get("transmit")
                .and_then(|t| t.as_str())
                .and_then(ImageTransmit::from_config_str)
                .unwrap_or(d.transmit),
            period_s: get_f32(messaging, "Period [s]", d.period_s),
            keyframe_batch_size: get_i64(
                messaging,
                "Keyframe batch size",
                d.keyframe_batch_size as i64,
            ) as usize,
            keyframe_velocities: get_bool(messaging, "Keyframe velocities", d.keyframe_velocities),
        };
    }

    if let Some(image) = json.get("image") {
        config.image_enabled = get_bool(image, "Enabled", false);
    }

    if let Some(columns) = json.get("columns").and_then(|c| c.as_array()) {
        let common = json.get("columnCommonSettings");
        config.columns = columns
            .iter()
            .map(|col| parse_column(col, common))
            .collect();
    }

    config
}

fn parse_column(json: &Json, common: Option<&Json>) -> ColumnConfig {
    // nlohmann `jsonColumn.update(jsonColumnSettings)`: shallow merge where
    // the COMMON settings overwrite per-column keys.
    let mut merged = json.clone();
    if let (Some(obj), Some(Json::Object(common))) = (merged.as_object_mut(), common) {
        for (k, v) in common {
            obj.insert(k.clone(), v.clone());
        }
    }

    ColumnConfig {
        count_x: get_i64(&merged, "Count X", 1) as usize,
        count_y: get_i64(&merged, "Count Y", 1) as usize,
        panel_height: get_i64(&merged, "Panel height", 0) as usize,
        flipped: param_lookup(&merged, "Flipped").and_then(|v| v.as_bool()),
        rs485: merged.get("rs485").cloned(),
        fwupdate: merged.get("fwupdate").cloned(),
        raw: merged,
    }
}

fn parse_server(json: Option<&Json>, default_port: u16) -> ServerConfig {
    let mut config = ServerConfig {
        enabled: true,
        port: default_port,
    };
    if let Some(json) = json {
        config.enabled = get_bool(json, "Enabled", true);
        config.port = get_i64(json, "Port", default_port as i64) as u16;
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Json {
        serde_json::from_str(include_str!("../../../tests-fixtures/config.sample.json")).unwrap()
    }

    #[test]
    fn parses_sample_config() {
        let config = AppConfig::from_json(sample());
        assert_eq!(config.installation.arrangement.columns, 4);
        assert_eq!(config.installation.arrangement.rows, 6);
        assert_eq!(config.installation.arrangement.column_width, 3);
        assert_eq!(
            config.installation.messaging.transmit,
            ImageTransmit::Keyframe
        );
        assert_eq!(config.installation.messaging.keyframe_batch_size, 8);
        assert_eq!(config.osc.port, 4000);
        assert_eq!(config.rest.port, 8080);
        assert!(config.rest.enabled);
        assert_eq!(config.renderer_sources.len(), 1);
        assert_eq!(config.installation.columns.len(), 4);
    }

    #[test]
    fn cpp_typo_enum_is_accepted() {
        assert_eq!(
            ImageTransmit::from_config_str("Inidividual"),
            Some(ImageTransmit::Individual)
        );
        assert_eq!(
            ImageTransmit::from_config_str("Individual"),
            Some(ImageTransmit::Individual)
        );
        // save writes the C++-compatible string
        assert_eq!(ImageTransmit::Individual.as_config_str(), "Inidividual");
    }

    #[test]
    fn column_common_settings_overwrite_per_column() {
        // nlohmann update(): common wins over per-column keys
        let doc = json!({
            "Installation": {
                "columns": [
                    { "Count X": 3, "rs485": {"deviceType": "Serial", "address": "COM7"} }
                ],
                "columnCommonSettings": {
                    "Count Y": 6,
                    "rs485": {"deviceType": "TCP", "address": "192.168.1.201"}
                }
            }
        });
        let config = AppConfig::from_json(doc);
        let col = &config.installation.columns[0];
        assert_eq!(col.count_x, 3, "per-column key not in common survives");
        assert_eq!(col.count_y, 6, "common key applies");
        // whole rs485 subtree replaced by common (shallow update)
        assert_eq!(col.rs485.as_ref().unwrap()["deviceType"], "TCP");
    }

    #[test]
    fn stripped_lowercase_keys_are_accepted() {
        let doc = json!({
            "Installation": {
                "arrangement": { "columns": 8, "rows": 12, "flipped": true }
            },
            "Server": { "enabled": false, "port": 9999 }
        });
        let config = AppConfig::from_json(doc);
        assert_eq!(config.installation.arrangement.columns, 8);
        assert_eq!(config.installation.arrangement.rows, 12);
        assert!(config.installation.arrangement.flipped);
        assert!(!config.rest.enabled);
        assert_eq!(config.rest.port, 9999);
    }

    #[test]
    fn save_round_trips_and_preserves_unknown_keys() {
        let mut doc = sample();
        doc.as_object_mut()
            .unwrap()
            .insert("customTopLevel".into(), json!({"keep": "me"}));
        let config = AppConfig::from_json(doc);
        let saved = config.to_json();
        assert_eq!(saved["customTopLevel"]["keep"], "me");

        // parse(save(parse(x))) == parse(x)
        let reparsed = AppConfig::from_json(saved);
        assert_eq!(reparsed.installation, config.installation);
        assert_eq!(reparsed.osc, config.osc);
        assert_eq!(reparsed.rest, config.rest);
        assert_eq!(reparsed.renderer_sources, config.renderer_sources);
    }

    #[test]
    fn defaults_when_empty() {
        let config = AppConfig::from_json(json!({}));
        assert_eq!(config.installation.arrangement.columns, 32);
        assert_eq!(config.installation.arrangement.rows, 24);
        assert_eq!(config.osc.port, 4000);
        assert_eq!(config.rest.port, 8080);
        assert_eq!(
            config.installation.messaging.transmit,
            ImageTransmit::Individual
        );
    }
}
