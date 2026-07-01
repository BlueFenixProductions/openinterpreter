use super::*;

#[test]
fn resolve_think_defaults_to_false_with_no_override_table() {
    assert_eq!(
        resolve_think("gemma4:26b-a4b-it-qat", None),
        ThinkValue::Bool(false)
    );
}

#[test]
fn resolve_think_matches_substring_and_returns_effort_string() {
    let table = "gpt-oss:low,qwen3:false";
    assert_eq!(
        resolve_think("gpt-oss:120b", Some(table)),
        ThinkValue::Effort("low".to_string())
    );
}

#[test]
fn resolve_think_explicit_false_string_forces_bool_false() {
    let table = "gpt-oss:low,qwen3:false";
    assert_eq!(
        resolve_think("qwen3:8b", Some(table)),
        ThinkValue::Bool(false)
    );
}

#[test]
fn resolve_think_no_match_in_table_falls_through_to_default_false() {
    let table = "gpt-oss:low";
    assert_eq!(
        resolve_think("gemma4:26b-a4b-it-qat", Some(table)),
        ThinkValue::Bool(false)
    );
}

#[test]
fn resolve_think_explicit_true_string() {
    let table = "some-model:true";
    assert_eq!(
        resolve_think("some-model:latest", Some(table)),
        ThinkValue::Bool(true)
    );
}
