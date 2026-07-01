# Fix `tools: None` on the Ollama native wire — design spec

**Status:** approved, ready for an implementation plan. Not yet implemented.
**Owner intent:** the already-shipped `ollama-native` backend (`docs/superpowers/specs/2026-07-01-native-ollama-backend-design.md`,
merged via PR #1) never sends tool definitions to the model — every request hardcodes `tools: None`.
Real agentic tool-calling (the primary use case this backend exists to serve) does not work over this
wire today. This spec fixes that specific gap.

## Problem

Confirmed by reading source: `codex-rs/core/src/client.rs`'s `stream_ollama_native_chat_api` builds:

```rust
let request = OllamaChatRequest {
    model: model_info.slug.clone(),
    messages: ollama_messages_from_prompt(prompt),
    think,
    stream: true,
    tools: None,
};
```

`tools: None` regardless of what tools the turn actually has available. Discovered during the TOON
support plan's Task 4 live verification (`.superpowers/sdd/task-4-report.md`): multiple real prompts
against `qwen3-coder:30b` and `gemma4:31b-it-qat` on Rikudo via `--local-provider ollama-native` never
produced a real tool call — models either described the action in prose or emitted a malformed
pseudo-tool-call as plain message text (`{"command": ["shell", "echo hi > hello.txt"]}`).

Confirmed empirically (curl against Rikudo, 2026-07-01) that this is fixable: a hand-built `/api/chat`
request carrying a `tools` array in OpenAI Chat-Completions' nested shape
(`{"type":"function","function":{"name":...,"description":...,"parameters":...}}`) gets a correct,
real `tool_calls` response back from `qwen3-coder:30b` — the model and the native endpoint both
support tool-calling correctly; the gap is purely that codex-rs never sends the definitions.

Checked elf-dispatch (`/home/chris/Documents/GitHub/elf-dispatch`, the reference implementation this
whole feature was modeled on) for prior art: it has **no tool-calling code for Ollama at all** — grep
of `src/dispatch/ollama-backend.js` finds no `tools` handling, and no finding doc mentions it.
elf-dispatch's own Ollama usage is single-shot structured extraction/review (`review-bugs`,
`pr-summary`, `json-extract`), never an agentic tool loop, so it never needed to solve this. There is
no elf-dispatch code to port for this fix — the design instead reuses this repo's own proven sibling
pattern (below).

## Goal

Make `stream_ollama_native_chat_api` send the turn's real tool definitions, in the shape Ollama's
native `/api/chat` actually accepts (verified above), so `--local-provider ollama-native` can drive
real agentic turns (shell commands, MCP tools) — the intended purpose of the whole backend.

## Non-goals (for this fix)

- Not adding namespace-flattening, tool-name-collision handling, or `ToolKinds`/response-side tracking
  the way `chat-wire-compat`'s `convert_tools` does for its own wire (`codex-rs/chat-wire-compat/src/request.rs:495-706`).
  Ollama-native's existing (already-shipped) response-parsing path (`codex-rs/ollama-wire/src/chat_events.rs`)
  does none of that today, and adding it is out of scope for a request-side bug fix — would expand this
  into a much larger, differently-scoped change. If a future turn needs namespaced/flattened tools over
  this wire, that's a separate spec.
- Not handling every `ToolSpec` variant with full fidelity. `ToolSpec::WebSearch` and
  `ToolSpec::ImageGeneration` are OpenAI-hosted-tool types with no Ollama-native equivalent — skip them
  (omit from the sent `tools` array) rather than inventing a shape Ollama can't execute. `ToolSpec::Namespace`
  is also out of scope per the point above.
- Not touching the OpenAI-compat `ollama` provider (Responses/Chat wires) — those already send tools
  correctly (confirmed: `client.rs:836`, `create_tools_json_for_responses_api`). This fix is scoped
  entirely to the `OllamaNative` wire's request construction.

## Where this code lives

Per this repo's own established precedent from the original native-ollama-backend plan:
`ollama_messages_from_prompt` (message conversion, exhaustive over `ResponseItem` variants) already
lives directly in `codex-rs/core/src/client.rs`, next to `stream_ollama_native_chat_api`, rather than in
`codex-ollama-wire` — because it needs `Prompt`/`ResponseItem`, core-level types. Tool conversion is the
same shape of problem (exhaustive over `ToolSpec`, a `codex-tools` type also reachable from `codex-core`)
and belongs in the same file, next to its sibling function.

## Architectural seam (verified from source and live-tested)

### The conversion function

Add `ollama_tools_from_prompt` in `codex-rs/core/src/client.rs`, next to `ollama_messages_from_prompt`:

```rust
/// Converts a turn's available tools into the JSON shape Ollama's native `/api/chat` expects for its
/// `tools` field — OpenAI Chat-Completions' nested `{"type":"function","function":{...}}` shape,
/// confirmed by curl against a real Ollama server (`qwen3-coder:30b`, 2026-07-01: a request built in
/// exactly this shape got back a real, correctly-parsed `tool_calls` response). This is the same
/// nested shape `chat-wire-compat`'s `convert_tools` (`chat-wire-compat/src/request.rs:495-706`)
/// produces for the sibling Chat-Completions-compat wire — that function is not reused directly here
/// because it also does namespace-flattening/tool-name-collision bookkeeping this wire's
/// (already-shipped) response-parsing path does not consume; see this spec's Non-goals.
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
            // Out of scope per this spec's Non-goals — omit rather than send a shape Ollama can't
            // execute.
            ToolSpec::Namespace(_) | ToolSpec::WebSearch { .. } | ToolSpec::ImageGeneration { .. } => None,
        })
        .collect();

    if tools.is_empty() { None } else { Some(tools) }
}
```

Confirm during implementation (not guessed here) whether `ToolSpec::LocalShell`'s real, already-shipped
parameter shape lives somewhere reusable instead of the hand-written schema above — search for
`ShellToolCallParams` (referenced in `chat-wire-compat/src/request.rs:551`, used for the sibling wire's
own `local_shell` handling) and its `JsonSchema`/schema-generation, and prefer reusing that exact schema
over hand-duplicating it, to avoid the two wires' shell-tool schemas silently drifting apart over time.

### Wiring it in

Change `stream_ollama_native_chat_api`'s request construction (`client.rs`, currently
`tools: None`):

```rust
let request = OllamaChatRequest {
    model: model_info.slug.clone(),
    messages: ollama_messages_from_prompt(prompt),
    think,
    stream: true,
    tools: ollama_tools_from_prompt(prompt),
};
```

## Testing

Per `AGENTS.md`: prefer integration tests over unit tests for agent-visible behavior.

- `codex-rs/core/src/client.rs` (or a new `#[cfg(test)]` module near `ollama_tools_from_prompt`): unit
  tests confirming the JSON shape for at least `ToolSpec::Function` and `ToolSpec::LocalShell` — assert
  the nested `{"type":"function","function":{...}}` structure, not just "doesn't panic". A tools list
  containing only out-of-scope variants (`WebSearch`/`ImageGeneration`/`Namespace`) must produce `None`
  (an empty `tools` array should not be sent as `Some(vec![])` — Ollama's own docs/behavior around an
  empty-but-present `tools` array vs. an absent one should be confirmed during implementation, not
  assumed, before deciding whether the empty-after-filter case should collapse to `None` as sketched
  above or send `Some(vec![])`).
- `codex-rs/core/tests/suite/ollama_native.rs` (already has two tests from the original plan) gains a
  new integration test: mock the `/api/chat` endpoint, submit a turn with a real shell/function tool
  available, and assert the mocked server actually **received** a `tools` field in the POST body — the
  existing `ollama_native_chat_turn_handles_tool_call` test only asserts the *response* is parsed
  correctly (via a scripted mock reply); it never asserts anything about the outgoing *request*, which
  is exactly how this bug shipped unnoticed. `wiremock`'s request-body matchers (or a captured-request
  assertion, mirroring how `codex-ollama`'s existing wiremock tests already inspect request bodies
  elsewhere in this workspace) should be used to close that gap for real, not just add another
  response-mocking test that would miss the same class of bug again.
- A live smoke test against a real Ollama instance (not mocked), mirroring the original
  native-ollama-backend plan's Task 10 and this fix's own investigation: confirm a real
  `--local-provider ollama-native` turn that requires a shell command now produces a genuine
  `tool_calls` response and executes it, where before this fix it did not.

## Resolved decisions

1. **Fix scope is request-construction only — no response-parsing/namespace changes.** The bug is
   narrowly "we never send tool definitions"; fixing it doesn't require touching the already-shipped,
   already-working tool-call-parsing path (`chat_events.rs`'s handling of `message.tool_calls` in
   responses is unaffected and unchanged).
2. **Reference `chat-wire-compat`'s `convert_tools` shape, don't reuse it directly.** Verified the exact
   output shape it produces for `"function"`/`"tool_search"`/`"local_shell"`/`"custom"` tool types is
   what Ollama's native API actually accepts (live-curl-verified against Rikudo) — but that function
   also carries namespace-flattening and `ToolKinds`/`OriginalFunctionNames` bookkeeping specific to its
   own wire's response-processing pipeline, which Ollama-native's simpler, already-shipped response path
   doesn't use and doesn't need for this fix.
3. **`WebSearch`/`ImageGeneration`/`Namespace` tool types are omitted, not attempted.** They're
   OpenAI-hosted-tool concepts with no Ollama-native equivalent; silently dropping them from the sent
   `tools` array (rather than erroring or inventing an unsupported shape) is the correct behavior — a
   turn that also has real function/shell tools available still gets those, just not the
   Ollama-incompatible ones.
4. **elf-dispatch has no relevant prior art for this fix** (checked, confirmed above) — the design
   instead grounds itself in this repo's own already-shipped `chat-wire-compat` sibling wire and a live
   empirical verification against Rikudo, the same rigor the original native-ollama-backend spec used
   throughout.
