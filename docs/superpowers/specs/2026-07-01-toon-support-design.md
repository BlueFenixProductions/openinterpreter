# TOON support for Open Interpreter — design spec

**Status:** approved, ready for an implementation plan. Not yet implemented.
**Owner intent:** cut LLM I/O token cost on the two structured-data seams codex-rs actually has today,
by porting elf-dispatch's bench-validated TOON (Token-Oriented Object Notation) work — its shipped
output-side design directly, and a fresh input-side design built from its stated-but-never-shipped
intent, mapped onto codex-rs's real architecture.

## Problem

TOON is a token-efficient, lossless drop-in for JSON when transmitting structured data to/from an
LLM (spec: `github.com/toon-format/spec` v3.2; TS SDK: `github.com/toon-format/toon`). It eliminates
JSON's key-repetition-per-object cost for uniform arrays via a tabular header format, at the cost of
requiring the reader (model or code) to understand TOON's indentation/delimiter rules instead of
JSON's.

elf-dispatch (`/home/chris/Documents/GitHub/elf-dispatch`) evaluated this in three dated finding docs
and shipped it for one direction:

- `docs/findings/2026-06-20-toon-token-oriented-object-notation.md` — what TOON is.
- `docs/findings/2026-06-20-toon-integration-approaches.md` — three options (input-only, output-only,
  both), recommending input-only first as lowest-risk.
- `docs/findings/2026-06-23-toon-output-bench-gate.md` — the Captain's actual call: bench-gate the
  **structured-output** lane instead. Full 6-model × 3-schema × 3-rep sweep against real Ollama
  models on real hardware. Verdict: **emit-validity is per-(model × schema), not universal** — flat
  schemas (`json-extract`) hit 100% valid emission across all 6 models tested (14–56% token savings);
  nested/free-text schemas (`bugs[]`) only 3 of 6 models emit reliably (qwen3-coder:30b: 100% valid,
  up to 63% savings). Adopted pattern: **"ask for TOON, accept either"** —
  `decodeToon(reply) ?? extractJson(reply)` — so a model that ignores the TOON instruction and replies
  in JSON still parses; the win is pure upside, gated per-model per-schema, off by default.

Confirmed by reading elf-dispatch's actual source (`src/dispatch/toon.js`, `src/dispatch/core.js`,
`src/dispatch/fleet.js`, `config.example.env`): **only the output direction shipped.** A repo-wide
grep for `encode(` under `src/` returns zero production call sites — the input-side recommendation in
the 2026-06-20 doc was never built. So this spec ports the output-side design directly (it has real
bench evidence behind it) and designs the input side fresh, mapped onto codex-rs's own structure,
since there is no elf-dispatch production code to port for that half.

Open Interpreter (`codex-rs`) has no TOON support today — confirmed via
`grep -rniI '\btoon\b' codex-rs docs` (zero matches, excluding "cartoon"-type false positives).

## Goal

Add TOON support on both directions, scoped to the two real structured-data seams that exist in
codex-rs today (not a blanket refactor of every `serde_json::to_string` call site — see Non-goals):

1. **Output side:** the guardian auto-reviewer's structured final-answer path
   (`codex-rs/core/src/guardian/`) gains a TOON-emit option, gated per-model, off by default.
2. **Input side:** MCP tool-call results with `structured_content` gain a TOON-encode option when
   they're re-injected into the model's context on later turns, gated by a single experimental
   toggle, off by default.

## Non-goals (for this first cut)

- Not TOON-ifying the wire-protocol envelope (message roles, tool schemas, the Responses/Chat/Messages
  JSON bodies themselves) — only structured *content* embedded as text within a message. The
  transport JSON stays JSON; only what a message's text body contains can become TOON.
- Not touching `codex-rs/core/src/tools/handlers/agent_jobs.rs`'s `build_worker_prompt` (embeds one
  CSV row as JSON per prompt). Considered and explicitly deferred (Captain's call, this session): it's
  a single object, not an array, so TOON's main win (eliminating repeated keys across array elements)
  barely applies there — real but marginal, not proportionate to include in v1.
- Not building a bench harness as elaborate as elf-dispatch's `bench/toon-run.ts` (6 models × 3
  schemas × 3 reps, dedicated result-comparison tooling) for v1. See Testing below for the
  proportionate v1 verification approach.
- Not attempting to bench-gate the *input* side the way the output side is bench-gated. There is no
  accuracy/validity axis to measure for reading TOON the way there is for emitting it (the model isn't
  producing anything to grade) — the real unknown is comprehension quality, which this spec does not
  claim to have verified. Ship the input side behind an explicit, clearly-labeled experimental toggle,
  not a bench-proven allowlist, and say so in the config doc-comment.
- Not extending TOON to every MCP tool result — only ones carrying `structured_content` (the
  `content` fallback path, plain text/image blocks, is untouched).

## Where this code lives

New leaf crate `codex-toon` (`codex-rs/toon/`, package `codex-toon`), following the same pattern
Task 9a established for `codex-ollama-wire`: a dependency-free leaf crate so both `codex-core` and
`codex-protocol` can depend on it without creating a cycle (`codex-protocol` sits below `codex-core`
in the dependency graph; a crate `codex-protocol` depends on must not depend back on `codex-core` or
`codex-protocol`).

Wraps `toon-format` (crates.io, `github.com/toon-format/toon-rust` — the official Rust port, same
upstream org as the spec elf-dispatch used), pulled in with `default-features = false` — the crate's
default `cli` feature drags in `clap`, `ratatui`, `syntect`, `tiktoken-rs`, `comfy-table`,
`tui-textarea`, `arboard`, `crossterm`, `chrono`, none of which this workspace needs; only the bare
`encode`/`decode` module tree is required.

`codex-rs/toon/src/lib.rs` exposes exactly three functions:

```rust
pub fn encode(value: &serde_json::Value) -> Result<String, toon_format::ToonError> {
    toon_format::encode_default(value)
}

/// Ports elf-dispatch's `decodeToon` guard (src/dispatch/toon.js): TOON's top level is always
/// `key: value` / `key[N]{...}:` lines and never starts with `{` or `[`. A compact single-line
/// JSON object (`{"ok":1}`) would otherwise decode as a one-line TOON `key: value` pair (key
/// `{"ok"`, value `1}`) — reject brace/bracket-leading input here so JSON always wins the ambiguous
/// case, matching the original's load-bearing guard.
pub fn decode_toon(text: &str) -> Option<serde_json::Value> {
    let body = text.trim();
    // strip a single ```toon / ``` fence pair if the model wrapped its output
    let body = body
        .strip_prefix("```toon").or_else(|| body.strip_prefix("```"))
        .unwrap_or(body)
        .trim_start_matches('\n');
    let body = body.strip_suffix("```").unwrap_or(body).trim();
    if body.is_empty() { return None; }
    if body.starts_with('{') || body.starts_with('[') { return None; }
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
```

Confirm during implementation (not guessed here) whether `toon_format::decode_strict`'s error
behavior on malformed input matches the "return `None`, never panic" contract this function promises
— wrap in `.ok()` as shown, but verify `decode_strict` doesn't have a differently-shaped failure mode
(e.g. panics on certain malformed input) before relying on it.

## Architectural seam 1 — output side (guardian review)

Confirmed via `grep -rln final_output_json_schema codex-rs`: every call site except
`guardian/review_session.rs:767` passes `None`. Guardian review is the only real structured-output
consumer in the codebase today.

### `guardian/prompt.rs::parse_guardian_assessment()` (currently lines 589–603)

Current behavior: try `serde_json::from_str::<GuardianAssessmentPayload>(text)`, then fall back to
finding the outermost `{...}` slice and parsing that, else bail. Structurally identical to
elf-dispatch's `extractJson`. Add a third tier after both JSON attempts fail, before bailing:

```rust
} else if let Some(value) = codex_toon::decode_toon(text) {
    serde_json::from_value::<GuardianAssessmentPayload>(value)?
} else {
    anyhow::bail!("guardian assessment was not valid JSON or TOON");
}
```

This mirrors elf-dispatch's ordering exactly (`extractJson(text) ?? decodeToon(text)`): JSON is tried
first and unconditionally, TOON is a fallback interpretation of the same reply, not a separate
request. A model that ignores the TOON instruction and answers in JSON is unaffected.

### `guardian_output_contract_prompt()` (currently `prompt.rs:672`)

Add a TOON variant — the existing contract text plus `codex_toon::toon_instruction()` plus a compact
schema hint derived from `guardian_output_schema()` — selected per-model at call time (see Config
surface below), mirroring `SYSTEM_REVIEW_TOON` in elf-dispatch's `core.js`.

### Gating

`GuardianReviewSessionParams` (`review_session.rs:73`) already carries `model: String`
(`review_session.rs:80`). At prompt-build time, check `model` against the configured allowlist
(substring match, see Config surface) the same way elf-dispatch's `wantsToon(model, capableList)`
does — a model not in the list gets the plain JSON-only prompt, unchanged from today.

## Architectural seam 2 — input side (MCP tool-result re-injection)

Confirmed via read of `codex-rs/protocol/src/models.rs:1925`,
`CallToolResult::as_function_call_output_payload()`:

```rust
pub fn as_function_call_output_payload(&self) -> FunctionCallOutputPayload {
    if let Some(structured_content) = &self.structured_content
        && !structured_content.is_null()
    {
        match serde_json::to_string(structured_content) {
            Ok(serialized_structured_content) => {
                return FunctionCallOutputPayload {
                    body: FunctionCallOutputBody::Text(serialized_structured_content),
                    success: Some(self.success()),
                };
            }
            // ...
        }
    }
    // falls through to `self.content` handling — untouched by this spec
}
```

This is the exact seam: an MCP server's `structured_content` (arbitrary JSON — commonly a uniform
array: DB rows, search hits, file listings, matching elf-dispatch's own cited Option-A example almost
exactly) gets JSON-serialized into text that becomes part of the model's context and is **re-sent as
input tokens on every subsequent turn** until compaction evicts it — precisely the "scales with data
volume" cost the original TOON finding doc calls out.

Change: when TOON-for-input is enabled (see Config surface) and `structured_content` is present and
non-null, attempt `codex_toon::encode(structured_content)` first; on success, use that as the body
text instead of `serde_json::to_string`. **On encode failure, fall back to the existing
`serde_json::to_string` path unconditionally** — this is a token-saving optimization, not a
correctness-load-bearing change, so it must never turn a working tool call into a broken one.
No decode is needed on this side: the text is read by the model, never re-parsed by our own code.

Confirm during implementation whether `FunctionCallOutputBody::Text`'s consumers (TUI rendering,
truncation, rollout persistence) assume JSON-parseable content anywhere downstream of this
call — `truncate_function_output_payload` (referenced from `context.rs:139`) operates on the
`FunctionCallOutputPayload` as opaque text/bytes today, but verify against real source rather than
this spec's assumption before wiring the change in.

### Gating

A single experimental boolean, not a per-server or per-model allowlist (see Non-goals: no bench-gating
axis exists for the input side). Following the existing `experimental_*` flat-field naming convention
already used throughout `ConfigToml` (`config_toml.rs`, e.g. `experimental_use_unified_exec_tool`):

```rust
    /// Encode MCP tool results' `structured_content` as TOON instead of JSON when re-injecting
    /// them into the model's context on later turns, saving input tokens on uniform-array results.
    /// Unlike guardian's TOON output, this has no bench-validated per-model allowlist backing it —
    /// enabling it is a bet that models comprehend TOON input as well as JSON, not a proven one.
    /// Falls back to JSON on any encode failure. Default: disabled.
    pub experimental_toon_tool_results: Option<bool>,
```

## Config surface — guardian's per-model allowlist

Mirrors `AutoReviewToml` (`config_toml.rs:554-557`, currently just `policy: Option<String>`) — add a
sibling field:

```rust
pub struct AutoReviewToml {
    /// Additional policy instructions inserted into the guardian prompt.
    pub policy: Option<String>,

    /// Comma-separated model-id substrings that are bench-proven to reliably emit valid TOON for
    /// the guardian assessment schema (see docs/superpowers/specs/2026-07-01-toon-support-design.md).
    /// A model is asked for TOON output only if its id contains one of these substrings; unset or
    /// empty means TOON is off for every model (unconditional JSON, today's behavior).
    /// Mirrors elf-dispatch's LLM_TOON_REVIEW: populate this only after confirming the target model
    /// actually emits valid TOON for this schema — don't guess.
    pub toon_capable_models: Option<String>,
}
```

Comma-separated substring list (not a `Vec<String>`) to match the existing string-table convention
this repo already established for a similar per-model knob
(`ModelProviderInfo.ollama_think: Option<String>`, native-ollama-backend spec, Decision 3) — anyone
who has seen one recognizes the shape of the other. Empty/unset → TOON off everywhere, matching
elf-dispatch's `LLM_TOON_REVIEW=""` default-off posture.

Run `just write-config-schema` after adding both new fields, per `AGENTS.md`.

## Testing

Per `AGENTS.md`: prefer integration tests over unit tests for agent-visible behavior.

- `codex-toon` crate: unit tests for `encode`/`decode_toon`/`toon_instruction` — round-trip a known
  object through `encode` then `decode_toon`, confirm the brace/bracket JSON-rejection guard (a
  compact JSON object must return `None`, not a mis-parsed TOON object), confirm a fenced
  ` ```toon ` block round-trips. Use `just test -p codex-toon`.
- Guardian output: extend `guardian/prompt.rs`'s existing test module with a TOON-formatted assessment
  reply fed through `parse_guardian_assessment`, confirming it parses identically to the JSON case.
  A second test confirms a model **not** in `toon_capable_models` still receives the plain JSON
  prompt (gating is real, not decorative).
- MCP input encoding: extend `protocol/src/models.rs`'s tests (or add
  `core/tests/suite/mcp_toon_input.rs` if the config plumbing needs a full `test_codex` turn) covering:
  `experimental_toon_tool_results` on + `structured_content` present → TOON body; off → unchanged
  JSON body; on + encode failure (if constructible) → falls back to JSON, doesn't error the tool call.
- **Live verification, not a full bench harness** (proportionate to v1 per Non-goals): one live run
  against a real Ollama server (the native-ollama-backend work just merged makes this straightforward)
  with guardian TOON enabled for that model, confirming a real assessment round-trips correctly and
  recording the actual emitted-token delta vs. the same review in JSON — mirrors how the
  native-ollama-backend plan's Task 10 did a live before/after comparison rather than building a bench
  suite. Capture the comparison output the same way (`.superpowers/sdd/` scratch artifacts), not as a
  permanent bench harness.

## Resolved decisions

1. **Scope: both input and output (Captain's call, this session)** — supersedes elf-dispatch's own
   phased "output first, prove it, then maybe input" caution. Justified because the two directions
   land on architecturally independent seams here (guardian review vs. MCP tool-result injection) —
   shipping both doesn't couple their risk the way it would if they shared a code path.
2. **Output side ports elf-dispatch's shipped design directly; input side is a fresh design**,
   because a repo-wide grep of elf-dispatch's `src/` confirms zero production `encode()` call sites —
   there is no input-side code to port, only a two-day-old recommendation doc's stated intent, mapped
   here onto codex-rs's real `CallToolResult::as_function_call_output_payload()` seam instead of
   invented from scratch.
3. **Guardian is the only output-side target for v1** — not a hypothetical scope choice: every other
   `final_output_json_schema` call site in the codebase passes `None` today, so there is nothing else
   to wire TOON output into yet.
4. **`agent_jobs.rs`'s `build_worker_prompt` is explicitly out of v1** (Captain's call, this session) —
   real candidate, but a single embedded object doesn't benefit from TOON's main win (eliminating
   repeated keys across array elements) the way the MCP tool-result seam does; not proportionate to
   pull into v1 alongside a genuinely high-value target.
5. **Input-side gating is a single experimental boolean, not a bench-proven allowlist** — unlike
   output-side emit-validity, there is no accuracy axis to bench-gate for the model *reading* TOON
   (it isn't producing anything gradeable). Ship it honestly labeled experimental rather than
   pretending a validated allowlist exists where the underlying evidence doesn't.
6. **Encode failure on the input side always falls back to plain JSON, silently** — this is a
   token-saving optimization layered on an existing, working code path; it must never be able to turn
   a successful tool call into a failed one. (Decode failure on the output side is different — TOON
   there is layered as a fallback *after* JSON already failed, so there's no equivalent "silently
   revert" case to design for.)
