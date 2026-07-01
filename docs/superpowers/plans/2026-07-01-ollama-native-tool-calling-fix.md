# Fix `tools: None` on the Ollama native wire Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `stream_ollama_native_chat_api` send real tool definitions to Ollama's native `/api/chat` instead of hardcoding `tools: None`, so `--local-provider ollama-native` can drive real agentic turns.

**Architecture:** A new function, `ollama_tools_from_prompt`, converts a turn's `&[ToolSpec]` into the JSON shape Ollama's native API accepts (OpenAI Chat-Completions' nested `{"type":"function","function":{...}}` shape — live-verified against Rikudo), placed in `client.rs` next to the existing `ollama_messages_from_prompt`. Wired into the one place that currently hardcodes `tools: None`.

**Tech Stack:** Rust, existing `codex-tools`/`ToolSpec` types, `serde_json`.

## Global Constraints

- Spec doc: `docs/superpowers/specs/2026-07-01-ollama-native-tool-calling-fix-design.md` — source of truth for intent.
- Scope is request-construction only. Do not touch `codex-rs/ollama-wire/src/chat_events.rs` (response-parsing path) — it already works correctly once real tool_calls come back; this fix is about what gets *sent*, not what gets *parsed*.
- `ToolSpec::WebSearch`, `ToolSpec::ImageGeneration`, `ToolSpec::Namespace` are omitted from the sent `tools` array (no Ollama-native equivalent) — not an error, not a partial/broken shape.
- Do not modify `codex-rs/chat-wire-compat/src/request.rs`'s visibility or reuse its private `ShellToolCallParams`/`schema_value` cross-crate — duplicate an equivalent schema shape locally in `client.rs` instead (confirmed both are private, non-`pub`, in that crate; touching a sibling crate's visibility is disproportionate for this fix).
- Run `just test -p codex-core -- ollama_native` and `just test -p codex-core -- client` (or the exact filter matching wherever the new unit tests land) before calling any task done.

---

### Task 1: `ollama_tools_from_prompt` + wire it in + tests

**Files:**
- Modify: `codex-rs/core/src/client.rs` (add the function near `ollama_messages_from_prompt` at line 2675; change the `tools: None` field at the `OllamaChatRequest` construction inside `stream_ollama_native_chat_api`, currently reading `tools: None,`)
- Test: `codex-rs/core/tests/suite/ollama_native.rs` (already has two tests from the original native-ollama-backend plan — add a third)

**Interfaces:**
- Consumes: `codex_tools::ToolSpec` (already imported/available in `client.rs` — confirm the exact `use` path when implementing, it may need adding), `Prompt.tools: Vec<ToolSpec>` (`client_common.rs:25`, `pub(crate)` — already reachable from `client.rs` since both are in `codex-core`).
- Produces: `fn ollama_tools_from_prompt(prompt: &Prompt) -> Option<Vec<serde_json::Value>>` — used once, at the `OllamaChatRequest` construction site. No other task depends on this.

- [ ] **Step 1: Write the failing unit tests**

Add near the bottom of `codex-rs/core/src/client.rs`, in whatever `#[cfg(test)] mod tests` block already covers this file's other unit tests (search for an existing test module in this file first — if `ollama_messages_from_prompt` already has unit tests nearby, add these alongside them; otherwise add a new `#[cfg(test)] mod ollama_tools_tests` right after `ollama_tools_from_prompt`'s definition):

```rust
#[test]
fn ollama_tools_from_prompt_converts_function_tool() {
    let prompt = Prompt {
        tools: vec![ToolSpec::Function(ResponsesApiTool {
            name: "get_weather".to_string(),
            description: "Get the weather for a city".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::Object {
                properties: Default::default(),
                required: None,
                additional_properties: None,
            },
        })],
        ..Default::default()
    };

    let tools = ollama_tools_from_prompt(&prompt).expect("expected Some(tools)");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["function"]["name"], "get_weather");
    assert_eq!(tools[0]["function"]["description"], "Get the weather for a city");
    assert!(tools[0]["function"]["parameters"].is_object());
}

#[test]
fn ollama_tools_from_prompt_converts_local_shell_tool() {
    let prompt = Prompt {
        tools: vec![ToolSpec::LocalShell {}],
        ..Default::default()
    };

    let tools = ollama_tools_from_prompt(&prompt).expect("expected Some(tools)");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["type"], "function");
    assert_eq!(tools[0]["function"]["name"], "local_shell");
    assert!(tools[0]["function"]["parameters"]["properties"]["command"].is_object());
}

#[test]
fn ollama_tools_from_prompt_returns_none_for_no_tools() {
    let prompt = Prompt {
        tools: vec![],
        ..Default::default()
    };

    assert_eq!(ollama_tools_from_prompt(&prompt), None);
}

#[test]
fn ollama_tools_from_prompt_returns_none_when_only_unsupported_tools_present() {
    let prompt = Prompt {
        tools: vec![ToolSpec::WebSearch {
            external_web_access: None,
            filters: None,
            user_location: None,
            search_context_size: None,
            search_content_types: None,
        }],
        ..Default::default()
    };

    assert_eq!(ollama_tools_from_prompt(&prompt), None);
}
```

Check the real field names/shape of `Prompt`'s `Default` impl and `ResponsesApiTool`/`JsonSchema::Object` (search `codex-rs/tools/src/responses_api.rs` and wherever `JsonSchema` is defined) before finalizing this step — the sketch above is built from reading `tool_spec.rs`/`responses_api.rs` but the exact `JsonSchema` enum variant names/fields were not independently re-verified line-by-line for this plan; confirm them against real source and adjust the test's construction accordingly, don't guess if something doesn't compile.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `source "$HOME/.cargo/env"; cd codex-rs && just test -p codex-core -- ollama_tools_from_prompt`
Expected: compile failure (`ollama_tools_from_prompt` doesn't exist yet) — the correct RED state for adding a new function.

- [ ] **Step 3: Implement `ollama_tools_from_prompt`**

Add to `codex-rs/core/src/client.rs`, near `ollama_messages_from_prompt` (line 2675):

```rust
/// Converts a turn's available tools into the JSON shape Ollama's native `/api/chat` expects for
/// its `tools` field — OpenAI Chat-Completions' nested `{"type":"function","function":{...}}`
/// shape, confirmed by curl against a real Ollama server (`qwen3-coder:30b`, 2026-07-01: a request
/// built in exactly this shape got back a real, correctly-parsed `tool_calls` response). See
/// docs/superpowers/specs/2026-07-01-ollama-native-tool-calling-fix-design.md.
fn ollama_tools_from_prompt(prompt: &Prompt) -> Option<Vec<serde_json::Value>> {
    if prompt.tools.is_empty() {
        return None;
    }

    let tools: Vec<serde_json::Value> = prompt
        .tools
        .iter()
        .filter_map(|tool| match tool {
            ToolSpec::Function(responses_tool) => Some(serde_json::json!({
                "type": "function",
                "function": {
                    "name": responses_tool.name,
                    "description": responses_tool.description,
                    "parameters": responses_tool.parameters,
                }
            })),
            ToolSpec::LocalShell {} => Some(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "local_shell",
                    "description": "Run a shell command in the local environment",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "command": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "The command to execute, as a list of arguments"
                            }
                        },
                        "required": ["command"]
                    }
                }
            })),
            ToolSpec::Freeform(freeform) => Some(serde_json::json!({
                "type": "function",
                "function": {
                    "name": freeform.name,
                    "description": freeform.description,
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "input": { "type": "string" }
                        },
                        "required": ["input"],
                        "additionalProperties": false
                    }
                }
            })),
            ToolSpec::ToolSearch { description, parameters, .. } => Some(serde_json::json!({
                "type": "function",
                "function": {
                    "name": "tool_search",
                    "description": description,
                    "parameters": parameters,
                }
            })),
            ToolSpec::Namespace(_) | ToolSpec::WebSearch { .. } | ToolSpec::ImageGeneration { .. } => None,
        })
        .collect();

    if tools.is_empty() { None } else { Some(tools) }
}
```

Add whatever `use codex_tools::ToolSpec;` (or equivalent path) import is needed — check the exact real path by searching how `client.rs` already imports other `codex_tools` items (it already uses `create_tools_json_for_responses_api` from this crate elsewhere in the file, so the crate is already a dependency; just confirm the exact `ToolSpec` import path).

- [ ] **Step 4: Wire it into the request construction**

Change the `OllamaChatRequest` construction inside `stream_ollama_native_chat_api` (currently `tools: None,`) to:

```rust
            tools: ollama_tools_from_prompt(prompt),
```

- [ ] **Step 5: Run the unit tests to verify they pass**

Run: `source "$HOME/.cargo/env"; cd codex-rs && just test -p codex-core -- ollama_tools_from_prompt`
Expected: all 4 tests pass.

- [ ] **Step 6: Write the failing integration test — assert the outgoing request, not just the response**

This closes the exact gap that let the original bug ship unnoticed: the existing `ollama_native_chat_turn_handles_tool_call` test (in `codex-rs/core/tests/suite/ollama_native.rs`) only asserts the mocked *response* parses correctly; it never checks what the outgoing *request* contained. Add a new test to the same file that does:

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ollama_native_chat_request_includes_tool_definitions() {
    skip_if_no_network!();

    let server = MockServer::start().await;

    let ndjson_body = concat!(
        r#"{"model":"m","message":{"role":"assistant","content":"ok"},"done":false}"#,
        "\n",
        r#"{"model":"m","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","eval_count":2,"prompt_eval_count":5}"#,
        "\n",
    );

    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .and(wiremock::matchers::body_partial_json(serde_json::json!({
            "model": "m"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_raw(ndjson_body, "application/x-ndjson"))
        .expect(1)
        .mount(&server)
        .await;

    // capture the actual request body via wiremock's request recording, then assert it
    // (read wiremock's docs/existing usage in this crate for the exact recording API — the
    // `mcp_tool_call.rs`/other wiremock-based tests in this workspace may already show the
    // pattern for asserting on a captured request body; use that established pattern rather
    // than inventing a new one)

    // ... build provider/TestCodex exactly like the existing tests in this file, submit a turn
    // whose tools include at least the shell/local_shell tool that's always available ...

    // assertion: the request wiremock received actually contains a non-null "tools" field
    // with at least one entry whose function.name matches a real available tool.
}
```

The exact wiremock request-capture mechanism (received-requests API, `MockServer::received_requests()`, or a custom `Respond` matcher that inspects the body) needs to be confirmed against real wiremock docs/existing usage in this workspace during implementation — this plan sketches the test's *intent* (assert the request body, not just the response), not a guessed-at exact wiremock API surface. Mirror whichever pattern this workspace's other wiremock-based tests already use for inspecting a received request body; if none exists yet, `wiremock::MockServer::received_requests()` (returns `Option<Vec<Request>>` when recording is enabled via `.expect(1)` + the server's request log) is the standard approach — verify against the actual wiremock version pinned in this workspace's `Cargo.lock`.

- [ ] **Step 7: Run the test to verify it fails**

Run: `source "$HOME/.cargo/env"; cd codex-rs && just test -p codex-core -- ollama_native_chat_request_includes_tool_definitions`
Expected: FAILS before Step 4's fix is in place would show no tools sent; since Step 4 already landed in this same task, this test should already pass once written correctly — if so, temporarily verify RED by re-checking out the pre-fix `tools: None` line locally (do not commit this), confirming the test fails against the old behavior, then restore the fix and confirm GREEN. This is the same "scratch-and-revert" verification technique the original native-ollama-backend plan used to prove a test is real (see that plan's own lessons-learned in `.superpowers/sdd/progress.md`) — apply it here too, since this specific test exists precisely to catch the class of bug that shipped silently once already.

- [ ] **Step 8: Run the full relevant test suite**

Run: `source "$HOME/.cargo/env"; cd codex-rs && just test -p codex-core -- ollama_native && just test -p codex-core -- ollama_tools_from_prompt`
Expected: all tests pass, 0 failures.

- [ ] **Step 9: Full-workspace compile sweep**

Run: `source "$HOME/.cargo/env"; cd codex-rs && cargo check --workspace --all-targets --keep-going 2>&1 | tee /tmp/ollama-tools-check.log; grep -c "^error" /tmp/ollama-tools-check.log`
Expected: 0.

- [ ] **Step 10: Live verification against a real Ollama server**

Mirror this fix's own design-phase investigation (already captured in the spec doc) and the original native-ollama-backend plan's Task 10: run a real `--local-provider ollama-native -m qwen3-coder:30b` turn against Rikudo requiring a shell command, and confirm it now produces a genuine `tool_calls` response and executes correctly — where before this fix it did not (multiple documented failed attempts exist in `.superpowers/sdd/scratch-task4-workdir/` from the TOON plan's Task 4, showing the pre-fix broken behavior for comparison).

- [ ] **Step 11: Commit**

```bash
cd codex-rs && git add core/src/client.rs core/tests/suite/ollama_native.rs && git commit -m "$(cat <<'EOF'
fix(ollama): send real tool definitions over the native /api/chat wire

stream_ollama_native_chat_api hardcoded tools: None, so no tool
definitions ever reached the model over the ollama-native wire —
real agentic tool-calling (shell commands, MCP tools) silently
didn't work. Add ollama_tools_from_prompt to convert the turn's
ToolSpec list into the nested function-calling JSON shape Ollama's
native API expects (verified live against a real server) and wire
it in. A new integration test asserts the outgoing request actually
carries tool definitions, not just that a mocked response parses —
closing the exact gap that let this ship unnoticed the first time.
EOF
)"
```
