pub fn encode<T: serde::Serialize>(value: &T) -> Result<String, toon_format::ToonError> {
    toon_format::encode_default(value)
}

/// Ports elf-dispatch's `decodeToon` guard (src/dispatch/toon.js, `/home/chris/Documents/GitHub/elf-dispatch`):
/// TOON's top level is always `key: value` / `key[N]{...}:` lines and never starts with `{` or `[`.
/// A compact single-line JSON object (`{"ok":1}`) would otherwise decode as a one-line TOON
/// `key: value` pair (key `{"ok"`, value `1}`) — reject brace/bracket-leading input here so JSON
/// always wins the ambiguous case, matching the original's load-bearing guard.
pub fn decode_toon<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    let body = text.trim();
    let body = body
        .strip_prefix("```toon")
        .or_else(|| body.strip_prefix("```"))
        .unwrap_or(body)
        .trim_start_matches('\n');
    let body = body.strip_suffix("```").unwrap_or(body).trim();
    if body.is_empty() {
        return None;
    }
    if body.starts_with('{') || body.starts_with('[') {
        return None;
    }
    toon_format::decode_strict(body).ok()
}

/// System-prompt fragment teaching TOON output. Ported near-verbatim from elf-dispatch's
/// `toonInstruction()` (src/dispatch/toon.js).
pub fn toon_instruction() -> &'static str {
    "Output ONLY TOON (Token-Oriented Object Notation). No prose, no markdown fences, no JSON.\n\
     Rules:\n\
     - An object is indented `key: value` lines.\n\
     - An array of STRINGS/numbers is inline: `key[N]: v1,v2,v3` (N = exact count).\n\
     - An array of OBJECTS is a header `key[N]{field1,field2,...}:` then N indented rows of \
     comma-separated values in that field order.\n\
     - N MUST equal the number of rows/values. Quote a value only if it contains a comma, colon, or quote.\n\
     Example — the object \
     {\"title\":\"Fix auth\",\"bullets\":[\"add check\",\"drop header\"],\"bugs\":[{\"location\":\"a.js:1\",\"severity\":\"high\",\"description\":\"null deref\"}],\"needs_tests\":true} \
     is exactly:\n\
     title: Fix auth\n\
     bullets[2]: add check,drop header\n\
     bugs[1]{location,severity,description}:\n\
     \x20 \"a.js:1\",high,null deref\n\
     needs_tests: true"
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serde_json::json;

    #[test]
    fn encode_then_decode_round_trips_an_object() {
        let value = json!({"name": "Alice", "age": 30});
        let encoded = encode(&value).expect("encode");
        let decoded: serde_json::Value = decode_toon(&encoded).expect("decode");
        assert_eq!(decoded, value);
    }

    #[test]
    fn decode_toon_rejects_brace_leading_input() {
        let decoded: Option<serde_json::Value> = decode_toon(r#"{"ok":1}"#);
        assert_eq!(decoded, None);
    }

    #[test]
    fn decode_toon_rejects_bracket_leading_input() {
        let decoded: Option<serde_json::Value> = decode_toon(r#"[1,2,3]"#);
        assert_eq!(decoded, None);
    }

    #[test]
    fn decode_toon_strips_a_toon_fence() {
        let value = json!({"outcome": "allow"});
        let encoded = encode(&value).expect("encode");
        let fenced = format!("```toon\n{encoded}\n```");
        let decoded: serde_json::Value = decode_toon(&fenced).expect("decode fenced");
        assert_eq!(decoded, value);
    }

    #[test]
    fn decode_toon_returns_none_for_empty_input() {
        let decoded: Option<serde_json::Value> = decode_toon("   ");
        assert_eq!(decoded, None);
    }

    #[test]
    fn toon_instruction_forbids_json_and_prose() {
        let instruction = toon_instruction();
        assert!(instruction.contains("Output ONLY TOON"));
        assert!(instruction.contains("no JSON"));
    }
}
