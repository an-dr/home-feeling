//! Turns what a bones guest requests into what it may do (ADR-003).
//!
//! The composition root declares capabilities; a guest's manifest requests
//! some of them by id; the user's answer, stored per guest, decides which
//! patterns the guest host denies. A guard the guarded code writes guards
//! nothing (bones ADR-042), so the manifest only asks.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use bones_messages::ui::kinds::{Button, Label, Strong};
use bones_messages::ui::{Event, Layout, Node, WidgetProps};
use serde::{Deserialize, Serialize};

/// One power the composition root guards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Capability {
    pub id: String,
    /// What the user reads, such as "Move and resize windows".
    pub label: String,
    /// The topics and endpoints it covers, in deny-list syntax.
    pub patterns: Vec<String>,
}

impl Capability {
    pub fn new(id: &str, label: &str, patterns: &[&str]) -> Self {
        Self {
            id: id.to_string(),
            label: label.to_string(),
            patterns: patterns.iter().map(|p| p.to_string()).collect(),
        }
    }
}

/// A guest's manifest: what it says about itself and what it asks for.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Manifest {
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub requests: Vec<String>,
}

impl Manifest {
    /// Reads `manifest.toml` in `dir`; a guest without one requests nothing.
    pub fn read(dir: &Path) -> Result<Self, String> {
        let path = dir.join("manifest.toml");
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).map_err(|err| format!("manifest.toml: {err}")),
            Err(_) if !path.exists() => Ok(Self::default()),
            Err(err) => Err(format!("manifest.toml: {err}")),
        }
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut manifest: Self = toml::from_str(text).map_err(|err| err.to_string())?;
        manifest.requests.sort();
        manifest.requests.dedup();
        Ok(manifest)
    }
}

/// Where a guest stands with the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// The user answered this exact request; these ids were granted.
    Answered { granted: Vec<String> },
    /// The request is new or changed and waits for the user.
    Pending,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Answer {
    requested: Vec<String>,
    granted: Vec<String>,
}

/// The capabilities and every stored answer, kept in one JSON file.
pub struct Consent {
    capabilities: Vec<Capability>,
    path: PathBuf,
    answers: BTreeMap<String, Answer>,
}

impl Consent {
    /// Opens the answers at `path`; a missing or unreadable file starts empty
    /// and an unreadable one is set aside, never overwritten.
    pub fn open(path: impl Into<PathBuf>, capabilities: Vec<Capability>) -> Self {
        let path = path.into();
        let answers = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|_| {
                let _ = std::fs::rename(&path, path.with_extension("unreadable.json"));
                BTreeMap::new()
            }),
            Err(_) => BTreeMap::new(),
        };
        Self {
            capabilities,
            path,
            answers,
        }
    }

    pub fn capabilities(&self) -> &[Capability] {
        &self.capabilities
    }

    /// Requested ids no capability declares; they can never be granted.
    pub fn unknown(&self, manifest: &Manifest) -> Vec<String> {
        manifest
            .requests
            .iter()
            .filter(|id| !self.capabilities.iter().any(|c| &c.id == *id))
            .cloned()
            .collect()
    }

    pub fn status(&self, guest: &str, manifest: &Manifest) -> Status {
        match self.answers.get(guest) {
            Some(answer) if answer.requested == manifest.requests => Status::Answered {
                granted: answer.granted.clone(),
            },
            _ if manifest.requests.is_empty() => Status::Answered {
                granted: Vec::new(),
            },
            _ => Status::Pending,
        }
    }

    /// Records the user's answer for this request; only requested ids can be
    /// granted.
    pub fn answer(
        &mut self,
        guest: &str,
        manifest: &Manifest,
        granted: &[String],
    ) -> Result<(), String> {
        let granted = granted
            .iter()
            .filter(|id| manifest.requests.contains(id))
            .cloned()
            .collect();
        self.answers.insert(
            guest.to_string(),
            Answer {
                requested: manifest.requests.clone(),
                granted,
            },
        );
        self.save()
    }

    /// The patterns of every capability the guest was not granted.
    pub fn deny_list(&self, guest: &str, manifest: &Manifest) -> Vec<String> {
        let granted = match self.status(guest, manifest) {
            Status::Answered { granted } => granted,
            Status::Pending => Vec::new(),
        };
        self.capabilities
            .iter()
            .filter(|capability| !granted.contains(&capability.id))
            .flat_map(|capability| capability.patterns.clone())
            .collect()
    }

    fn save(&self) -> Result<(), String> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        let text = serde_json::to_vec_pretty(&self.answers).map_err(|err| err.to_string())?;
        let temporary = self.path.with_extension("json.new");
        std::fs::write(&temporary, text).map_err(|err| err.to_string())?;
        std::fs::rename(&temporary, &self.path).map_err(|err| err.to_string())
    }

    /// The approval prompt for `guest` as a widget subtree. Its two buttons
    /// use `id_base + 1` (allow) and `id_base + 2` (refuse).
    pub fn prompt(&self, guest: &str, manifest: &Manifest, id_base: u32) -> Node {
        let mut children = vec![Strong {
            text: format!("{guest} asks to:"),
        }
        .node(0)];
        for id in &manifest.requests {
            let label = self
                .capabilities
                .iter()
                .find(|c| &c.id == id)
                .map(|c| c.label.clone())
                .unwrap_or_else(|| format!("{id} (unknown, cannot be granted)"));
            children.push(
                Label {
                    text: format!("• {label}"),
                }
                .node(0),
            );
        }
        children.push(Node::container(
            Layout::Horizontal,
            vec![
                Button::new("Allow").node(id_base + 1),
                Button::new("Refuse").node(id_base + 2),
            ],
        ));
        Node::container(Layout::Frame, children)
    }
}

/// The user's click on a [`Consent::prompt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptAnswer {
    Allow,
    Refuse,
}

impl PromptAnswer {
    pub fn from_event(id_base: u32, event: &Event) -> Option<Self> {
        match event {
            Event::Clicked { id } if *id == id_base + 1 => Some(Self::Allow),
            Event::Clicked { id } if *id == id_base + 2 => Some(Self::Refuse),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
