# Open Interpreter CLI

## Critical CLI Interface Contract

`interpreter` is the user-facing Open Interpreter command and must remain a
drop-in replacement for the Codex CLI surface. We are not redesigning upstream
Codex CLI usage under a different name: subcommands, flags, and non-interactive
flows should preserve the behavior users would expect from the corresponding
Codex command under the Open Interpreter name.

The intentional exception is bare interactive startup: running `interpreter`
without a subcommand starts Open Interpreter's app-server-backed interactive TUI.
On first interactive use, that path starts the local app-server daemon and then
connects the TUI through it.
`interpreter exec` and the standalone `interpreter-exec` binary both use the
same non-interactive exec implementation.
