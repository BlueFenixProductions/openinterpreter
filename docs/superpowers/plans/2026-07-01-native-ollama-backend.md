# Native Ollama Backend Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a fourth wire protocol (`WireApi::OllamaNative`) to Open Interpreter that talks to Ollama's native `/api/chat` endpoint instead of the OpenAI-compat surface, so reasoning suppression (`think:false`) actually works — the identical fix elf-dispatch's own `ollama-backend.js` already applies for the same reason.

**Architecture:** New `WireApi::OllamaNative` + `StreamTransportRoute::OllamaNativeChat` variants follow the exact seam the codebase already uses to separate wire protocol from harness shaping (`resolve_stream_transport_route()`). All new request/response/streaming logic lives in the existing `codex-ollama` crate (not `codex-core`, per this repo's own `AGENTS.md`), registered as a new `ollama-native` provider id alongside the existing `ollama` (additive, no change to current behavior).

**Tech Stack:** Rust (2024 edition), `reqwest` (streaming), `serde_json`, `tokio`, the existing `codex-ollama` crate's `line_buffer.rs` for NDJSON framing, `wiremock` for HTTP-level tests.

## Global Constraints

- Never add or modify code related to `CODEX_SANDBOX_NETWORK_DISABLED_ENV_VAR` or `CODEX_SANDBOX_ENV_VAR` (AGENTS.md).
- Always collapse `if` statements per clippy's `collapsible_if`; always inline `format!` args (`AGENTS.md`).
- Prefer RPITIT (`fn foo(&self) -> impl Future<Output = T> + Send`) over `#[async_trait]`/`#[allow(async_fn_in_trait)]` for any new trait method — existing code in this plan uses plain `async fn` on concrete structs, which is fine (traits only need the RPITIT shape).
- Target new Rust modules under ~500 LoC; split into a new module rather than growing an existing file past ~800 LoC (AGENTS.md).
- Run `just fmt` after every task's code changes (AGENTS.md says do this automatically, no approval needed).
- Use `just test -p <crate>` for that crate's tests, never bare `cargo test` (AGENTS.md).
- If `ConfigToml`/`ModelProviderInfo` changes, run `just write-config-schema` to refresh `codex-rs/core/config.schema.json`, and include that diff in the same commit (AGENTS.md + this plan's Task 2).
- Do not add new standalone helper methods referenced only once (AGENTS.md).
- Make new `match` statements exhaustive; avoid wildcard arms (AGENTS.md).
- `think` defaults to suppressing reasoning (`false`) when unconfigured — that's the entire reason this feature exists (spec, Resolved Decision 3).
- Tool-calling is in scope for v1 (spec, Resolved Decision 1) — do not defer it to a follow-up plan.

Spec: `docs/superpowers/specs/2026-07-01-native-ollama-backend-design.md`. Read it before starting Task 1 — this plan assumes its Resolved Decisions section as settled.

---

### Task 1: `WireApi::OllamaNative` variant

**Files:**
- Modify: `codex-rs/model-provider-info/src/lib.rs:58-98` (the `WireApi` enum + its `Display`/`Deserialize` impls)
- Test: `codex-rs/model-provider-info/src/lib.rs` (inline `#[cfg(test)]` module — check if one exists near the bottom of the file first; if not, add one)

**Interfaces:**
- Produces: `WireApi::OllamaNative` variant, usable by all later tasks that match on `WireApi`.

- [ ] **Step 1: Write the failing test**

Add to the test module in `codex-rs/model-provider-info/src/lib.rs` (create a `#[cfg(test)] mod tests { use super::*; ... }` block near the end of the file if one doesn't already exist there):

```rust
#[test]
fn ollama_native_wire_api_display_and_serde_roundtrip() {
    assert_eq!(WireApi::OllamaNative.to_string(), "ollama_native");

    let serialized = serde_json::to_string(&WireApi::OllamaNative).unwrap();
    assert_eq!(serialized, "\"ollama_native\"");

    let deserialized: WireApi = serde_json::from_str("\"ollama_native\"").unwrap();
    assert_eq!(deserialized, WireApi::OllamaNative);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `just test -p codex-model-provider-info` (from `codex-rs/`)
Expected: FAIL — compile error, `no variant named OllamaNative found for enum WireApi`.

- [ ] **Step 3: Add the variant and update both impls**

In `codex-rs/model-provider-info/src/lib.rs`, replace the existing enum and impls (lines 58-98) with:

```rust
/// Wire protocol that the provider speaks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WireApi {
    /// The Responses API exposed by OpenAI at `/v1/responses`.
    #[default]
    Responses,
    /// OpenAI-compatible Chat Completions exposed at `/v1/chat/completions`.
    Chat,
    /// Anthropic Messages exposed at `/v1/messages`.
    Messages,
    /// Ollama's native chat API at `/api/chat` — NOT the OpenAI-compat surface.
    /// Only meaningful for Ollama-family providers; carries `think` support.
    OllamaNative,
}

impl fmt::Display for WireApi {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Responses => "responses",
            Self::Chat => "chat",
            Self::Messages => "messages",
            Self::OllamaNative => "ollama_native",
        };
        f.write_str(value)
    }
}

impl<'de> Deserialize<'de> for WireApi {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        match value.as_str() {
            "responses" => Ok(Self::Responses),
            "chat" => Ok(Self::Chat),
            "messages" => Ok(Self::Messages),
            "ollama_native" => Ok(Self::OllamaNative),
            _ => Err(serde::de::Error::unknown_variant(
                &value,
                &["responses", "chat", "messages", "ollama_native"],
            )),
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `just test -p codex-model-provider-info`
Expected: PASS. Also expect NEW compile errors elsewhere in the workspace — every existing `match wire_api { ... }` that was exhaustive over 3 variants now fails to compile because it isn't exhaustive over 4. That's expected and exactly what AGENTS.md's "make matches exhaustive" rule is for — **do not add a wildcard arm to silence these**; each one needs a real decision, made in Tasks 4 and 9 below. Note which files break here (run `cargo build --workspace 2>&1 | grep "non-exhaustive"` for a full list) so Task 4/9 don't miss one.

- [ ] **Step 5: Commit**

```bash
cd codex-rs
just fmt
git add model-provider-info/src/lib.rs
git commit -m "feat(model-provider-info): add WireApi::OllamaNative variant"
```

---

### Task 2: `ModelProviderInfo.ollama_think` field + config schema

**Files:**
- Modify: `codex-rs/model-provider-info/src/lib.rs:103-145` (the `ModelProviderInfo` struct)
- Modify: `codex-rs/core/config.schema.json` (regenerated, not hand-edited)
- Test: same test module as Task 1

**Interfaces:**
- Consumes: `WireApi::OllamaNative` (Task 1).
- Produces: `ModelProviderInfo.ollama_think: Option<String>`, read by Task 5's `resolve_think()`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn model_provider_info_deserializes_ollama_think_override_table() {
    let toml_str = r#"
        name = "ollama-native"
        wire_api = "ollama_native"
        ollama_think = "gpt-oss:low,qwen3:false"
    "#;
    let provider: ModelProviderInfo = toml::from_str(toml_str).unwrap();
    assert_eq!(
        provider.ollama_think.as_deref(),
        Some("gpt-oss:low,qwen3:false")
    );
}

#[test]
fn model_provider_info_ollama_think_defaults_to_none() {
    let provider = ModelProviderInfo {
        name: "ollama-native".to_string(),
        wire_api: WireApi::OllamaNative,
        ..Default::default()
    };
    assert_eq!(provider.ollama_think, None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `just test -p codex-model-provider-info`
Expected: FAIL — `no field \`ollama_think\` on type \`ModelProviderInfo\``.

- [ ] **Step 3: Add the field**

In `codex-rs/model-provider-info/src/lib.rs`, inside the `ModelProviderInfo` struct definition (after the existing `wire_api` field, around line 125), add:

```rust
    /// Reasoning-suppression override table for the OllamaNative wire, mirroring elf-dispatch's
    /// ollama-backend.js resolveThink(): comma-separated "substr:value" pairs, checked against the
    /// model id in order, first match wins. value is "false" (think:false, the default behavior
    /// when this field is unset), "true", or an effort string ("low"/"medium"/"high") for models
    /// like gpt-oss that ignore a bare think:false and need an effort level instead. Ignored for
    /// any wire_api other than OllamaNative.
    pub ollama_think: Option<String>,
```

Since `ModelProviderInfo` derives `Default`, `Option<String>` defaults to `None` with no further change needed.

- [ ] **Step 4: Run test to verify it passes**

Run: `just test -p codex-model-provider-info`
Expected: PASS.

Then regenerate the config schema (AGENTS.md requirement whenever `ModelProviderInfo` changes):

Run: `just write-config-schema` (from `codex-rs/`)
Expected: `codex-rs/core/config.schema.json` is modified to include the new `ollama_think` property under the provider schema. Diff it to confirm the new field appears and nothing else unexpectedly changed.

- [ ] **Step 5: Commit**

```bash
cd codex-rs
just fmt
git add model-provider-info/src/lib.rs core/config.schema.json
git commit -m "feat(model-provider-info): add ollama_think override-table field"
```

---

### Task 3: `ollama-native` provider registration

**Files:**
- Modify: `codex-rs/model-provider-info/src/lib.rs:508-541` (`built_in_model_providers()` and its constants)
- Test: same test module

**Interfaces:**
- Consumes: `WireApi::OllamaNative` (Task 1), `create_oss_provider(port, wire_api)` (existing, unchanged signature).
- Produces: `OLLAMA_NATIVE_PROVIDER_ID: &str = "ollama-native"`, present as a key in `built_in_model_providers()`'s returned map — consumed by Task 9's CLI wiring.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn built_in_providers_include_ollama_native() {
    let providers = built_in_model_providers(None);
    let ollama_native = providers
        .get(OLLAMA_NATIVE_PROVIDER_ID)
        .expect("ollama-native provider must be registered");
    assert_eq!(ollama_native.wire_api, WireApi::OllamaNative);
    assert_eq!(
        ollama_native.base_url.as_deref(),
        Some("http://localhost:11434/v1")
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `just test -p codex-model-provider-info`
Expected: FAIL — `cannot find value \`OLLAMA_NATIVE_PROVIDER_ID\` in this scope`.

- [ ] **Step 3: Add the constant and registration**

In `codex-rs/model-provider-info/src/lib.rs`, after the existing `OLLAMA_OSS_PROVIDER_ID` constant (line 509), add:

```rust
pub const OLLAMA_NATIVE_PROVIDER_ID: &str = "ollama-native";
```

In `built_in_model_providers()`, inside the array literal that currently registers `OLLAMA_OSS_PROVIDER_ID` and `LMSTUDIO_OSS_PROVIDER_ID` (around line 525-531), add a third entry:

```rust
        (
            OLLAMA_NATIVE_PROVIDER_ID,
            create_oss_provider(DEFAULT_OLLAMA_PORT, WireApi::OllamaNative),
        ),
```

so the full array reads:

```rust
    let mut providers: HashMap<String, ModelProviderInfo> = [
        (
            OPENAI_PROVIDER_ID,
            P::create_openai_provider(openai_base_url),
        ),
        (
            AMAZON_BEDROCK_PROVIDER_ID,
            P::create_amazon_bedrock_provider(/*aws*/ None),
        ),
        (
            OLLAMA_OSS_PROVIDER_ID,
            create_oss_provider(DEFAULT_OLLAMA_PORT, WireApi::Responses),
        ),
        (
            OLLAMA_NATIVE_PROVIDER_ID,
            create_oss_provider(DEFAULT_OLLAMA_PORT, WireApi::OllamaNative),
        ),
        (
            LMSTUDIO_OSS_PROVIDER_ID,
            create_oss_provider(DEFAULT_LMSTUDIO_PORT, WireApi::Responses),
        ),
    ]
```

Note `DEFAULT_OLLAMA_PORT`/`CODEX_OSS_BASE_URL` env-override behavior is already generic in `create_oss_provider` (verified in the spec) — no changes needed there.

- [ ] **Step 4: Run test to verify it passes**

Run: `just test -p codex-model-provider-info`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cd codex-rs
just fmt
git add model-provider-info/src/lib.rs
git commit -m "feat(model-provider-info): register ollama-native built-in provider"
```

---

### Task 4: `StreamTransportRoute::OllamaNativeChat` + routing resolution

**Files:**
- Modify: `codex-rs/core/src/harness/routing.rs:35-90` (the `StreamTransportRoute` enum and `resolve_stream_transport_route()`)
- Test: `codex-rs/core/src/harness/routing.rs` — an inline test module already exists here (the spec's source reading found tests referencing `resolve_stream_transport_route` around line 188+); add to it

**Interfaces:**
- Consumes: `WireApi::OllamaNative` (Task 1).
- Produces: `StreamTransportRoute::OllamaNativeChat`, matched by Task 9's `client.rs` dispatch.

- [ ] **Step 1: Write the failing test**

Add near the existing tests in `codex-rs/core/src/harness/routing.rs` (the file already has tests like the ones at lines 188, 227, 236 referenced in the spec — add this alongside them, matching their exact style: call `resolve_stream_transport_route` directly and assert on the `Ok(...)` variant):

```rust
#[test]
fn ollama_native_wire_api_routes_bare_regardless_of_harness() {
    for harness in [Harness::Native, Harness::ClaudeCode, Harness::QwenCode] {
        let route = resolve_stream_transport_route(WireApi::OllamaNative, &harness)
            .expect("OllamaNative must resolve for every harness in v1 (spec Resolved Decision 4)");
        assert_eq!(route, StreamTransportRoute::OllamaNativeChat);
    }
}
```

(If `Harness::QwenCode` isn't the exact variant name, check `codex-rs/tools/src/*.rs` — wherever `Harness` is defined — for the real variant names before writing this; `ChatHarnessRoute` in this same file lists `KimiCli`, `KimiCode`, `QwenCode`, etc. as route names, which should correspond 1:1 to `Harness` enum variants of similar names.)

- [ ] **Step 2: Run test to verify it fails**

Run: `just test -p codex-core -- routing` (scopes to routing-module tests within the (large) `codex-core` crate)
Expected: FAIL — `no variant named OllamaNativeChat found for enum StreamTransportRoute`.

- [ ] **Step 3: Add the variant and match arm**

In `codex-rs/core/src/harness/routing.rs`, add to `StreamTransportRoute` (after the existing `ClaudeCodeChat` variant, around line 43):

```rust
    /// Ollama's native /api/chat, carrying `think` — no harness-specific shaping in v1
    /// (spec: docs/superpowers/specs/2026-07-01-native-ollama-backend-design.md, Resolved Decision 4).
    OllamaNativeChat,
```

In `resolve_stream_transport_route()`, add a match arm. Place it as its own arm (matching how `(WireApi::Responses, _) => ...` at line 63 is a catch-all for that variant) — add immediately after the last `(WireApi::Messages, ...)` arm in the existing match, before the function's closing brace:

```rust
        (WireApi::OllamaNative, _) => Ok(StreamTransportRoute::OllamaNativeChat),
```

This makes the match exhaustive again for the 4th `WireApi` variant (Task 1 caused this and every other `match wire_api` in the workspace to stop compiling — this is the fix for the one in this file specifically).

- [ ] **Step 4: Run test to verify it passes**

Run: `just test -p codex-core -- routing`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cd codex-rs
just fmt
git add core/src/harness/routing.rs
git commit -m "feat(core): route WireApi::OllamaNative to StreamTransportRoute::OllamaNativeChat"
```

---

### Task 5: `resolve_think()` — per-model reasoning-override parser

**Files:**
- Create: `codex-rs/ollama/src/think.rs`
- Modify: `codex-rs/ollama/src/lib.rs` (expose the new module)
- Test: inline `#[cfg(test)] mod tests` in `think.rs`

**Interfaces:**
- Consumes: nothing new (pure function over `&str` inputs).
- Produces: `pub fn resolve_think(model: &str, override_table: Option<&str>) -> ThinkValue`, consumed by Task 6's request-builder.
- Produces: `pub enum ThinkValue { Bool(bool), Effort(String) }` — mirrors that Ollama's `think` field accepts either a bool or an effort string like `"low"`.

- [ ] **Step 1: Write the failing test**

```rust
use codex_ollama::think::{resolve_think, ThinkValue};

#[test]
fn resolve_think_defaults_to_false_with_no_override_table() {
    assert_eq!(resolve_think("gemma4:26b-a4b-it-qat", None), ThinkValue::Bool(false));
}

#[test]
fn resolve_think_matches_substring_and_returns_effort_string() {
    let table = "gpt-oss:low,qwen3:false";
    assert_eq!(resolve_think("gpt-oss:120b", Some(table)), ThinkValue::Effort("low".to_string()));
}

#[test]
fn resolve_think_explicit_false_string_forces_bool_false() {
    let table = "gpt-oss:low,qwen3:false";
    assert_eq!(resolve_think("qwen3:8b", Some(table)), ThinkValue::Bool(false));
}

#[test]
fn resolve_think_no_match_in_table_falls_through_to_default_false() {
    let table = "gpt-oss:low";
    assert_eq!(resolve_think("gemma4:26b-a4b-it-qat", Some(table)), ThinkValue::Bool(false));
}

#[test]
fn resolve_think_explicit_true_string() {
    let table = "some-model:true";
    assert_eq!(resolve_think("some-model:latest", Some(table)), ThinkValue::Bool(true));
}
```

Place these as a separate test file `codex-rs/ollama/src/think_tests.rs` if `think.rs` itself would exceed a comfortable size with tests inline (mirroring this crate's existing `line_buffer.rs` / `line_buffer_tests.rs` split) — reference it from `think.rs` with `#[cfg(test)] mod think_tests;` at the bottom, matching the existing `line_buffer.rs` pattern exactly (check `codex-rs/ollama/src/lib.rs` for how `line_buffer_tests` is declared and mirror it).

- [ ] **Step 2: Run test to verify it fails**

Run: `just test -p codex-ollama`
Expected: FAIL — `could not find \`think\` in \`codex_ollama\``.

- [ ] **Step 3: Write the implementation**

Create `codex-rs/ollama/src/think.rs`:

```rust
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
mod think_tests;
```

Create `codex-rs/ollama/src/think_tests.rs` with the 5 tests from Step 1 (using `use super::*;` instead of `use codex_ollama::think::{...}`).

In `codex-rs/ollama/src/lib.rs`, add `pub mod think;` alongside the other existing `pub mod`/`mod` declarations (check the file first for the exact existing declarations for `line_buffer`, `client`, `pull`, `parser`, and mirror whichever ones are `pub` vs private — `think` needs to be `pub mod` since Task 6 in another module, and eventually `codex-core`, needs to call `resolve_think` from outside this crate).

- [ ] **Step 4: Run test to verify it passes**

Run: `just test -p codex-ollama`
Expected: PASS, all 5 tests green.

- [ ] **Step 5: Commit**

```bash
cd codex-rs
just fmt
git add ollama/src/think.rs ollama/src/think_tests.rs ollama/src/lib.rs
git commit -m "feat(ollama): add resolve_think() per-model reasoning override parser"
```

---

### Task 6: Native chat request/response types

**Files:**
- Create: `codex-rs/ollama/src/chat_types.rs`
- Modify: `codex-rs/ollama/src/lib.rs` (expose the module)
- Test: inline in `chat_types.rs`, using real captured JSON fixtures

**Interfaces:**
- Consumes: `think::ThinkValue` (Task 5) — reused directly as `ChatRequest.think`'s type, not
  redefined here (Task 5's `ThinkValue` already implements the exact bool-or-string `Serialize`
  this field needs).
- Produces: `ChatRequest`, `ChatMessage`, `ChatToolCall`, `ChatResponseChunk` structs — `serde_json`-(de)serializable, consumed by Task 7's streaming function and Task 8's event translation.

- [ ] **Step 1: Write the failing test**

Create `codex-rs/ollama/src/chat_types.rs` with just the test module first (empty structs to be filled in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::think::ThinkValue;

    // Captured live from Rikudo's Ollama (gemma4:26b-a4b-it-qat), stream:false, 2026-07-01 —
    // see docs/superpowers/specs/2026-07-01-native-ollama-backend-design.md §5.
    const NON_STREAMING_CHAT_RESPONSE: &str = r#"{
        "model": "gemma4:26b-a4b-it-qat",
        "created_at": "2026-07-01T11:22:52.304048354Z",
        "message": {
            "role": "assistant",
            "content": "",
            "tool_calls": [
                {
                    "id": "call_og2056ag",
                    "function": {
                        "index": 0,
                        "name": "get_weather",
                        "arguments": {"city": "Paris"}
                    }
                }
            ]
        },
        "done": true,
        "done_reason": "stop",
        "total_duration": 1757438142,
        "load_duration": 328879050,
        "prompt_eval_count": 69,
        "prompt_eval_duration": 777192000,
        "eval_count": 15,
        "eval_duration": 560723000
    }"#;

    // Captured live, stream:true, mid-stream content chunk (done:false, no tool_calls).
    const STREAMING_CONTENT_CHUNK: &str = r#"{
        "model": "gemma4:26b-a4b-it-qat",
        "created_at": "2026-07-01T11:23:11.856682501Z",
        "message": {"role": "assistant", "content": "Hello"},
        "done": false
    }"#;

    // Captured live, stream:true, final chunk (done:true, no message content, usage stats present).
    const STREAMING_FINAL_CHUNK: &str = r#"{
        "model": "gemma4:26b-a4b-it-qat",
        "created_at": "2026-07-01T11:23:11.896534616Z",
        "message": {"role": "assistant", "content": ""},
        "done": true,
        "done_reason": "stop",
        "total_duration": 1012876548,
        "load_duration": 310477301,
        "prompt_eval_count": 69,
        "prompt_eval_duration": 142060000,
        "eval_count": 15,
        "eval_duration": 558562000
    }"#;

    // Captured live, think:true, stream:false, gemma4:26b-a4b-it-qat asked to reason through
    // "What is 15 * 23?" — confirms `message.thinking` is a real, separate field alongside
    // `message.content` (resolves the spec's flagged uncertainty about this field's existence).
    // Truncated here to the parts the test actually checks; the field is a much longer string live.
    const RESPONSE_WITH_THINKING: &str = r#"{
        "model": "gemma4:26b-a4b-it-qat",
        "created_at": "2026-07-01T11:33:59.695247273Z",
        "message": {
            "role": "assistant",
            "content": "15 x 23 = 345",
            "thinking": "The user wants the product of 15 and 23. Method: distributive property."
        },
        "done": true,
        "done_reason": "stop",
        "prompt_eval_count": 30,
        "eval_count": 778
    }"#;

    #[test]
    fn parses_non_streaming_response_with_tool_call() {
        let chunk: ChatResponseChunk = serde_json::from_str(NON_STREAMING_CHAT_RESPONSE).unwrap();
        assert!(chunk.done);
        assert_eq!(chunk.done_reason.as_deref(), Some("stop"));
        assert_eq!(chunk.eval_count, Some(15));
        assert_eq!(chunk.prompt_eval_count, Some(69));
        let tool_calls = chunk.message.tool_calls.expect("tool_calls must parse");
        assert_eq!(tool_calls.len(), 1);
        assert_eq!(tool_calls[0].id, "call_og2056ag");
        assert_eq!(tool_calls[0].function.name, "get_weather");
        assert_eq!(
            tool_calls[0].function.arguments.get("city").and_then(|v| v.as_str()),
            Some("Paris")
        );
    }

    #[test]
    fn parses_streaming_content_chunk() {
        let chunk: ChatResponseChunk = serde_json::from_str(STREAMING_CONTENT_CHUNK).unwrap();
        assert!(!chunk.done);
        assert_eq!(chunk.message.content.as_deref(), Some("Hello"));
        assert!(chunk.message.tool_calls.is_none());
    }

    #[test]
    fn parses_response_with_thinking_field_separate_from_content() {
        let chunk: ChatResponseChunk = serde_json::from_str(RESPONSE_WITH_THINKING).unwrap();
        assert_eq!(chunk.message.content.as_deref(), Some("15 x 23 = 345"));
        assert!(chunk.message.thinking.as_deref().unwrap().contains("distributive property"));
    }

    #[test]
    fn parses_streaming_final_chunk_with_usage() {
        let chunk: ChatResponseChunk = serde_json::from_str(STREAMING_FINAL_CHUNK).unwrap();
        assert!(chunk.done);
        assert_eq!(chunk.eval_count, Some(15));
        assert_eq!(chunk.prompt_eval_count, Some(69));
    }

    #[test]
    fn chat_request_serializes_with_think_bool() {
        let req = ChatRequest {
            model: "gemma4:26b-a4b-it-qat".to_string(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: Some("hello".to_string()),
                thinking: None,
                tool_calls: None,
            }],
            think: ThinkValue::Bool(false),
            stream: true,
            tools: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["think"], serde_json::json!(false));
        assert_eq!(json["stream"], serde_json::json!(true));
    }

    #[test]
    fn chat_request_serializes_with_think_effort_string() {
        let req = ChatRequest {
            model: "gpt-oss:120b".to_string(),
            messages: vec![],
            think: ThinkValue::Effort("low".to_string()),
            stream: true,
            tools: None,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["think"], serde_json::json!("low"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `just test -p codex-ollama`
Expected: FAIL — compile errors, `ChatResponseChunk`/`ChatRequest`/etc. don't exist yet.

- [ ] **Step 3: Write the implementation**

Add above the test module in `codex-rs/ollama/src/chat_types.rs`:

```rust
//! Request/response types for Ollama's native `/api/chat` endpoint (NOT the OpenAI-compat
//! `/v1/chat/completions` surface). Shapes verified against a live Ollama instance, 2026-07-01 —
//! see docs/superpowers/specs/2026-07-01-native-ollama-backend-design.md §5 for the captured
//! fixtures these types are built from.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::think::ThinkValue;

#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub think: ThinkValue,
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// Reasoning trace, separate from `content` — confirmed real via a live capture (a
    /// think:true request against gemma4:26b-a4b-it-qat): the model's `<think>`-equivalent
    /// output arrives here, not mixed into `content`. Absent on non-reasoning responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ChatToolCall>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatToolCall {
    pub id: String,
    pub function: ChatToolCallFunction,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatToolCallFunction {
    pub index: u32,
    pub name: String,
    pub arguments: Value,
}

/// One line of Ollama's native streamed (or non-streamed, when `stream:false`) chat response.
#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponseChunk {
    pub model: String,
    pub message: ChatMessage,
    pub done: bool,
    #[serde(default)]
    pub done_reason: Option<String>,
    #[serde(default)]
    pub eval_count: Option<u64>,
    #[serde(default)]
    pub eval_duration: Option<u64>,
    #[serde(default)]
    pub prompt_eval_count: Option<u64>,
    #[serde(default)]
    pub prompt_eval_duration: Option<u64>,
}
```

In `codex-rs/ollama/src/lib.rs`, add `pub mod chat_types;`.

- [ ] **Step 4: Run test to verify it passes**

Run: `just test -p codex-ollama`
Expected: PASS, all 5 tests green.

- [ ] **Step 5: Commit**

```bash
cd codex-rs
just fmt
git add ollama/src/chat_types.rs ollama/src/lib.rs
git commit -m "feat(ollama): add native /api/chat request/response types"
```

---

### Task 7: NDJSON chat-streaming function

**Files:**
- Create: `codex-rs/ollama/src/chat_stream.rs`
- Modify: `codex-rs/ollama/src/lib.rs` (expose the module)
- Test: inline, `wiremock`-based, mirroring the existing pattern in `codex-rs/ollama/src/client.rs`'s `test_probe_server_happy_path_openai_compat_and_native`

**Interfaces:**
- Consumes: `ChatRequest`/`ChatResponseChunk` (Task 6), the existing `line_buffer` module's NDJSON line-splitting (read `codex-rs/ollama/src/line_buffer.rs` in full before writing this task's implementation — it's 32 lines, confirm its exact public function signature rather than guessing it here).
- Produces: `pub async fn chat_stream(host_root: &str, request: &ChatRequest) -> io::Result<impl Stream<Item = io::Result<ChatResponseChunk>>>`, consumed by Task 8/9.

- [ ] **Step 1: Read the existing NDJSON infrastructure**

Before writing any code, read `codex-rs/ollama/src/line_buffer.rs` in full (`cat codex-rs/ollama/src/line_buffer.rs`) and its test file `line_buffer_tests.rs`, and read `codex-rs/ollama/src/client.rs`'s `pull_model_stream` method (around line 157) end to end — that method already does exactly the network-request-plus-NDJSON-streaming pattern this task needs, just for `/api/pull` instead of `/api/chat`. Confirm the exact function/type names `line_buffer.rs` exports before using them below; if they differ from what's assumed here, use the real names instead.

- [ ] **Step 2: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::think::ThinkValue;
    use futures::StreamExt;

    #[tokio::test]
    async fn chat_stream_yields_chunks_in_order_and_completes() {
        let server = wiremock::MockServer::start().await;
        let ndjson_body = concat!(
            r#"{"model":"m","message":{"role":"assistant","content":"Hel"},"done":false}"#, "\n",
            r#"{"model":"m","message":{"role":"assistant","content":"lo"},"done":false}"#, "\n",
            r#"{"model":"m","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","eval_count":2,"prompt_eval_count":5}"#, "\n",
        );
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .and(wiremock::matchers::path("/api/chat"))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_raw(ndjson_body, "application/x-ndjson"),
            )
            .mount(&server)
            .await;

        let request = ChatRequest {
            model: "m".to_string(),
            messages: vec![ChatMessage { role: "user".to_string(), content: Some("hi".to_string()), thinking: None, tool_calls: None }],
            think: ThinkValue::Bool(false),
            stream: true,
            tools: None,
        };

        let mut stream = chat_stream(&server.uri(), &request).await.unwrap();
        let first = stream.next().await.unwrap().unwrap();
        assert_eq!(first.message.content.as_deref(), Some("Hel"));
        let second = stream.next().await.unwrap().unwrap();
        assert_eq!(second.message.content.as_deref(), Some("lo"));
        let third = stream.next().await.unwrap().unwrap();
        assert!(third.done);
        assert_eq!(third.eval_count, Some(2));
        assert!(stream.next().await.is_none());
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `just test -p codex-ollama`
Expected: FAIL — `chat_stream` doesn't exist.

- [ ] **Step 4: Write the implementation**

Write `codex-rs/ollama/src/chat_stream.rs` following the exact structure of `client.rs`'s `pull_model_stream` (read in Step 1) — reuse whatever NDJSON-line-splitting helper that method calls from `line_buffer.rs` rather than reimplementing it; POST to `{host_root}/api/chat` with the serialized `ChatRequest` as the JSON body, and for each complete line yielded by the line-buffer splitter, `serde_json::from_str::<ChatResponseChunk>` it and yield the result through an `async_stream::stream!` (this crate already depends on `async-stream`, per its `Cargo.toml`). Match `pull_model_stream`'s error-handling shape (how it turns a non-2xx response or a JSON-parse failure into an `io::Error`) rather than inventing a different error convention in this new function.

Add `pub mod chat_stream;` to `codex-rs/ollama/src/lib.rs`.

- [ ] **Step 5: Run test to verify it passes**

Run: `just test -p codex-ollama`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cd codex-rs
just fmt
git add ollama/src/chat_stream.rs ollama/src/lib.rs
git commit -m "feat(ollama): add chat_stream() NDJSON streaming for /api/chat"
```

---

### Task 8: Chunk → `ResponseEvent` translation

**Files:**
- Create: `codex-rs/ollama/src/chat_events.rs`
- Modify: `codex-rs/ollama/src/lib.rs` (expose the module)
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Consumes: `ChatResponseChunk` (Task 6), `codex_api::common::{ResponseEvent, TokenUsage}` (existing, `codex-ollama`'s `Cargo.toml` needs a `codex-api` dependency added if not already present — check `codex-rs/ollama/Cargo.toml`'s current `[dependencies]` first, since the version read during spec research did not list `codex-api` there).
- Produces: `pub fn chat_chunk_to_events(chunk: ChatResponseChunk) -> Vec<ResponseEvent>`, consumed by Task 9.

- [ ] **Step 1: Confirm the `codex-api` dependency**

Read `codex-rs/ollama/Cargo.toml`'s current `[dependencies]` block. If `codex-api = { workspace = true }` is not already listed, add it (matching the style of the existing `codex-core = { workspace = true }` line) before writing any code that imports from `codex_api`.

- [ ] **Step 2: Write the failing test**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use codex_ollama::chat_types::{ChatMessage, ChatResponseChunk, ChatToolCall, ChatToolCallFunction};
    use codex_api::common::ResponseEvent;

    #[test]
    fn content_chunk_becomes_output_text_delta() {
        let chunk = ChatResponseChunk {
            model: "m".to_string(),
            message: ChatMessage { role: "assistant".to_string(), content: Some("Hi".to_string()), thinking: None, tool_calls: None },
            done: false,
            done_reason: None,
            eval_count: None,
            eval_duration: None,
            prompt_eval_count: None,
            prompt_eval_duration: None,
        };
        let events = chat_chunk_to_events(chunk);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], ResponseEvent::OutputTextDelta(s) if s == "Hi"));
    }

    #[test]
    fn thinking_field_becomes_reasoning_content_delta_separate_from_text() {
        let chunk = ChatResponseChunk {
            model: "m".to_string(),
            message: ChatMessage {
                role: "assistant".to_string(),
                content: Some("345".to_string()),
                thinking: Some("15 * 23 = 345 via distributive property".to_string()),
                tool_calls: None,
            },
            done: false,
            done_reason: None,
            eval_count: None,
            eval_duration: None,
            prompt_eval_count: None,
            prompt_eval_duration: None,
        };
        let events = chat_chunk_to_events(chunk);
        assert_eq!(events.len(), 2, "expected both a reasoning delta and a text delta: {events:?}");
        assert!(events.iter().any(|e| matches!(e, ResponseEvent::ReasoningContentDelta { delta, .. } if delta.contains("distributive"))));
        assert!(events.iter().any(|e| matches!(e, ResponseEvent::OutputTextDelta(s) if s == "345")));
    }

    #[test]
    fn final_chunk_becomes_completed_with_token_usage() {
        let chunk = ChatResponseChunk {
            model: "m".to_string(),
            message: ChatMessage { role: "assistant".to_string(), content: Some("".to_string()), thinking: None, tool_calls: None },
            done: true,
            done_reason: Some("stop".to_string()),
            eval_count: Some(15),
            eval_duration: Some(560723000),
            prompt_eval_count: Some(69),
            prompt_eval_duration: Some(777192000),
        };
        let events = chat_chunk_to_events(chunk);
        let completed = events.iter().find(|e| matches!(e, ResponseEvent::Completed { .. }));
        assert!(completed.is_some(), "expected a Completed event in {events:?}");
        if let Some(ResponseEvent::Completed { end_turn, token_usage, .. }) = completed {
            assert_eq!(*end_turn, Some(true));
            let usage = token_usage.as_ref().expect("token_usage must be populated");
            assert_eq!(usage.output_tokens, 15);
            assert_eq!(usage.input_tokens, 69);
        }
    }

    #[test]
    fn tool_call_chunk_becomes_tool_call_input_delta() {
        let chunk = ChatResponseChunk {
            model: "m".to_string(),
            message: ChatMessage {
                role: "assistant".to_string(),
                content: Some("".to_string()),
                thinking: None,
                tool_calls: Some(vec![ChatToolCall {
                    id: "call_1".to_string(),
                    function: ChatToolCallFunction {
                        index: 0,
                        name: "get_weather".to_string(),
                        arguments: serde_json::json!({"city": "Paris"}),
                    },
                }]),
            },
            done: false,
            done_reason: None,
            eval_count: None,
            eval_duration: None,
            prompt_eval_count: None,
            prompt_eval_duration: None,
        };
        let events = chat_chunk_to_events(chunk);
        let tool_event = events.iter().find(|e| matches!(e, ResponseEvent::ToolCallInputDelta { .. }));
        assert!(tool_event.is_some(), "expected a ToolCallInputDelta in {events:?}");
        if let Some(ResponseEvent::ToolCallInputDelta { item_id, call_id, delta }) = tool_event {
            assert_eq!(item_id, "call_1");
            assert_eq!(call_id.as_deref(), Some("call_1"));
            assert!(delta.contains("Paris"));
        }
    }
}
```

- [ ] **Step 3: Run test to verify it fails**

Run: `just test -p codex-ollama`
Expected: FAIL — `chat_chunk_to_events` doesn't exist.

- [ ] **Step 4: Write the implementation**

```rust
//! Translates one native-Ollama chat chunk (chat_types::ChatResponseChunk) into zero or more
//! codex_api::common::ResponseEvent — the internal event type the rest of the agent loop
//! (TUI rendering, tool-call handling, token accounting) already consumes for every other wire.

use codex_api::common::{ResponseEvent, TokenUsage};

use crate::chat_types::ChatResponseChunk;

pub fn chat_chunk_to_events(chunk: ChatResponseChunk) -> Vec<ResponseEvent> {
    let mut events = Vec::new();

    // Reasoning trace, confirmed a genuinely separate field from `content` (live capture against
    // gemma4:26b-a4b-it-qat, think:true — see chat_types.rs's RESPONSE_WITH_THINKING fixture).
    // content_index: 0 — Ollama's native API has no multi-block reasoning index of its own; a
    // single running index is correct until a real multi-segment case is observed.
    if let Some(thinking) = chunk.message.thinking.as_deref() {
        if !thinking.is_empty() {
            events.push(ResponseEvent::ReasoningContentDelta {
                delta: thinking.to_string(),
                content_index: 0,
            });
        }
    }

    if let Some(content) = chunk.message.content.as_deref() {
        if !content.is_empty() {
            events.push(ResponseEvent::OutputTextDelta(content.to_string()));
        }
    }

    if let Some(tool_calls) = &chunk.message.tool_calls {
        for call in tool_calls {
            // Ollama sends the entire tool call in one chunk (confirmed empirically, both
            // stream:false and stream:true — see spec §5), never incremental argument deltas
            // the way OpenAI's streaming tool calls work. Emit it as a single complete delta.
            let delta = serde_json::to_string(&call.function.arguments)
                .unwrap_or_else(|_| "{}".to_string());
            events.push(ResponseEvent::ToolCallInputDelta {
                item_id: call.id.clone(),
                call_id: Some(call.id.clone()),
                delta,
            });
        }
    }

    if chunk.done {
        let token_usage = match (chunk.prompt_eval_count, chunk.eval_count) {
            (Some(input), Some(output)) => Some(TokenUsage {
                input_tokens: input as i64,
                cached_input_tokens: 0,
                output_tokens: output as i64,
                reasoning_output_tokens: 0,
                total_tokens: (input + output) as i64,
            }),
            _ => None,
        };
        events.push(ResponseEvent::Completed {
            response_id: String::new(),
            token_usage,
            end_turn: chunk.done_reason.as_deref().map(|r| r == "stop"),
        });
    }

    events
}
```

Add `pub mod chat_events;` to `codex-rs/ollama/src/lib.rs`.

- [ ] **Step 5: Run test to verify it passes**

Run: `just test -p codex-ollama`
Expected: PASS, all 3 tests green. If `ResponseEvent::Completed`'s actual field names differ from `response_id`/`token_usage`/`end_turn` (re-verify against `codex-rs/codex-api/src/common.rs` if this fails to compile for a field-name mismatch — the spec captured this from a partial read), fix the struct literal to match the real field names rather than guessing further.

- [ ] **Step 6: Commit**

```bash
cd codex-rs
just fmt
git add ollama/src/chat_events.rs ollama/src/lib.rs ollama/Cargo.toml
git commit -m "feat(ollama): translate native chat chunks to ResponseEvent"
```

---

### Task 9: Wire into `client.rs`'s `stream()` dispatch + CLI surface

**Files:**
- Modify: `codex-rs/core/src/client.rs` (add a `stream_ollama_native_chat_api` method + a new match arm in `stream()`, around line 2339)
- Modify: `codex-rs/exec/src/lib.rs:392` (the `--local-provider` valid-values error message)
- Test: integration test under `codex-rs/core/suite` (per `AGENTS.md`'s stated preference for integration tests over unit tests for agent-visible behavior), using `test_codex` per that guidance — read `codex-rs/core/suite`'s existing tests first for the exact `test_codex` setup pattern before writing this one, since this plan has not read that harness in full

**Interfaces:**
- Consumes: `StreamTransportRoute::OllamaNativeChat` (Task 4), `chat_stream()` (Task 7), `chat_chunk_to_events()` (Task 8), `resolve_think()` (Task 5).
- Produces: a working `codex exec --oss --local-provider ollama-native -m <model> "<prompt>"` end-to-end path.

- [ ] **Step 1: Read the existing chat-completions-compat implementation in full**

This is the trickiest integration seam in the whole plan and the one piece this plan has not fully read source for — do not guess its body. Run:

```bash
sed -n '1428,1560p' codex-rs/core/src/client.rs
```

(1560 is a guess at a reasonable upper bound for where the method ends — if it continues past that, keep reading until its closing brace.) Confirm: how it builds the outbound HTTP request (auth headers, retry/`PendingUnauthorizedRetry` handling — note whether Ollama-family requests need any of that auth machinery at all, since local Ollama has no API key), and how it wraps a raw event stream into the `Result<ResponseStream>` return type (check `ResponseStream`'s constructor in `codex-rs/core/src/client_common.rs:111`).

- [ ] **Step 2: Write the failing integration test**

Read `codex-rs/core/suite/*.rs` for the exact `test_codex` helper signature and an existing test that exercises a non-default `WireApi` end-to-end (there should be one for `ChatCompletionsCompat`, since that's the closest analog) — copy its structure, pointing the test's mock server/provider config at `wire_api: WireApi::OllamaNative` and a wiremock `/api/chat` mount serving the same NDJSON fixture style from Task 7's test, then assert the resulting turn's final message content matches what the mock returned. Do not invent this test's exact code here without first reading `test_codex`'s real signature — copy the closest existing WireApi-level integration test's structure exactly, substituting the wire/mock details for this new route.

- [ ] **Step 3: Run test to verify it fails**

Run: `just test -p codex-core -- ollama_native` (adjust the filter to match whatever test name Step 2 actually used)
Expected: FAIL — `StreamTransportRoute::OllamaNativeChat` has no handler in `stream()`'s match, or the test's provider config doesn't resolve as expected yet.

- [ ] **Step 4: Add the new method and match arm**

In `codex-rs/core/src/client.rs`, add a new method near `stream_chat_completions_compat` (matching its signature exactly, confirmed in Step 1):

```rust
    async fn stream_ollama_native_chat_api(
        &self,
        prompt: &Prompt,
        model_info: &ModelInfo,
        session_telemetry: &SessionTelemetry,
        effort: Option<ReasoningEffortConfig>,
        summary: ReasoningSummaryConfig,
        service_tier: Option<String>,
        responses_metadata: &CodexResponsesMetadata,
        inference_trace: &InferenceTraceContext,
    ) -> Result<ResponseStream> {
        // Implementer: build a codex_ollama::chat_types::ChatRequest from `prompt`'s messages
        // (check how stream_chat_completions_compat, read in Step 1, converts `prompt` into its
        // own OpenAI-chat-shaped messages array — mirror that conversion, not the OpenAI shape),
        // resolve `think` via codex_ollama::think::resolve_think(&model_info.model_id,
        // self.client.state.provider.ollama_think.as_deref()), call
        // codex_ollama::chat_stream::chat_stream(base_url, &request), and map each yielded
        // ChatResponseChunk through codex_ollama::chat_events::chat_chunk_to_events(), wrapping
        // the resulting event stream into a ResponseStream via whatever constructor Step 1 found
        // stream_chat_completions_compat uses for its own return value.
    }
```

(The body above is intentionally left as a structured comment describing exactly what to build, not a placeholder like "add error handling" — every referenced function/type is real and defined in an earlier task; only the glue connecting them depends on the exact `Prompt`→messages conversion and `ResponseStream` constructor this plan could not read in full ahead of time. Fill in the body using what Step 1 found before moving to Step 5.)

In `stream()`'s match (around line 2339), add:

```rust
            StreamTransportRoute::OllamaNativeChat => {
                self.stream_ollama_native_chat_api(
                    prompt,
                    model_info,
                    session_telemetry,
                    effort,
                    summary,
                    service_tier,
                    responses_metadata,
                    inference_trace,
                )
                .await
            }
```

- [ ] **Step 5: Run test to verify it passes**

Run: `just test -p codex-core -- ollama_native`
Expected: PASS.

- [ ] **Step 6: CLI surface**

In `codex-rs/exec/src/lib.rs:392`, update the error message:

```rust
                "No default OSS provider configured. Use --local-provider=provider or set oss_provider to one of: {LMSTUDIO_OSS_PROVIDER_ID}, {OLLAMA_OSS_PROVIDER_ID}, {OLLAMA_NATIVE_PROVIDER_ID} in config.toml"
```

adding `use codex_model_provider_info::OLLAMA_NATIVE_PROVIDER_ID;` near the existing imports of the other two provider-id constants (line 80-81). Then grep the rest of the workspace for any other hardcoded reference to `OLLAMA_OSS_PROVIDER_ID` that also needs the native id added:

```bash
grep -rn "OLLAMA_OSS_PROVIDER_ID" codex-rs --include="*.rs"
```

Add `OLLAMA_NATIVE_PROVIDER_ID` at every site this turns up beyond the one already fixed above.

- [ ] **Step 7: Run the full workspace test suite**

Per `AGENTS.md`: ask before running the complete suite. Once approved:

Run: `just test` (from `codex-rs/`)
Expected: PASS across the whole workspace — this confirms Task 1's "every match on WireApi must now handle 4 variants" requirement was actually satisfied everywhere, not just in the files this plan touched directly.

- [ ] **Step 8: Lint**

Run: `just fix -p codex-core` and `just fix -p codex-ollama` and `just fix -p codex-exec`
Expected: clean, or auto-fixed. Do not re-run tests after this per `AGENTS.md`.

- [ ] **Step 9: Commit**

```bash
cd codex-rs
just fmt
git add core/src/client.rs exec/src/lib.rs
git commit -m "feat(core): wire OllamaNativeChat route into stream() dispatch and CLI"
```

---

### Task 10: Live verification against Rikudo

**Files:** none (verification only, no code changes)

**Interfaces:** none — this task exercises everything built in Tasks 1-9 end to end against a real server.

- [ ] **Step 1: Build the release binary**

```bash
cd codex-rs
cargo build --release -p codex-cli
```

- [ ] **Step 2: Run against the real fleet**

```bash
CODEX_OSS_BASE_URL=http://100.65.215.78:11434/v1 ./target/release/codex exec \
  --oss --local-provider ollama-native -m gemma4:26b-a4b-it-qat \
  --sandbox read-only --skip-git-repo-check --json \
  "In one sentence, what does the file bin/dispatch.js do?"
```

Expected: the `--json` NDJSON event stream shows a real `agent_message` with correct content, **and no visible reasoning trace consuming the response** the way the original `/v1/responses`-based `ollama` provider's non-native path does — since the whole point of this feature is that `think:false` genuinely suppresses it this time. Compare against the spec's captured baseline (§ "Problem") if it's unclear which failure mode this is meant to fix.

- [ ] **Step 3: Confirm reasoning suppression concretely**

Run the same prompt against the OLD `ollama` provider id for comparison:

```bash
CODEX_OSS_BASE_URL=http://100.65.215.78:11434/v1 ./target/release/codex exec \
  --oss --local-provider ollama -m gemma4:26b-a4b-it-qat \
  --sandbox read-only --skip-git-repo-check --json \
  "In one sentence, what does the file bin/dispatch.js do?"
```

Both should produce a correct final answer for this simple prompt (gemma4 isn't gpt-oss, so it may not exhibit the worst-case reasoning-eats-the-budget failure on a short prompt) — the meaningful acceptance signal is whichever of the two shows a visible `thinking`/reasoning block in its JSON event stream vs. one that doesn't, or (better) rerun both against a prompt complex enough to make gemma4 reason at length, and confirm `ollama-native` suppresses it while the plain `ollama` (Responses API) provider does not.

- [ ] **Step 4: Update the spec status**

Edit `docs/superpowers/specs/2026-07-01-native-ollama-backend-design.md`'s header: change `**Status:** decisions resolved ... Not yet implemented.` to `**Status:** implemented and verified against Rikudo, <date>.`

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/specs/2026-07-01-native-ollama-backend-design.md
git commit -m "docs: mark native Ollama backend spec as implemented and verified"
```
