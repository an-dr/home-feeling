//! The settings a guest declares in its manifest, and the values they take.
//!
//! A guest lists `[[settings]]` with a dotted `key`, a `type`, and a
//! `default`. The app builds its settings page from the list, keeps what the
//! user chose, and gives the guest the merged values as its `config` table.
use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::{json, Map, Number, Value};

/// How a setting is edited and checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// On or off.
    Bool,
    /// A whole number between `min` and `max`.
    Int,
    /// A number between `min` and `max`.
    Number,
    /// Free text.
    String,
    /// `#rrggbb`.
    Color,
    /// One of `options`.
    Choice,
    /// A hotkey such as `alt+shift+left`, or empty for none.
    Keys,
}

/// One declared setting.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Setting {
    /// Where the value lands in `config`; a dot nests: `border.width`.
    pub key: String,
    #[serde(rename = "type")]
    pub kind: Kind,
    pub default: Value,
    /// What the user reads; the key when missing.
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub description: String,
    /// Headings group settings in the order they first appear.
    #[serde(default)]
    pub section: String,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub options: Vec<String>,
}

impl Setting {
    pub fn title(&self) -> &str {
        if self.label.is_empty() {
            &self.key
        } else {
            &self.label
        }
    }

    /// The range a slider covers: the declared one, else a span around the default.
    pub fn range(&self) -> (f64, f64) {
        let default = self.default.as_f64().unwrap_or(0.0);
        let min = self.min.unwrap_or(0.0_f64.min(default));
        let max = self.max.unwrap_or(if self.kind == Kind::Int {
            (default * 2.0).max(10.0)
        } else {
            1.0_f64.max(default * 2.0)
        });
        (min, max.max(min))
    }

    /// Whether the declaration itself is usable: a default of the right type.
    pub fn check(&self) -> Result<(), String> {
        self.accept(&self.default)
            .map(|_| ())
            .map_err(|err| format!("setting {}: default: {err}", self.key))
    }

    /// `value` as this setting holds it, or why it is refused.
    pub fn accept(&self, value: &Value) -> Result<Value, String> {
        match self.kind {
            Kind::Bool => value
                .as_bool()
                .map(Value::Bool)
                .ok_or_else(|| "expected true or false".to_string()),
            Kind::Int | Kind::Number => {
                let number = value.as_f64().ok_or("expected a number")?;
                let (min, max) = (self.min.unwrap_or(f64::MIN), self.max.unwrap_or(f64::MAX));
                if number < min || number > max {
                    return Err(format!("expected a number from {min} to {max}"));
                }
                if self.kind == Kind::Int {
                    Ok(json!(number.round() as i64))
                } else {
                    Number::from_f64(number)
                        .map(Value::Number)
                        .ok_or_else(|| "expected a number".to_string())
                }
            }
            Kind::String => value
                .as_str()
                .map(|s| Value::String(s.to_string()))
                .ok_or_else(|| "expected text".to_string()),
            Kind::Color => {
                let text = value.as_str().ok_or("expected a colour like #00bcd4")?;
                let hex = text.strip_prefix('#').unwrap_or("");
                if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    Ok(Value::String(text.to_ascii_lowercase()))
                } else {
                    Err("expected a colour like #00bcd4".into())
                }
            }
            Kind::Choice => match value.as_str() {
                Some(text) if self.options.iter().any(|o| o == text) => Ok(value.clone()),
                _ => Err(format!("expected one of {}", self.options.join(", "))),
            },
            Kind::Keys => value
                .as_str()
                .map(|s| Value::String(s.trim().to_string()))
                .ok_or_else(|| "expected a key combination like alt+left".to_string()),
        }
    }
}

/// The value of every setting: the stored one when it is still valid, else
/// the default. Keys are flat here, dots included.
pub fn values(settings: &[Setting], stored: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    settings
        .iter()
        .map(|setting| {
            let value = stored
                .get(&setting.key)
                .and_then(|value| setting.accept(value).ok())
                .or_else(|| setting.accept(&setting.default).ok())
                .unwrap_or_else(|| setting.default.clone());
            (setting.key.clone(), value)
        })
        .collect()
}

/// The values as the nested JSON object the guest's `config` takes.
pub fn nest(values: &BTreeMap<String, Value>) -> Value {
    let mut root = Map::new();
    for (key, value) in values {
        let mut parts: Vec<&str> = key.split('.').collect();
        let last = parts.pop().unwrap_or_default();
        let mut table = &mut root;
        for part in parts {
            let entry = table
                .entry(part.to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            if !entry.is_object() {
                *entry = Value::Object(Map::new());
            }
            table = entry.as_object_mut().expect("just made an object");
        }
        table.insert(last.to_string(), value.clone());
    }
    Value::Object(root)
}
