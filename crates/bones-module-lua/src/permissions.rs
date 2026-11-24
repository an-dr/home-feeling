/// The topics and endpoints one guest may not use (bones ADR-042).
///
/// The same rules as `bones_module_wasm::Permissions`, repeated here so this
/// crate does not link wasmtime:
/// - entries are exact names (`files`) or prefixes ending in `*` (`os/*`);
/// - the default denies nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Permissions {
    denied: Vec<String>,
}

impl Permissions {
    /// Denies every topic and endpoint `patterns` names.
    pub fn deny<I, S>(patterns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            denied: patterns.into_iter().map(Into::into).collect(),
        }
    }

    /// The denied patterns, in the order given.
    pub fn denied(&self) -> &[String] {
        &self.denied
    }

    /// Whether the guest may publish to `topic` or send to the endpoint it names.
    pub fn allows(&self, topic: &str) -> bool {
        !self.denied.iter().any(|denied| overlaps(denied, topic))
    }

    /// Whether the guest may subscribe to `pattern`: refused when the pattern
    /// could match any denied topic, so `*` is refused once anything is denied.
    pub fn allows_subscription(&self, pattern: &str) -> bool {
        self.allows(pattern)
    }
}

/// Whether some topic matches both patterns.
fn overlaps(a: &str, b: &str) -> bool {
    match (a.strip_suffix('*'), b.strip_suffix('*')) {
        (Some(a), Some(b)) => a.starts_with(b) || b.starts_with(a),
        (Some(prefix), None) => b.starts_with(prefix),
        (None, Some(prefix)) => a.starts_with(prefix),
        (None, None) => a == b,
    }
}
