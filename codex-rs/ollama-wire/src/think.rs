//! Per-model reasoning-suppression override table for the OllamaNative wire.
//!
//! Mirrors elf-dispatch's ollama-backend.js `resolveThink()`: models are matched against a
//! comma-separated "substr:value" table in order, first substring match wins. `value` is
//! `"false"` (suppress reasoning, the default when no override matches), `"true"` (leave
//! reasoning on), or an effort string ("low"/"medium"/"high") for models — like OpenAI's
//! gpt-oss family — that ignore a bare `think:false` and need an explicit effort level instead.
//!
//! This type doubles as the wire-serialization shape for the `think` request field — Task 6's
//! ChatRequest.think uses it directly, so Serialize is implemented here by hand (Ollama's native
//! API accepts either a JSON bool or a string) rather than duplicating an equivalent type there.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinkValue {
    Bool(bool),
    Effort(String),
}

impl Serialize for ThinkValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Bool(b) => serializer.serialize_bool(*b),
            Self::Effort(s) => serializer.serialize_str(s),
        }
    }
}

/// Resolve the `think` value to send for `model`, given an optional override table (the
/// `ModelProviderInfo.ollama_think` field's raw string). No override table, or no match within
/// it, both fall through to `ThinkValue::Bool(false)` — this wire path exists specifically to
/// suppress reasoning, so that's the correct default, not an unset/passthrough state.
pub fn resolve_think(model: &str, override_table: Option<&str>) -> ThinkValue {
    let Some(table) = override_table else {
        return ThinkValue::Bool(false);
    };

    for pair in table.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        // Model ids can contain ':' (e.g. "gemma4:26b-a4b-it-qat"), so split at the LAST colon —
        // mirrors ollama-backend.js's resolveThink() doing `pair.lastIndexOf(':')`.
        let Some(idx) = pair.rfind(':') else {
            continue;
        };
        let (substr, value) = (&pair[..idx], &pair[idx + 1..]);
        if substr.is_empty() || !model.contains(substr) {
            continue;
        }
        return match value {
            "false" => ThinkValue::Bool(false),
            "true" => ThinkValue::Bool(true),
            effort => ThinkValue::Effort(effort.to_string()),
        };
    }

    ThinkValue::Bool(false)
}

#[cfg(test)]
#[path = "think_tests.rs"]
mod tests;
