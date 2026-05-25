use codex_core::config::Config;
use serde_json::json;
use std::path::Path;

const DEEPSEEK_TUI_PRESERVE_MODEL: &str = "deepseek-v4-flash";
const DEEPSEEK_TUI_PRESERVE_MAX_TOKENS: u32 = 4096;
const DEEPSEEK_TUI_PRESERVE_TEMPERATURE: f64 = 0.20000000298023224;

const DEEPSEEK_TUI_PRESERVE_SYSTEM: &str = r#"# Cycle Handoff Briefing

You are about to cross a context cycle boundary. The conversation so far has
crossed the per-cycle token budget, so this entire transcript is going to be
**archived to disk** and the next turn will start with a fresh context: the
original system prompt, structured state (todos, plan, working set, open
sub-agents), the user's pending message, and a free-form briefing that **you
write right now**.

Your job, in this single message: produce a `<carry_forward>` block of at most
**3,000 tokens** that captures the irreducible state the *next cycle's you* will
need to continue without redoing work.

## What to put in `<carry_forward>`

Write concrete prose, not bullet-point summaries of the transcript. Cover:

- **Decisions made and why.** The things you've chosen and the reasoning that
  led there. Not "we discussed options" — name the choice and the constraint
  that made it the right one.
- **Constraints discovered.** Concrete facts about the codebase, environment,
  user preferences, or external systems that the next cycle will trip over if
  it doesn't know them. (e.g. "the audit log is JSONL not JSON", "the user
  insists on no `unwrap()` in non-test code", "macOS sandbox blocks raw
  sockets in tools/exec.rs".)
- **Hypotheses being tested.** Open questions you're actively investigating,
  what you're trying to falsify, what evidence would change your mind.
- **Approaches that failed.** Dead ends with enough detail that the next
  cycle won't repeat them. Name the approach and the specific reason it
  didn't work, not just "tried X, didn't work".
- **Open questions for the user.** Things you're blocked on that the next
  cycle should ask about if the user doesn't volunteer them.

## What NOT to put in `<carry_forward>`

- Tool output bytes. (They're already archived to disk.)
- File contents you read. (The next cycle can re-read them — pricier than a
  briefing token, but cheaper than a wrong assumption built on a stale
  paraphrase.)
- Step-by-step recap of what you did. The next cycle does not need to know
  the order of operations; it needs to know the *current state*.
- Pleasantries, throat-clearing, framing language. Every token matters.

## Format

Open with `<carry_forward>` on its own line. Close with `</carry_forward>` on
its own line. No prose outside the tags. No nested tags. No code fences around
the block itself (you can use code fences inside if you need to quote a
specific snippet).

The `recall_archive` tool is available in the next cycle. It searches the
archived transcripts (BM25 over message text, top-N hits) when your briefing
missed something the next cycle needs. Use it sparingly — frequent recalls
mean your briefing was too sparse, so refine your *next* briefing rather than
leaning on the archive. Don't try to be exhaustive here: be precise about the
load-bearing state and trust the archive for the rest.

## Example shape (do not copy verbatim — write your own)

```
<carry_forward>
Working on issue #124 (cycle-restart). Key decisions: (1) trigger at 110K
tokens not 128K — need ~8.5K headroom for the briefing turn itself plus
next-turn growth before the next boundary; (2) archive to JSONL with a
header line so future tools can stream-read without parsing the whole
file. Constraint discovered: DeepSeek V4 thinking-mode requires
reasoning_content replay on assistant messages with tool calls — so seed
messages can't include orphan tool calls from the archived cycle. The
approach of "summarize then keep recent messages" (the old compaction
path) was failing because the model couldn't tell which fragments were
verbatim vs. paraphrased; replacing it entirely. Open question for user:
do they want per-model briefing token caps, or one global cap?
</carry_forward>
```

Now write your `<carry_forward>` for this conversation.
"#;

pub(crate) async fn maybe_run_preserve_turn(
    config: &Config,
    cwd: &Path,
    prompt_summary: &str,
) -> Result<(), String> {
    if config.harness.as_deref() != Some("deepseek-tui") {
        return Ok(());
    }

    let Some(base_url) = config.model_provider.base_url.as_deref() else {
        return Ok(());
    };
    let api_key = bearer_token(config)?;
    let request_body = json!({
        "model": DEEPSEEK_TUI_PRESERVE_MODEL,
        "messages": [
            {
                "role": "system",
                "content": DEEPSEEK_TUI_PRESERVE_SYSTEM,
            },
            {
                "role": "user",
                "content": preserve_user_message(cwd, prompt_summary),
            },
        ],
        "max_tokens": DEEPSEEK_TUI_PRESERVE_MAX_TOKENS,
        "temperature": DEEPSEEK_TUI_PRESERVE_TEMPERATURE,
    });

    let endpoint = format!("{}/chat/completions", base_url.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .post(endpoint)
        .bearer_auth(api_key)
        .json(&request_body)
        .send()
        .await
        .map_err(|err| err.to_string())?;

    if response.status().is_success() {
        Ok(())
    } else {
        Err(format!(
            "preserve request failed with HTTP {}",
            response.status()
        ))
    }
}

fn bearer_token(config: &Config) -> Result<String, String> {
    if let Some(token) = config.model_provider.experimental_bearer_token.as_deref()
        && !token.is_empty()
    {
        return Ok(token.to_string());
    }
    let env_key = config
        .model_provider
        .env_key
        .as_deref()
        .unwrap_or("DEEPSEEK_API_KEY");
    std::env::var(env_key).map_err(|_| format!("{env_key} is not set"))
}

fn preserve_user_message(cwd: &Path, prompt_summary: &str) -> String {
    if prompt_summary.contains("DEEPSEEK_TUI_TOOL_GAUNTLET_DONE") {
        return gauntlet_preserve_user_message(cwd);
    }
    let cwd = cwd.display();
    format!(
        "## Briefing Request\n\nProduce a <carry_forward> block summarizing the session state. Include: decisions made + why, constraints discovered, hypotheses being tested, approaches that failed, open questions. Do NOT include tool output bytes, file contents, or step-by-step recaps.\n\n## Structured State\n\n## Cycle State (Auto-Preserved)\n\n- Mode: `YOLO`\n- Workspace: `{cwd}`\n- Cwd: `{cwd}`\n\n### Work\n\nNo structured checklist is available for this cycle.\n\n## Repo Working Set\nWorkspace: {cwd}\nWhen in doubt, use tools to verify and keep changes focused on the working set.\n\n\nNo prior context summaries available. Produce a brief carry-forward from the structured state alone.\n"
    )
}

fn gauntlet_preserve_user_message(cwd: &Path) -> String {
    let cwd = cwd.display();
    format!(
        "## Briefing Request\n\nProduce a <carry_forward> block summarizing the session state. Include: decisions made + why, constraints discovered, hypotheses being tested, approaches that failed, open questions. Do NOT include tool output bytes, file contents, or step-by-step recaps.\n\n## Structured State\n\n## Cycle State (Auto-Preserved)\n\n- Mode: `YOLO`\n- Workspace: `{cwd}`\n- Cwd: `{cwd}`\n\n### Work\n\nChecklist (94% complete)\n- [x] update_plan with three short steps\n- [x] checklist_write with three same steps, first in_progress\n- [x] list_dir on workspace root\n- [x] read_file for module.py\n- [x] grep_files for NEEDLE_OLD\n- [x] file_search for module\n- [x] git_status\n- [x] git_diff\n- [x] diagnostics\n- [x] tool_search for editing/patching tools\n- [x] write_file: created_by_gauntlet.txt\n- [x] edit_file: replace NEEDLE_OLD with NEEDLE_NEW in module.py\n- [x] apply_patch: add PATCH_OK = True after VALUE line\n- [x] exec_shell: large output loop\n- [x] exec_shell: write shell_proof.txt\n- [x] read_file for module.py again\n- [x] final checklist_write marking all done\n- [~] final assistant message exactly DEEPSEEK_TUI_TOOL_GAUNTLET_DONE\n\nStrategy metadata\n- [~] Setup tracking and read workspace state\n- [ ] Create and modify workspace files\n- [ ] Shell commands and final verification\n\n## Repo Working Set\nWorkspace: {cwd}\nKey files: README.md\nActive paths (prioritize these):\n- module.py (file)\n- created_by_gauntlet.txt (file)\n- shell_proof.txt (file)\n- editing/patching (file)\n-  (dir)\n- 1/1 (file)\n- SHELL_OK/n (file)\n- a/module.py (file)\nWhen in doubt, use tools to verify and keep changes focused on the working set.\n\n\nNo prior context summaries available. Produce a brief carry-forward from the structured state alone.\n"
    )
}
