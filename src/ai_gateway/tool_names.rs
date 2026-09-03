use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const TOOL_SEARCH_NAME: &str = "tool_search";
pub const VIEW_IMAGE_TOOL_NAME: &str = "view_image";
pub const GROK_READ_FILE_TOOL_NAME: &str = "read_file";
const NAMESPACE_MARKER: &str = "__codexns__";
const LEGACY_NAMESPACE_MARKER: &str = "responses_unit__";

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ToolCallKind {
    Function,
    ToolSearch,
    Custom,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ToolCallTarget {
    pub kind: ToolCallKind,
    pub namespace: Option<String>,
    pub name: String,
}

impl ToolCallTarget {
    pub fn function(namespace: Option<&str>, name: &str) -> Self {
        Self {
            kind: ToolCallKind::Function,
            namespace: namespace
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string),
            name: name.to_string(),
        }
    }

    pub fn tool_search() -> Self {
        Self {
            kind: ToolCallKind::ToolSearch,
            namespace: None,
            name: TOOL_SEARCH_NAME.to_string(),
        }
    }

    pub fn custom(name: &str) -> Self {
        Self {
            kind: ToolCallKind::Custom,
            namespace: None,
            name: name.to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolNameMap {
    encoded_to_target: HashMap<String, ToolCallTarget>,
    target_to_encoded: HashMap<ToolCallTarget, String>,
}

impl ToolNameMap {
    pub fn is_empty(&self) -> bool {
        self.encoded_to_target.is_empty()
    }

    pub fn insert(&mut self, encoded: impl Into<String>, target: ToolCallTarget) {
        let encoded = encoded.into();
        self.encoded_to_target
            .insert(encoded.clone(), target.clone());
        self.target_to_encoded.insert(target, encoded);
    }

    pub fn encode_function(&mut self, namespace: Option<&str>, name: &str) -> String {
        self.encode(ToolCallTarget::function(namespace, name))
    }

    pub fn encode_function_as(
        &mut self,
        namespace: Option<&str>,
        name: &str,
        preferred_name: &str,
    ) -> String {
        self.encode_as(ToolCallTarget::function(namespace, name), preferred_name)
    }

    pub fn encode_custom(&mut self, name: &str) -> String {
        self.encode(ToolCallTarget::custom(name))
    }

    pub fn encode_tool_search(&mut self) -> String {
        self.encode(ToolCallTarget::tool_search())
    }

    pub fn encode(&mut self, target: ToolCallTarget) -> String {
        let preferred = match target.kind {
            ToolCallKind::Function => {
                encode_provider_tool_name(target.namespace.as_deref(), &target.name)
            }
            ToolCallKind::ToolSearch => TOOL_SEARCH_NAME.to_string(),
            ToolCallKind::Custom => target.name.clone(),
        };
        self.encode_as(target, &preferred)
    }

    fn encode_as(&mut self, target: ToolCallTarget, preferred: &str) -> String {
        if let Some(encoded) = self.target_to_encoded.get(&target) {
            return encoded.clone();
        }

        let encoded = self.allocate_encoded_name(preferred, &target);
        self.insert(encoded.clone(), target);
        encoded
    }

    pub fn decode(&self, encoded: &str) -> ToolCallTarget {
        self.encoded_to_target
            .get(encoded)
            .cloned()
            .unwrap_or_else(|| decode_provider_tool_name(encoded))
    }

    pub fn has_encoded(&self, encoded: &str) -> bool {
        self.encoded_to_target.contains_key(encoded)
    }

    fn allocate_encoded_name(&self, preferred: &str, target: &ToolCallTarget) -> String {
        let base = provider_safe_tool_name(preferred);
        if base.len() <= PROVIDER_TOOL_NAME_MAX_LEN {
            match self.encoded_to_target.get(&base) {
                None => return base,
                Some(existing) if existing == target => return base,
                _ => {}
            }
        }

        let hash = target_hash(target);
        let mut counter = 0usize;
        loop {
            let suffix = if counter == 0 {
                format!("_h{hash}")
            } else {
                format!("_h{hash}{counter}")
            };
            let prefix_len = PROVIDER_TOOL_NAME_MAX_LEN.saturating_sub(suffix.len());
            let prefix = trim_to_char_boundary(&base, prefix_len).trim_end_matches('_');
            let encoded = if prefix.is_empty() {
                format!("tool{suffix}")
            } else {
                format!("{prefix}{suffix}")
            };
            match self.encoded_to_target.get(&encoded) {
                None => return encoded,
                Some(existing) if existing == target => return encoded,
                _ => counter += 1,
            }
        }
    }
}

const PROVIDER_TOOL_NAME_MAX_LEN: usize = 64;

pub fn encode_provider_tool_name(namespace: Option<&str>, name: &str) -> String {
    let namespace = namespace.map(str::trim).filter(|value| !value.is_empty());
    let name = name.trim();
    match namespace {
        Some(namespace) if !name.is_empty() => format!("{namespace}{NAMESPACE_MARKER}{name}"),
        _ => name.to_string(),
    }
}

pub fn decode_provider_tool_name(encoded: &str) -> ToolCallTarget {
    if encoded == TOOL_SEARCH_NAME {
        return ToolCallTarget::tool_search();
    }

    if let Some((namespace, name)) = split_marker(encoded, NAMESPACE_MARKER)
        .or_else(|| split_marker(encoded, LEGACY_NAMESPACE_MARKER))
    {
        return ToolCallTarget::function(Some(namespace), name);
    }

    ToolCallTarget::function(None, encoded)
}

fn provider_safe_tool_name(name: &str) -> String {
    let mut output = String::with_capacity(name.len().min(PROVIDER_TOOL_NAME_MAX_LEN));
    for ch in name.trim().chars() {
        let safe = if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            ch
        } else {
            '_'
        };
        output.push(safe);
    }
    if output.is_empty() {
        "tool".to_string()
    } else {
        output
    }
}

fn target_hash(target: &ToolCallTarget) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"codexhub-tool-name-v1\0");
    hasher.update(format!("{:?}", target.kind).as_bytes());
    hasher.update(b"\0");
    hasher.update(target.namespace.as_deref().unwrap_or("").as_bytes());
    hasher.update(b"\0");
    hasher.update(target.name.as_bytes());
    hex::encode(&hasher.finalize()[..5])
}

fn trim_to_char_boundary(value: &str, max_len: usize) -> &str {
    if value.len() <= max_len {
        return value;
    }
    let mut end = max_len;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}

fn split_marker<'a>(value: &'a str, marker: &str) -> Option<(&'a str, &'a str)> {
    let idx = value.find(marker)?;
    let namespace = &value[..idx];
    let name = &value[idx + marker.len()..];
    if namespace.is_empty() || name.is_empty() {
        return None;
    }
    Some((namespace, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_any_responses_namespace() {
        let encoded = encode_provider_tool_name(Some("codex_app"), "read_thread_terminal");
        assert_eq!(encoded, "codex_app__codexns__read_thread_terminal");

        let decoded = decode_provider_tool_name(&encoded);
        assert_eq!(decoded.kind, ToolCallKind::Function);
        assert_eq!(decoded.namespace.as_deref(), Some("codex_app"));
        assert_eq!(decoded.name, "read_thread_terminal");
    }

    #[test]
    fn decodes_legacy_responses_unit_marker() {
        let decoded = decode_provider_tool_name("multi_agent_v1responses_unit__spawn_agent");
        assert_eq!(decoded.kind, ToolCallKind::Function);
        assert_eq!(decoded.namespace.as_deref(), Some("multi_agent_v1"));
        assert_eq!(decoded.name, "spawn_agent");
    }

    #[test]
    fn encodes_names_for_strict_provider_tool_name_regex() {
        let mut map = ToolNameMap::default();
        let encoded = map.encode_function(Some("browser:control-in-app-browser"), "open page");

        assert!(encoded.len() <= 64);
        assert!(
            encoded
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
        );

        let decoded = map.decode(&encoded);
        assert_eq!(decoded.kind, ToolCallKind::Function);
        assert_eq!(
            decoded.namespace.as_deref(),
            Some("browser:control-in-app-browser")
        );
        assert_eq!(decoded.name, "open page");
    }

    #[test]
    fn hashes_long_or_colliding_tool_names_but_keeps_roundtrip() {
        let mut map = ToolNameMap::default();
        let first = map.encode_function(None, "a:b");
        let second = map.encode_function(None, "a_b");
        let long = map.encode_function(
            Some("very_long_namespace_with_many_segments_and_symbols"),
            "very_long_tool_name_that_would_exceed_the_provider_limit",
        );

        assert_ne!(first, second);
        assert!(long.len() <= 64);
        assert_eq!(map.decode(&first).name, "a:b");
        assert_eq!(map.decode(&second).name, "a_b");
        assert_eq!(
            map.decode(&long).namespace.as_deref(),
            Some("very_long_namespace_with_many_segments_and_symbols")
        );
    }

    #[test]
    fn preferred_function_name_reserves_name_and_keeps_collision_roundtrip() {
        let mut map = ToolNameMap::default();
        let view_image =
            map.encode_function_as(None, VIEW_IMAGE_TOOL_NAME, GROK_READ_FILE_TOOL_NAME);
        let original_read_file = map.encode_function(None, GROK_READ_FILE_TOOL_NAME);

        assert_eq!(view_image, GROK_READ_FILE_TOOL_NAME);
        assert_ne!(original_read_file, GROK_READ_FILE_TOOL_NAME);
        assert_eq!(
            map.decode(&view_image),
            ToolCallTarget::function(None, VIEW_IMAGE_TOOL_NAME)
        );
        assert_eq!(
            map.decode(&original_read_file),
            ToolCallTarget::function(None, GROK_READ_FILE_TOOL_NAME)
        );
    }
}
