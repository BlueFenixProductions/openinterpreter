# Native Ollama backend for Open Interpreter — design spec

**Status:** decisions resolved (see "Resolved decisions" below), ready for an implementation plan. Not yet implemented.
**Owner intent:** give Open Interpreter's `--local-provider ollama` a wire path that actually reaches
Ollama's native inference API, so reasoning-suppression (`think:false`) is possible — parity with
elf-dispatch's own `ollama-backend.js`, which this spec uses throughout as the reference behavior.

## Problem

Open Interpreter's built-in `ollama` provider (`codex-rs/model-provider-info/src/lib.rs:527`,
`create_oss_provider(DEFAULT_OLLAMA_PORT, WireApi::Responses)`) only ever speaks one of three
generic wire protocols — `Responses` (`/v1/responses`), `Chat` (`/v1/chat/completions`), or
`Messages` (`/v1/messages`, Anthropic-shaped, irrelevant here). All three are Ollama's
**OpenAI-compat surface**. None of them exposes Ollama's native `think` parameter.

Confirmed empirically (curl against Rikudo's live Ollama, 2026-07-01): `/v1/responses` works and
returns a well-formed Responses-API payload, so the built-in provider isn't broken — it's just the
wrong transport for reasoning control. A full-codebase grep (`grep -rn '"/api/chat"\|/api/generate'
codex-rs --include="*.rs"`) returns **zero matches** — there is no code path anywhere in this
workspace that calls Ollama's native `/api/chat` or `/api/generate`.

elf-dispatch avoids exactly this problem with its own dispatch core: `ollama-backend.js` uses the
official `ollama` npm package, whose `.chat()` method (confirmed in
`node_modules/ollama/dist/browser.mjs:339`) POSTs to `${host}/api/chat` with a `think` field, and
`config.env` pins `LLM_DISPATCH_BACKEND=ollama` specifically because the `/v1` REST surface **cannot
disable reasoning on Ollama** (verified 2026-06-23 in that project: gemma4 via `/v1` returns empty
content, reasoning eats the whole token budget). Open Interpreter's `--oss --local-provider ollama`
has that identical failure mode today, just via a different generic wire (`Responses` instead of
`/v1` chat-completions) — same root cause, same fix shape.

## Goal

Add a fourth wire path — call it **`OllamaNative`** — that:
1. POSTs to `{base_url}/api/chat` (Ollama's native endpoint, not the OpenAI-compat one).
2. Carries a `think: bool` (or `"low"|"medium"|"high"` per model, mirroring
   `ollama-backend.js`'s per-model override table) through from config/CLI.
3. Parses Ollama's native streaming NDJSON chat response and maps it onto the existing internal
   `ResponseEvent` enum (`codex-rs/codex-api/src/common.rs:73`) so the rest of the agent loop
   (TUI rendering, tool-call handling, token accounting) needs no changes.
4. Ships as a new, explicit provider id (`ollama-native`) alongside the existing `ollama` —
   **additive, not a default change** to the current `ollama` provider's behavior, per this repo's
   own "Breaking changes" review checklist (`AGENTS.md`: CLI parameters and configuration loading
   are named as surfaces to check).

## Non-goals (for this first cut)

- Not touching LM Studio at all — its SDK path (`@lmstudio/sdk`, WebSocket) is a completely separate
  concern elf-dispatch already solved differently; this spec is Ollama-only.
- Not attempting harness-emulation integration for this route in v1 — ship it as a plain
  `StreamTransportRoute` first (mirroring the existing bare `ResponsesApi`/`ChatCompletionsCompat`
  arms in `routing.rs`), and revisit whether `ChatHarness`-style shaping is worth adding for it once
  it's working end to end. Not permanently out of scope — see Decision 4 below.

Tool-calling is explicitly **in scope for v1** (reversed from an earlier draft of this spec — see
Decision 1): confirmed cheap, not deferred.

## Where this code lives

Per `AGENTS.md`'s explicit guidance ("resist adding code to codex-core"; prefer an existing crate or
a new one): this belongs in the **existing `codex-ollama` crate**
(`codex-rs/ollama/`, package `codex-ollama`), not `codex-core`. That crate already:
- Depends on `codex-core` and `codex-model-provider-info` (Cargo.toml already wires both).
- Has NDJSON-streaming infrastructure (`src/line_buffer.rs`, 32 lines — already splits a byte stream
  on newlines for `/api/pull`'s streamed JSON; the same framing applies to `/api/chat` streaming).
- Has a `client.rs` that already builds request URLs off `self.host_root` — the natural place to add
  a `chat()` method alongside the existing `fetch_models`/`fetch_version`/`pull_model_stream`.
- Has zero existing chat/inference logic to conflict with (confirmed: `client.rs` today only does
  `/v1/models` + `/api/tags` probing, `/api/tags`, `/api/version`, `/api/pull` — no `/api/chat`
  anywhere in the crate).

New files, keeping modules under the ~500 LoC guidance in `AGENTS.md`:
- `codex-rs/ollama/src/chat.rs` — request/response types for `/api/chat`, and the actual
  `chat_stream()` function.
- `codex-rs/ollama/src/chat_events.rs` — translation from Ollama's native streamed chat chunks into
  `codex_api::ResponseEvent`.

## Architectural seam (verified from source)

Confirmed the exact extension point: `WireApi` (transport) and `Harness` (prompt/tool-shaping) are
already separate dimensions that combine in `resolve_stream_transport_route()`
(`codex-rs/core/src/harness/routing.rs:52`) into a `StreamTransportRoute`, which
`core/src/client.rs`'s `stream()` (line ~2339) matches on to actually dispatch the request. This is
the right seam — it already cleanly separates "how do I talk to the server" from "how do I shape the
prompt for this specific harness."

### 1. `WireApi` — new variant

`codex-rs/model-provider-info/src/lib.rs:61`, current definition:

```rust
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum WireApi {
    #[default]
    Responses,
    Chat,
    Messages,
}
```

Add a fourth variant:

```rust
pub enum WireApi {
    #[default]
    Responses,
    Chat,
    Messages,
    /// Ollama's native chat API at `/api/chat` — NOT the OpenAI-compat surface.
    /// Only meaningful for Ollama-family providers; carries `think` support.
    OllamaNative,
}
```

Update the `Display` impl (`lib.rs:71-78`) with an `OllamaNative => "ollama_native"` arm — the match
is already exhaustive over the other three, so the compiler will force this.

### 2. `StreamTransportRoute` — new variant

`codex-rs/core/src/harness/routing.rs:35`, current definition (partial):

```rust
pub(crate) enum StreamTransportRoute {
    ResponsesApi,
    ChatCompletionsCompat,
    ChatHarness(ChatHarnessRoute),
    MessagesHarness(MessagesHarnessRoute),
    ClaudeCodeResponses(ClaudeCodeProfileRoute),
    ClaudeCodeChat(ClaudeCodeProfileRoute),
}
```

Add:

```rust
    /// Ollama's native /api/chat, carrying `think` — no harness-specific shaping in v1.
    OllamaNativeChat,
```

And in `resolve_stream_transport_route()` (`routing.rs:52`), add the match arm (v1: harness-agnostic,
matching how `ResponsesApi`'s fallthrough arm at line 63 works today):

```rust
        (WireApi::OllamaNative, _) => Ok(StreamTransportRoute::OllamaNativeChat),
```

### 3. `built_in_model_providers()` — new provider id

`codex-rs/model-provider-info/src/lib.rs:512`, add alongside the existing `ollama`/`lmstudio`
entries:

```rust
pub const OLLAMA_NATIVE_PROVIDER_ID: &str = "ollama-native";
```

```rust
        (
            OLLAMA_NATIVE_PROVIDER_ID,
            create_oss_provider(DEFAULT_OLLAMA_PORT, WireApi::OllamaNative),
        ),
```

`create_oss_provider` (`lib.rs:595`) already takes a `wire_api: WireApi` parameter and respects
`CODEX_OSS_BASE_URL`/`CODEX_OSS_PORT` env overrides — no changes needed there, it's already generic.

### 4. CLI surface

`codex-rs/exec/src/lib.rs:392` already lists valid `--local-provider` values by interpolating
`{LMSTUDIO_OSS_PROVIDER_ID}, {OLLAMA_OSS_PROVIDER_ID}` into its error message — add
`{OLLAMA_NATIVE_PROVIDER_ID}` to that same message so the help text stays accurate. Confirm (during
implementation, not guessed here) whether `--local-provider` validates against a hardcoded list
elsewhere that also needs the new id added — search `codex-rs/exec/src` and `codex-rs/cli/src` for
every reference to `OLLAMA_OSS_PROVIDER_ID` and add the native id at each site, not just the error
string above.

### 5. The actual request/response (new code, `codex-ollama` crate)

Ollama's native `/api/chat` (streaming, `"stream": true` — the default) request shape:

```json
{
  "model": "gemma4:26b-a4b-it-qat",
  "messages": [{"role": "user", "content": "..."}],
  "think": false,
  "stream": true,
  "options": {"temperature": 0.1}
}
```

Streamed response: one JSON object per line (NDJSON), each with `message.content` (an incremental
delta) until a final line with `"done": true` carrying `eval_count`, `eval_duration`,
`prompt_eval_count`, `done_reason`. This is the same *framing* mechanism (NDJSON) already handled by
`line_buffer.rs` for `/api/pull` — reuse that module, don't reimplement NDJSON splitting.

Map each streamed chunk to `ResponseEvent` (`codex-rs/codex-api/src/common.rs:73`):
- Non-final chunk with `message.content` → `ResponseEvent::OutputTextDelta(content)`.
- If Ollama's response includes a separate `message.thinking` field (recent Ollama versions
  surface reasoning separately from content for models like gemma4 — **confirm this against the
  installed Ollama version during implementation**, don't assume the schema from this spec alone) →
  `ResponseEvent::ReasoningContentDelta { delta, content_index }`.
- A chunk carrying `message.tool_calls` → confirmed empirically (curl against Rikudo, both
  `stream:false` and `stream:true`) that Ollama sends the **entire** tool call in one chunk, never
  incrementally-streamed argument deltas the way OpenAI's streaming tool calls work:
  ```json
  {"message":{"role":"assistant","content":"","tool_calls":[{"id":"call_sh02sro7","function":{"index":0,"name":"get_weather","arguments":{"city":"Paris"}}}]},"done":false}
  ```
  followed by a separate final `"done":true` chunk with no tool_calls, carrying only usage stats.
  Map the whole `tool_calls[i]` object to `ResponseEvent::ToolCallInputDelta { item_id: call.id,
  call_id: Some(call.id), delta: serde_json::to_string(&call.function.arguments)? }` in one shot
  (a single "delta" containing the complete JSON, since there's nothing to accumulate across
  chunks) — confirm during implementation whether the existing `ResponseEvent` consumers assume
  `ToolCallInputDelta` may be incremental and require a follow-up "done" signal; if so, check
  whether emitting it as a single complete delta is sufficient or a small consumer-side adjustment
  is needed. `function.index` in Ollama's payload maps to tool-call ordering when multiple tools
  are called in one turn — thread it through if the target `ResponseEvent` shape needs it.
- Final chunk (`"done": true`) → `ResponseEvent::Completed { response_id, token_usage: Some(...), end_turn: Some(done_reason == "stop") }`,
  building `token_usage` from `prompt_eval_count`/`eval_count`.
- A chat response that errors (non-2xx, or a mid-stream error field) → surface as the same
  `CodexErr` variant the `ResponsesApi` route uses today for a failed request — check
  `core/src/client.rs`'s `ResponsesApi` arm for the exact error type before implementing, don't
  invent a new error variant unless the existing one genuinely doesn't fit.

### 6. Config surface for `think`

`ModelProviderInfo` (`codex-rs/model-provider-info/src/lib.rs:103`, `#[schemars(deny_unknown_fields)]`,
`Option<T>` fields throughout for provider-specific knobs) is the right home. Add:

```rust
    /// Reasoning-suppression override table for the OllamaNative wire, mirroring elf-dispatch's
    /// ollama-backend.js resolveThink(): comma-separated "substr:value" pairs, checked against the
    /// model id in order, first match wins. value is "false" (think:false, the default behavior
    /// when this field is unset), "true", or an effort string ("low"/"medium"/"high") for models
    /// like gpt-oss that ignore a bare think:false and need an effort level instead. Ignored for
    /// any wire_api other than OllamaNative.
    pub ollama_think: Option<String>,
```

Default behavior when `ollama_think` is unset: `think: false` unconditionally — matches
`ollama-backend.js`'s own default (`resolveThink` falls through to `false` when no config and no
per-model match). Run `just write-config-schema` after adding this field, per `AGENTS.md`.

## Testing

Per `AGENTS.md`: prefer integration tests (`core/suite`, `test_codex`) over unit tests for
agent-visible behavior; use `just test -p codex-ollama` for the new crate's own unit tests
(NDJSON parsing, chat-event translation), not `cargo test` directly. Any wiremock-based test for the
new `chat()` method should live in `codex-ollama` next to the existing `client.rs`
wiremock tests (`test_probe_server_happy_path_openai_compat_and_native` is the existing pattern to
match).

A live smoke test against a real Ollama instance (not mocked) should confirm, at minimum: a
`think:false` request against a real reasoning-capable model (e.g. `gemma4:26b-a4b-it-qat` on
Rikudo) returns clean final content without the reasoning trace eating the response, mirroring the
exact failure this spec exists to fix.

## Resolved decisions

1. **Tool-calling is in v1, not deferred.** Confirmed empirically against Rikudo's live Ollama
   (both `stream:false` and `stream:true`): the native `/api/chat` tool-call shape is clean and
   arrives as one complete chunk, not incremental argument deltas — a bounded, low-risk translation
   (see §5 above), not a research spike. Shipping without tool use would cripple the very use case
   (agentic coding) this backend exists to serve, so there's no good reason to cut it.
2. **Ship the per-model `think` override table in v1, not a bare bool.** elf-dispatch's own history
   is the reason: `ollama-backend.js`'s `resolveThink()` exists specifically because gpt-oss-class
   models silently ignore a bare `think:false` and need `think:"low"` instead — a real bug already
   found and fixed once in the reference implementation. Shipping a global-bool-only version here
   would silently reintroduce that exact already-solved bug for any gpt-oss-family model used
   through this route. The override table is one small parsing function (`ollama-backend.js`'s
   `resolveThink` is ~15 lines) — not proportionate to skip given the known failure mode.
3. **Field shape: `ModelProviderInfo.ollama_think: Option<String>`**, comma-separated
   `"substr:value"` pairs, unset → `think:false` unconditionally. Decided against reading the full
   struct (§6 above) — it's `Option<T>`-per-knob throughout, no nested-struct precedent to break
   from, and a string-table matches elf-dispatch's own `LLM_REASONING_EFFORT` config format closely
   enough that anyone who knows the reference implementation recognizes it immediately.
4. **Harness-emulation shaping stays out of v1, not permanently.** `StreamTransportRoute`'s own
   history already shows this exact growth path — `ResponsesApi` and `ChatCompletionsCompat`
   shipped bare, and harness-specific variants (`ClaudeCodeResponses`, `ChatHarness(...)`) were
   layered on after. Do the same here: ship `OllamaNativeChat` bare, revisit whether a
   `think:false` + harness-shaped-prompt combination is ever wanted once the base route is proven
   against a real coding task on Rikudo.
