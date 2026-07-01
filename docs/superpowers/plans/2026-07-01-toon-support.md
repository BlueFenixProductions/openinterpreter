# TOON support for Open Interpreter Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add TOON (Token-Oriented Object Notation) support to codex-rs on both the output side (guardian auto-reviewer structured answers) and the input side (MCP tool-call results re-injected into context), per the approved design spec.

**Architecture:** A new dependency-free leaf crate `codex-toon` wraps the `toon-format` crate's `encode`/`decode_strict`. `codex-core` is the only consumer: guardian's `parse_guardian_assessment` gets a TOON-fallback parse tier and a per-model-gated TOON prompt variant; `McpToolOutput::response_payload()` gets an experimental-flag-gated TOON re-encode of `structured_content` with silent JSON fallback on any failure.

**Tech Stack:** Rust, Cargo workspace, `toon-format` crate (crates.io, `default-features = false`), `just`/`cargo-nextest` for testing.

## Global Constraints

- Spec doc: `docs/superpowers/specs/2026-07-01-toon-support-design.md` — read it if a task brief is ambiguous; it is the source of truth for intent, this plan is the source of truth for exact code.
- Encode failures on the input side (MCP tool results) must always silently fall back to plain JSON — this is a token-saving optimization on a working path, never allowed to turn a successful tool call into a failed one (spec Resolved Decision 6).
- Decode on the output side (guardian) is a fallback tier *after* both JSON parse attempts fail — never tried first, never replaces the JSON path (spec Resolved Decision 6, mirrors elf-dispatch's `extractJson(text) ?? decodeToon(text)` ordering).
- `codex_toon::decode_toon` must reject any input starting with `{` or `[` before attempting a TOON decode — a compact single-line JSON object otherwise mis-parses as a one-line TOON `key: value` pair (spec, `decode_toon` doc comment). This guard is load-bearing; do not remove it or "simplify" it away.
- Both new `ConfigToml` fields (`AutoReviewToml.toon_capable_models`, `ConfigToml.experimental_toon_tool_results`) default to `None`/off — TOON must not change behavior for any existing installation that hasn't opted in.
- Run `just write-config-schema` (from `codex-rs/`) after adding either new config field, per `AGENTS.md`.
- Use `just test -p <crate>` / `just fmt` / `just fix -p <crate>`, never bare `cargo test` — this workspace's `justfile` wraps `cargo-nextest` and workspace-wide lint config.
- After any task touches `codex-rs/core/Cargo.toml`, `codex-rs/config/src/config_toml.rs`, or `codex-rs/core/src/config/mod.rs`, run `cargo check --workspace --all-targets --keep-going` (not just `-p <crate>`) before calling the task done — this workspace has repeatedly had struct-literal call sites elsewhere in the tree silently miss a new field (see native-ollama-backend plan's Tasks 2/3c history) and a narrow `cargo check` will not catch them.

---

### Task 1: `codex-toon` leaf crate

**Files:**
- Create: `codex-rs/toon/Cargo.toml`
- Create: `codex-rs/toon/src/lib.rs`
- Modify: `codex-rs/Cargo.toml:2-...` (add `"toon",` to `members`) and `codex-rs/Cargo.toml:140-...` (add `codex-toon = { path = "toon" }` to `[workspace.dependencies]`, alongside the existing `codex-ollama-wire = { path = "ollama-wire" }` entry)

**Interfaces:**
- Produces: `codex_toon::encode<T: serde::Serialize>(value: &T) -> Result<String, toon_format::ToonError>`, `codex_toon::decode_toon<T: serde::de::DeserializeOwned>(text: &str) -> Option<T>`, `codex_toon::toon_instruction() -> &'static str`. Task 2 and Task 3 both consume these exact signatures.

**Note on the spec's exact wording:** the design spec (`docs/superpowers/specs/2026-07-01-toon-support-design.md`) sketches `decode_toon(text: &str) -> Option<serde_json::Value>`. Verified against the real `toon-format` crate source (`github.com/toon-format/toon-rust`, `src/decode/mod.rs`): `decode_strict<T: serde::de::DeserializeOwned>(input: &str) -> ToonResult<T>` is generic over the target type, not `Value`-returning. Making `codex_toon::decode_toon` generic too (rather than fixed to `serde_json::Value` + a separate `serde_json::from_value` step) is a direct, idiomatic wrapper around the real API and lets callers (Task 2) decode straight into their target struct in one step, exactly like the existing `serde_json::from_str::<GuardianAssessmentPayload>(text)` call it sits beside. This is an implementation-level refinement of the spec's sketch, not a change to its intent — keep it.

- [ ] **Step 1: Add crate to the workspace**

Edit `codex-rs/Cargo.toml`. In the `members = [...]` array (starts line 2), add `"toon",` anywhere alphabetically reasonable (e.g. next to `"tools",` if present, or near the end). In the `[workspace.dependencies]` section (starts line 140), add, near the existing `codex-ollama-wire = { path = "ollama-wire" }` line:

```toml
codex-toon = { path = "toon" }
```

Also add the external dependency to `[workspace.dependencies]`:

```toml
toon-format = { version = "0.5", default-features = false }
```

- [ ] **Step 2: Create the crate skeleton**

Create `codex-rs/toon/Cargo.toml`:

```toml
[package]
name = "codex-toon"
version.workspace = true
edition.workspace = true
license.workspace = true

[lib]
name = "codex_toon"
path = "src/lib.rs"
doctest = false

[lints]
workspace = true

[dependencies]
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
toon-format = { workspace = true }

[dev-dependencies]
pretty_assertions = { workspace = true }
```

Confirm `serde`, `serde_json`, and `pretty_assertions` are already present as `[workspace.dependencies]` entries (they are — every other leaf crate in this workspace, including `codex-ollama-wire`, uses them the same way). If `cargo check -p codex-toon` reports a missing workspace dependency, add it to the root `Cargo.toml`'s `[workspace.dependencies]` following the existing entries' exact style before proceeding — do not hand-pin a version that diverges from how every other crate declares it.

- [ ] **Step 3: Write the failing tests**

Create `codex-rs/toon/src/lib.rs`:

```rust
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
```

- [ ] **Step 4: Run the tests to verify they fail correctly, then build them out**

Run: `source "$HOME/.cargo/env"; cd codex-rs && just test -p codex-toon`

Since Step 3 wrote both the implementation and its tests together (this is a thin wrapper crate — TDD's RED phase here is "crate doesn't exist yet," already true before Step 2), the meaningful check is that this now compiles and all 6 tests pass, not a pre-implementation red run. Expected: `cargo nextest` reports 6 tests passed, 0 failed.

If `decode_strict` behaves unexpectedly on malformed input (panics instead of returning `Err`, or a different error shape than assumed) — the spec flags this as a genuine unknown to confirm during implementation, not guessed. If it panics, wrap the call in `std::panic::catch_unwind` is the wrong fix (masks real bugs); instead add a test reproducing the panic and report it as a BLOCKED-then-resolved note in this task's report — do not silently work around a crate bug.

- [ ] **Step 5: Commit**

```bash
cd codex-rs && git add toon/ Cargo.toml Cargo.lock && git commit -m "$(cat <<'EOF'
feat(toon): add codex-toon leaf crate

Wraps toon-format's encode/decode_strict behind a small, dependency-free
API (encode, decode_toon, toon_instruction). Leaf crate so codex-core can
depend on it without adding weight to any lower-level crate.
EOF
)"
```

---

### Task 2: Guardian output-side TOON

**Files:**
- Modify: `codex-rs/config/src/config_toml.rs:554-557` (`AutoReviewToml`)
- Modify: `codex-rs/core/src/config/mod.rs:686` (add `Config.guardian_toon_capable_models` field) and the resolution block around `mod.rs:3371-3377` (mirror the existing `guardian_policy_config` resolution) and its struct-literal site around `mod.rs:3660`
- Modify: `codex-rs/core/Cargo.toml` (add `codex-toon = { workspace = true }`)
- Modify: `codex-rs/core/src/guardian/prompt.rs:589-703` (`parse_guardian_assessment`, `guardian_output_contract_prompt`, `guardian_policy_prompt`, `guardian_policy_prompt_with_config`)
- Modify: `codex-rs/core/src/guardian/review_session.rs:950-970` (`build_guardian_review_session_config`)
- Test: `codex-rs/core/src/guardian/tests.rs` (extends the existing test module — see the three `parse_guardian_assessment_*` tests at lines 1335-1374 for the exact pattern to follow)

**Interfaces:**
- Consumes: `codex_toon::encode`, `codex_toon::decode_toon`, `codex_toon::toon_instruction` from Task 1.
- Produces: nothing later tasks depend on — this is the terminal consumer of the guardian output-side seam.

**Context:** `parse_guardian_assessment` (`guardian/prompt.rs:589-603`) already tries `serde_json::from_str::<GuardianAssessmentPayload>(text)`, then falls back to slicing the outermost `{...}` and retrying. `guardian_output_contract_prompt()` (`prompt.rs:672-684`) is the JSON-contract text baked into `guardian_policy_prompt_with_config()` (`prompt.rs:699-703`), which `guardian_policy_prompt()` (`prompt.rs:695-697`) calls with the default policy file. Both are called from `build_guardian_review_session_config(parent_config: &Config, ..., active_model: &str, ...)` (`review_session.rs:950-979`) at line 964-970, which already receives `active_model: &str` — this is where per-model gating slots in. `GuardianAssessmentPayload` (`prompt.rs:632-638`) derives `Deserialize` already.

- [ ] **Step 1: Add the config field**

In `codex-rs/config/src/config_toml.rs`, find `AutoReviewToml` (currently lines 554-557):

```rust
pub struct AutoReviewToml {
    /// Additional policy instructions inserted into the guardian prompt.
    pub policy: Option<String>,
}
```

Change to:

```rust
pub struct AutoReviewToml {
    /// Additional policy instructions inserted into the guardian prompt.
    pub policy: Option<String>,

    /// Comma-separated model-id substrings that are bench-proven to reliably emit valid TOON for
    /// the guardian assessment schema (see docs/superpowers/specs/2026-07-01-toon-support-design.md).
    /// A model is asked for TOON output only if its id contains one of these substrings; unset or
    /// empty means TOON is off for every model (unconditional JSON, today's behavior). Mirrors
    /// elf-dispatch's LLM_TOON_REVIEW: populate this only after confirming the target model
    /// actually emits valid TOON for this schema — don't guess.
    pub toon_capable_models: Option<String>,
}
```

- [ ] **Step 2: Thread the field through to the runtime `Config` struct**

In `codex-rs/core/src/config/mod.rs`, find the `guardian_policy_config` field declaration (currently around line 686):

```rust
    pub guardian_policy_config: Option<String>,
```

Add a sibling field immediately after it:

```rust
    /// Comma-separated model-id substrings gated in for guardian TOON output.
    /// Mirrors `guardian_policy_config`'s resolution from `AutoReviewToml`. See
    /// docs/superpowers/specs/2026-07-01-toon-support-design.md.
    pub guardian_toon_capable_models: Option<String>,
```

`guardian_policy_config` is resolved at `mod.rs:3371-3379`, immediately before `let personality = ...` (line 3380):

```rust
        let guardian_policy_config =
            guardian_policy_config_from_requirements(config_layer_stack.requirements_toml())
                .or_else(|| {
                    cfg.auto_review
                        .as_ref()
                        .and_then(|auto_review| normalize_guardian_policy_config(
                            auto_review.policy.as_deref(),
                        ))
                });
```

`cfg: &ConfigToml` is already in scope in this function. The `guardian_policy_config_from_requirements(...)` call threads through a separate enterprise-managed "requirements" override layer (`Sourced<String>`/`RequirementSource`) that `toon_capable_models` has no equivalent of — don't replicate it. Add this simpler resolution as a new `let` binding immediately after the `guardian_policy_config` block (before `let personality = ...`):

```rust
        let guardian_toon_capable_models = cfg
            .auto_review
            .as_ref()
            .and_then(|auto_review| auto_review.toon_capable_models.clone());
```

Add `guardian_toon_capable_models,` to the `Config { ... }` struct-literal construction near `mod.rs:3660` (wherever `guardian_policy_config,` appears in that same literal — add the new field as a direct sibling).

Run `cargo check -p codex-core --all-targets 2>&1 | grep -A3 "guardian_toon_capable_models\|missing field"` and fix every additional struct-literal site the compiler reports (there are unit-test config builders in this crate that construct `Config` with every field spelled out explicitly — the compiler will name each one).

- [ ] **Step 3: Add codex-toon as a dependency of codex-core**

In `codex-rs/core/Cargo.toml`, add to `[dependencies]` (alphabetical among the existing `codex-*` entries):

```toml
codex-toon = { workspace = true }
```

- [ ] **Step 4: Write the failing tests for the TOON parse fallback**

In `codex-rs/core/src/guardian/tests.rs`, add near the existing `parse_guardian_assessment_*` tests (after line 1374's closing brace):

```rust
#[test]
fn parse_guardian_assessment_falls_back_to_toon_when_json_fails() {
    let toon_reply = "risk_level: medium\nuser_authorization: low\noutcome: allow\nrationale: ok";

    let parsed = parse_guardian_assessment(Some(toon_reply)).expect("guardian assessment");

    assert_eq!(
        parsed,
        GuardianAssessment {
            risk_level: GuardianRiskLevel::Medium,
            user_authorization: GuardianUserAuthorization::Low,
            outcome: GuardianAssessmentOutcome::Allow,
            rationale: "ok".to_string(),
        }
    );
}

#[test]
fn parse_guardian_assessment_prefers_json_over_toon_when_both_present() {
    // A reply that happens to look like it could be TOON-adjacent but starts with `{` must take
    // the JSON path, never the TOON path — this is the ordering the spec's Global Constraints
    // section requires (JSON first, TOON only after JSON fails).
    let json_reply = r#"{"outcome":"deny","rationale":"json wins"}"#;

    let parsed = parse_guardian_assessment(Some(json_reply)).expect("guardian assessment");

    assert_eq!(parsed.rationale, "json wins");
}

#[test]
fn parse_guardian_assessment_rejects_invalid_toon_and_json() {
    let garbage = "not valid json and not valid toon either {{{";

    let err = parse_guardian_assessment(Some(garbage)).expect_err("should fail to parse");

    assert!(err.to_string().contains("JSON or TOON"));
}
```

- [ ] **Step 5: Run the tests to verify they fail**

Run: `source "$HOME/.cargo/env"; cd codex-rs && just test -p codex-core -- parse_guardian_assessment`
Expected: the three new tests FAIL (`parse_guardian_assessment_falls_back_to_toon_when_json_fails` and `parse_guardian_assessment_rejects_invalid_toon_and_json` fail because there is no TOON fallback tier yet — the "not valid json and not valid toon either" case currently already fails, but for a different reason; check the actual failure message says "was not valid JSON" not "JSON or TOON" to confirm you're failing for the right reason before moving on). `parse_guardian_assessment_prefers_json_over_toon_when_both_present` passes already (no behavior change needed for it) — that's fine, it's a regression guard, not new-behavior RED.

- [ ] **Step 6: Add the TOON fallback tier**

In `codex-rs/core/src/guardian/prompt.rs`, change `parse_guardian_assessment` (currently lines 589-603):

```rust
pub(crate) fn parse_guardian_assessment(text: Option<&str>) -> anyhow::Result<GuardianAssessment> {
    let Some(text) = text else {
        anyhow::bail!("guardian review completed without an assessment payload");
    };
    let parsed_payload =
        if let Ok(payload) = serde_json::from_str::<GuardianAssessmentPayload>(text) {
            payload
        } else if let (Some(start), Some(end)) = (text.find('{'), text.rfind('}'))
            && start < end
            && let Some(slice) = text.get(start..=end)
            && let Ok(payload) = serde_json::from_str::<GuardianAssessmentPayload>(slice)
        {
            payload
        } else if let Some(payload) = codex_toon::decode_toon::<GuardianAssessmentPayload>(text) {
            payload
        } else {
            anyhow::bail!("guardian assessment was not valid JSON or TOON");
        };
    // ... rest of the function (outcome/risk_level/rationale derivation) is unchanged
```

Note the change from the original's `serde_json::from_str::<GuardianAssessmentPayload>(slice)?` (which used `?` to propagate a parse error immediately) to `&& let Ok(payload) = ... { payload }` — this is required so that a `{...}`-slice that fails JSON parsing falls through to the TOON attempt instead of bailing immediately. Confirm this change doesn't alter the two existing passing tests' behavior (`parse_guardian_assessment_extracts_embedded_json`, and the bare-allow/bare-deny tests) — it shouldn't, since valid embedded JSON still matches on the first `if let Ok(payload) = ...` of this branch.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env"; cd codex-rs && just test -p codex-core -- guardian`
Expected: all guardian tests pass, including the three new ones and the pre-existing `parse_guardian_assessment_*` tests.

- [ ] **Step 8: Add the TOON prompt variant and thread gating through**

In `codex-rs/core/src/guardian/prompt.rs`, add a TOON-flavored contract prompt next to the existing one (after `guardian_output_contract_prompt()`, currently ending at line 684):

```rust
/// TOON-flavored variant of `guardian_output_contract_prompt()`, used only for models
/// bench-confirmed to reliably emit TOON for this schema (see
/// docs/superpowers/specs/2026-07-01-toon-support-design.md). Layers `codex_toon::toon_instruction()`
/// over the same field contract described in `guardian_output_schema()`, mirroring elf-dispatch's
/// SYSTEM_REVIEW_TOON.
fn guardian_output_contract_prompt_toon() -> String {
    format!(
        "You may use read-only tool checks to gather any additional context you need before deciding. When you are ready to answer, your final message must be TOON (Token-Oriented Object Notation), not JSON.\n\n{}\n\nFields: risk_level (\"low\"|\"medium\"|\"high\"|\"critical\"), user_authorization (\"unknown\"|\"low\"|\"medium\"|\"high\"), outcome (\"allow\"|\"deny\", required), rationale (string).\n\nFor low-risk actions, give the final answer directly: outcome: allow",
        codex_toon::toon_instruction()
    )
}
```

Change `guardian_policy_prompt_with_config` and `guardian_policy_prompt` (currently `prompt.rs:695-703`) to take a `use_toon: bool`:

```rust
pub(crate) fn guardian_policy_prompt(use_toon: bool) -> String {
    guardian_policy_prompt_with_config(include_str!("policy.md"), use_toon)
}

pub(crate) fn guardian_policy_prompt_with_config(tenant_policy_config: &str, use_toon: bool) -> String {
    let template = include_str!("policy_template.md").trim_end();
    let prompt = template.replace("{tenant_policy_config}", tenant_policy_config.trim());
    let contract = if use_toon {
        guardian_output_contract_prompt_toon()
    } else {
        guardian_output_contract_prompt().to_string()
    };
    format!("{prompt}\n\n{contract}\n")
}
```

This changes both functions' signatures — every caller must be updated (Step 9 handles the production caller; the compiler will name any others, including the two direct calls in `guardian/tests.rs` at lines 2939 and 3196, and the one in `session/tests/guardian_tests.rs:516` — pass `false` at each of those three test call sites unless the specific test is about TOON gating, since they test unrelated guardian behavior and should keep today's JSON-only prompt).

- [ ] **Step 9: Wire per-model gating in `build_guardian_review_session_config`**

In `codex-rs/core/src/guardian/review_session.rs`, change the `base_instructions` construction (currently lines 964-970):

```rust
    guardian_config.base_instructions = Some({
        let use_toon = parent_config
            .guardian_toon_capable_models
            .as_deref()
            .is_some_and(|list| {
                list.split(',')
                    .map(str::trim)
                    .filter(|substr| !substr.is_empty())
                    .any(|substr| active_model.contains(substr))
            });
        parent_config
            .guardian_policy_config
            .as_deref()
            .map(|config| guardian_policy_prompt_with_config(config, use_toon))
            .unwrap_or_else(|| guardian_policy_prompt(use_toon))
    });
```

- [ ] **Step 10: Add a gating test**

In `codex-rs/core/src/guardian/tests.rs`, add a test confirming an unlisted model gets the JSON contract and a listed model gets the TOON contract — read `guardian/tests.rs` around lines 2900-2950 first (the existing calls to `guardian_policy_prompt()` at line 2939) to find how a `Config` with `auto_review` set is already constructed in this test file (there is very likely an existing helper or `test_config()` pattern used elsewhere in this file per the `use crate::config::test_config;` import at the top of the file); build the new test on that same pattern rather than hand-rolling a new `Config` construction:

```rust
#[test]
fn guardian_policy_prompt_includes_toon_instruction_when_use_toon_true() {
    let prompt = guardian_policy_prompt(true);
    assert!(prompt.contains("Output ONLY TOON"));
}

#[test]
fn guardian_policy_prompt_omits_toon_instruction_when_use_toon_false() {
    let prompt = guardian_policy_prompt(false);
    assert!(!prompt.contains("Output ONLY TOON"));
}
```

- [ ] **Step 11: Run the full guardian test suite**

Run: `source "$HOME/.cargo/env"; cd codex-rs && just test -p codex-core -- guardian`
Expected: all tests pass, 0 failures.

- [ ] **Step 12: Regenerate the config schema**

Run: `source "$HOME/.cargo/env"; cd codex-rs && just write-config-schema`
Expected: `core/config.schema.json` diff shows the new `toon_capable_models` property under the `auto_review` object and the new `guardian_toon_capable_models`-adjacent... (confirm the schema generator surfaces `AutoReviewToml`'s field, not `Config`'s internal-only field — `Config` itself is not schema-generated, only `ConfigToml`/`AutoReviewToml` are, per how `AutoReviewToml.policy` already appears in the existing schema).

- [ ] **Step 13: Full-workspace compile sweep**

Run: `source "$HOME/.cargo/env"; cd codex-rs && cargo check --workspace --all-targets --keep-going 2>&1 | tee /tmp/toon-task2-check.log; grep -c "^error" /tmp/toon-task2-check.log`
Expected: 0 (per Global Constraints — this catches any struct-literal `Config { ... }` construction elsewhere in the workspace that doesn't use `..Default::default()` and now needs `guardian_toon_capable_models` spelled out explicitly). Fix every reported site by adding `guardian_toon_capable_models: None,` (or the appropriate test value) before proceeding.

- [ ] **Step 14: Commit**

```bash
cd codex-rs && git add config/src/config_toml.rs core/src/config/mod.rs core/Cargo.toml core/src/guardian/prompt.rs core/src/guardian/review_session.rs core/src/guardian/tests.rs core/src/session/tests/guardian_tests.rs core/config.schema.json Cargo.lock && git commit -m "$(cat <<'EOF'
feat(guardian): add TOON output support, gated per-model

parse_guardian_assessment gains a TOON fallback tier after both JSON
parse attempts fail. Models listed in auto_review.toon_capable_models
get a TOON-flavored contract prompt instead of the JSON one; unlisted
models are unaffected. Off by default.
EOF
)"
```

---

### Task 3: MCP input-side TOON

**Files:**
- Modify: `codex-rs/config/src/config_toml.rs` (add `experimental_toon_tool_results` near the other `experimental_*` fields, e.g. after `experimental_use_unified_exec_tool` at line 506)
- Modify: `codex-rs/core/src/config/mod.rs:701` (add `Config.experimental_toon_tool_results: bool` field), `:3370` (resolution) and `:3585` (struct-literal site) — mirrors `include_environment_context`'s three sites exactly (**not** `experimental_use_unified_exec_tool`, which routes through the separate `FeatureConfigSource`/`Features` subsystem this field doesn't need — confirmed by reading `mod.rs:2790-2803`, where `experimental_use_unified_exec_tool` is copied into a `FeatureConfigSource { .. }` literal, not `Config` directly)
- Modify: `codex-rs/core/src/tools/context.rs:66-72` (`McpToolOutput` struct) and `:111-140` (`response_payload()`)
- Modify: `codex-rs/core/src/tools/handlers/mcp.rs:154-160` (the one production `McpToolOutput { ... }` construction)
- Modify: `codex-rs/core/src/tools/context_tests.rs` (4 existing `McpToolOutput { ... }` constructions at lines 90, 140, 184, 237 — compiler will force these) and `codex-rs/core/src/tools/handlers/mcp.rs`'s own test module (1 construction at line 431)
- Test: `codex-rs/core/src/tools/context_tests.rs`

**Interfaces:**
- Consumes: `codex_toon::encode` from Task 1 (already a `codex-core` dependency as of Task 2 Step 3).
- Produces: nothing later tasks depend on — this is the terminal consumer of the MCP input-side seam.

**Context:** `McpToolOutput::response_payload()` (`context.rs:111-140`) is the "context-injection form" (its own comment says so) — it calls `self.result.as_function_call_output_payload()` (`protocol/src/models.rs:1925`, unmodified by this task — do not touch that function or its other 3 call sites in `stream_events_utils.rs`, `tools/src/tool_output.rs`, and `models.rs:1969` itself; they are out of scope per the spec's Non-goals), which produces `FunctionCallOutputBody::Text(serialized_structured_content)` whenever `self.result.structured_content` is `Some` and non-null. `McpToolOutput` already carries per-construction, config-derived fields (`truncation_policy`, `original_image_detail_supported`) set at its one production construction site in `tools/handlers/mcp.rs:154-160`, which has `turn: Arc<TurnContext>` in scope, and `TurnContext.config: Arc<Config>` (`session/turn_context.rs:107`).

- [ ] **Step 1: Add the config field**

In `codex-rs/config/src/config_toml.rs`, find `experimental_use_unified_exec_tool` (currently line 506) and add a sibling field near it:

```rust
    /// Encode MCP tool results' `structured_content` as TOON instead of JSON when re-injecting
    /// them into the model's context on later turns, saving input tokens on uniform-array results.
    /// Unlike guardian's TOON output, this has no bench-validated per-model allowlist backing it —
    /// enabling it is a bet that models comprehend TOON input as well as JSON, not a proven one.
    /// Falls back to JSON on any encode failure. Default: disabled.
    pub experimental_toon_tool_results: Option<bool>,
```

- [ ] **Step 2: Thread the field through to the runtime `Config` struct**

`include_environment_context` is the correct precedent to mirror — a plain `Option<bool>` on `ConfigToml`, resolved once with a default into a definite `bool` stored directly on `Config`. Its three real sites:

`mod.rs:701`, the `Config` struct field declaration:
```rust
    pub include_environment_context: bool,
```

`mod.rs:3370`, the resolution (immediately before `let guardian_policy_config = ...` at line 3371):
```rust
        let include_environment_context = cfg.include_environment_context.unwrap_or(true);
```

`mod.rs:3585`, the struct-literal site:
```rust
            include_environment_context,
```

Add the same three sites for the new field — declare it on `Config` near line 701:

```rust
    pub experimental_toon_tool_results: bool,
```

resolve it near line 3370 (default `false`, unlike `include_environment_context`'s default `true` — this field must be off by default per Global Constraints):

```rust
        let experimental_toon_tool_results = cfg.experimental_toon_tool_results.unwrap_or(false);
```

and add `experimental_toon_tool_results,` to the same struct literal as `include_environment_context,` at line 3585.

Run `cargo check -p codex-core --all-targets 2>&1 | grep -B2 "missing field.*experimental_toon_tool_results"` and add the field at every additional reported site (test builders elsewhere in this crate that construct `Config` with every field spelled out explicitly).

- [ ] **Step 3: Add the field to `McpToolOutput`**

In `codex-rs/core/src/tools/context.rs`, change (currently lines 66-72):

```rust
#[derive(Clone, Debug)]
pub struct McpToolOutput {
    pub result: CallToolResult,
    pub tool_input: JsonValue,
    pub wall_time: Duration,
    pub original_image_detail_supported: bool,
    pub truncation_policy: TruncationPolicy,
}
```

to:

```rust
#[derive(Clone, Debug)]
pub struct McpToolOutput {
    pub result: CallToolResult,
    pub tool_input: JsonValue,
    pub wall_time: Duration,
    pub original_image_detail_supported: bool,
    pub truncation_policy: TruncationPolicy,
    pub encode_structured_content_as_toon: bool,
}
```

- [ ] **Step 4: Write the failing test**

In `codex-rs/core/src/tools/context_tests.rs`, find `mcp_tool_output_response_item_includes_wall_time` (currently starting around line 88) to see the exact `McpToolOutput { ... }` field list and imports used in this file, then add a new test near it:

```rust
#[test]
fn mcp_tool_output_response_payload_encodes_structured_content_as_toon_when_enabled() {
    let output = McpToolOutput {
        result: CallToolResult {
            content: vec![serde_json::json!({ "type": "text", "text": "" })],
            structured_content: Some(serde_json::json!({ "bytes": 5 })),
            is_error: Some(false),
            meta: None,
        },
        tool_input: json!({}),
        wall_time: Duration::from_secs(1),
        original_image_detail_supported: true,
        truncation_policy: TruncationPolicy::default(),
        encode_structured_content_as_toon: true,
    };

    let payload = output.response_payload();
    let text = payload.body.to_text().expect("text body");

    assert!(text.contains("bytes: 5"), "expected TOON body, got: {text}");
    assert!(!text.contains("{\"bytes\":5}"), "should not contain JSON, got: {text}");
}

#[test]
fn mcp_tool_output_response_payload_stays_json_when_toon_disabled() {
    let output = McpToolOutput {
        result: CallToolResult {
            content: vec![serde_json::json!({ "type": "text", "text": "" })],
            structured_content: Some(serde_json::json!({ "bytes": 5 })),
            is_error: Some(false),
            meta: None,
        },
        tool_input: json!({}),
        wall_time: Duration::from_secs(1),
        original_image_detail_supported: true,
        truncation_policy: TruncationPolicy::default(),
        encode_structured_content_as_toon: false,
    };

    let payload = output.response_payload();
    let text = payload.body.to_text().expect("text body");

    assert!(text.contains("{\"bytes\":5}"), "expected JSON body, got: {text}");
}
```

Confirm the exact construction fields above (`content`, `is_error`, `meta`, `TruncationPolicy::default()`, the `json!` macro import) against what's really in `context_tests.rs` — this sketch mirrors the pattern already visible at lines 80-100 of that file but adjust any field name or import path that doesn't match the real file once you have it open. `response_payload()` is currently a private method on `impl McpToolOutput` (`context.rs:111`, no `pub`) — check whether it's reachable from `context_tests.rs` (likely a `#[cfg(test)] mod context_tests` submodule of the same file, or a sibling module with `use super::*`); if it isn't reachable, use the existing test pattern for this file, which likely already exercises `response_payload()` indirectly through `to_response_item()` (a `pub` trait method) instead — check `mcp_tool_output_response_item_includes_wall_time`'s own test body for exactly how it currently reaches this payload, and mirror that.

- [ ] **Step 5: Run the tests to verify they fail**

Run: `source "$HOME/.cargo/env"; cd codex-rs && just test -p codex-core -- mcp_tool_output_response_payload`
Expected: compile failure first (`McpToolOutput` doesn't have `encode_structured_content_as_toon` yet — this is the correct RED state for a struct-field addition: a compile error, not a runtime assertion failure). Confirm the compiler error names exactly the field you're about to add, not something else.

- [ ] **Step 6: Implement the TOON encode with fallback**

In `codex-rs/core/src/tools/context.rs`, change `response_payload()` (currently lines 111-140):

```rust
    fn response_payload(&self) -> FunctionCallOutputPayload {
        let mut payload = self.result.as_function_call_output_payload();
        if let Some(items) = payload.content_items_mut() {
            sanitize_original_image_detail(self.original_image_detail_supported, items);
        }

        if self.encode_structured_content_as_toon
            && let Some(structured_content) = &self.result.structured_content
            && !structured_content.is_null()
            && let FunctionCallOutputBody::Text(text) = &mut payload.body
            && let Ok(toon_text) = codex_toon::encode(structured_content)
        {
            *text = toon_text;
        }

        let wall_time_seconds = self.wall_time.as_secs_f64();
        let header = format!("Wall time: {wall_time_seconds:.4} seconds\nOutput:");

        match &mut payload.body {
            FunctionCallOutputBody::Text(text) => {
                if text.is_empty() {
                    *text = header;
                } else {
                    *text = format!("{header}\n{text}");
                }
            }
            FunctionCallOutputBody::ContentItems(items) => {
                items.insert(0, FunctionCallOutputContentItem::InputText { text: header });
            }
        }

        // This is the context-injection form, so keep it aligned with the
        // function-call output truncation that conversation history already
        // applies. Code-mode consumers still get the raw `CallToolResult`.
        //
        // The text is serialized again inside the Responses payload, so allow
        // a small buffer for JSON escaping and wrapper overhead.
        truncate_function_output_payload(&payload, self.truncation_policy * 1.2)
    }
```

The new block runs strictly before the wall-time header is prepended, so the TOON text (not the JSON text) gets the header — matches the existing ordering exactly. On any encode failure (the `&& let Ok(...)` guard is false), `payload.body` is untouched and stays whatever `as_function_call_output_payload()` already produced — the silent JSON fallback the spec's Global Constraints require, with zero extra code needed for the failure path.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env"; cd codex-rs && just test -p codex-core -- mcp_tool_output`
Expected: all tests in this group pass, including the two new ones and every pre-existing `mcp_tool_output_*` test (they all now need `encode_structured_content_as_toon: false` added to their construction — the compiler will have already forced this in Step 5's failed build; confirm you set it to `false` for every pre-existing test since none of them were testing TOON behavior).

- [ ] **Step 8: Wire the config flag into the one production construction site**

In `codex-rs/core/src/tools/handlers/mcp.rs`, change the `McpToolOutput { ... }` construction (currently lines 154-160):

```rust
        Ok(boxed_tool_output(McpToolOutput {
            result: result.result,
            tool_input: result.tool_input,
            wall_time: started.elapsed(),
            original_image_detail_supported: can_request_original_image_detail(&turn.model_info),
            truncation_policy: turn.model_info.truncation_policy.into(),
            encode_structured_content_as_toon: turn.config.experimental_toon_tool_results,
        }))
```

Also fix the compile error at this file's own test module (`mcp.rs`'s inline `#[cfg(test)] mod tests`, the `McpToolOutput { ... }` construction currently at line 431) by adding `encode_structured_content_as_toon: false,` — that test (`mcp_post_tool_use_payload_uses_prefixed_tool_name_args_and_result`) is unrelated to TOON.

- [ ] **Step 9: Full-workspace compile sweep**

Run: `source "$HOME/.cargo/env"; cd codex-rs && cargo check --workspace --all-targets --keep-going 2>&1 | tee /tmp/toon-task3-check.log; grep -c "^error" /tmp/toon-task3-check.log`
Expected: 0. Fix every reported site (any other `McpToolOutput { ... }` or `Config { ... }` construction elsewhere in the workspace that doesn't use `..Default::default()`).

- [ ] **Step 10: Regenerate the config schema**

Run: `source "$HOME/.cargo/env"; cd codex-rs && just write-config-schema`
Expected: `core/config.schema.json` diff shows the new top-level `experimental_toon_tool_results` boolean property.

- [ ] **Step 11: Run the full core test suite for the touched modules**

Run: `source "$HOME/.cargo/env"; cd codex-rs && just test -p codex-core -- tools::context`
Expected: all tests pass, 0 failures.

- [ ] **Step 12: Commit**

```bash
cd codex-rs && git add config/src/config_toml.rs core/src/config/mod.rs core/src/tools/context.rs core/src/tools/context_tests.rs core/src/tools/handlers/mcp.rs core/config.schema.json Cargo.lock && git commit -m "$(cat <<'EOF'
feat(mcp): encode structured tool-call results as TOON, opt-in

McpToolOutput::response_payload() re-encodes structured_content as
TOON instead of JSON when experimental_toon_tool_results is enabled,
saving input tokens on every subsequent turn until compaction. Falls
back to plain JSON silently on any encode failure. Off by default.
EOF
)"
```

---

### Task 4: Full workspace verification and live smoke test

**Files:** none created or modified unless verification surfaces a gap; if it does, the fix belongs in whichever of Task 1/2/3's files the gap traces back to, following that task's existing patterns.

**Interfaces:** none — this task consumes everything Tasks 1-3 produced and produces nothing further.

- [ ] **Step 1: Full workspace test sweep**

Run: `source "$HOME/.cargo/env"; cd codex-rs && just test -p codex-toon && just test -p codex-core -- guardian && just test -p codex-core -- tools::context`
Expected: 0 failures across all three.

- [ ] **Step 2: Full workspace lint sweep**

Run: `source "$HOME/.cargo/env"; cd codex-rs && just fix -p codex-toon && just fix -p codex-core && just fmt`
Expected: no unresolved clippy warnings introduced by this plan's changes (pre-existing warnings elsewhere in the workspace, if any, are out of scope — confirm any warning you see is not on a line this plan touched before ignoring it).

- [ ] **Step 3: Live verification — guardian TOON output against a real Ollama model**

Using the native-ollama-backend work already merged (`bluefenix/main`, PR #1), set `guardian_toon_capable_models` in a local `config.toml` to a real installed model id (e.g. `"qwen3-coder"` if available, matching elf-dispatch's own bench-blessed model for this exact schema per `docs/findings/2026-06-23-toon-output-bench-gate.md` in `/home/chris/Documents/GitHub/elf-dispatch`), trigger a guardian review against that model (any action that invokes the auto-reviewer), and confirm in the transcript/logs that the guardian's final message is TOON-shaped (`outcome: allow` or `outcome: deny` on its own line, not `{"outcome":...}`) and that `parse_guardian_assessment` still produced a correct `GuardianAssessment`. Record the raw reply and parsed result the same way the native-ollama-backend plan's Task 10 captured its comparison JSON (a scratch file under `.superpowers/sdd/`, referenced from this task's report — not a permanent bench harness, per the spec's Non-goals).

If no locally-installed model is confirmed TOON-capable for this schema, this step cannot produce a real result — report that explicitly (BLOCKED or DONE_WITH_CONCERNS, not a fabricated pass) rather than skipping it silently.

- [ ] **Step 4: Live verification — MCP TOON input**

Set `experimental_toon_tool_results = true` in a local `config.toml`, invoke an MCP tool that returns `structured_content` (any configured MCP server with a tool returning structured JSON — check `codex-rs/core/tests/suite/` for an existing MCP-mock test harness pattern to drive this without a real external MCP server, per this workspace's own convention of preferring `test_codex`/mocked integration tests over live external dependencies where one exists), and confirm the text re-injected into context is TOON-shaped, not JSON. Record before/after token counts for the same structured payload (`serde_json::to_string` length vs. `codex_toon::encode` output length is a reasonable proxy if a live model call isn't practical here) in the same scratch-artifact style as Step 3.

- [ ] **Step 5: Update the progress ledger**

Append to `.superpowers/sdd/progress.md`:

```
=== PLAN COMPLETE (TOON support) === All 4 tasks done, reviewed, and approved. Final HEAD: <fill in actual HEAD sha>
```
