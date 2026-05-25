use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::handlers::claude_code::effective_turn_file_system_policy;
use crate::tools::handlers::claude_code::ensure_readable_path;
use crate::tools::handlers::claude_code::ensure_writable_path;
use crate::tools::handlers::claude_code::parse_absolute_path;
use crate::tools::registry::ToolHandler;
use crate::tools::registry::ToolKind;
use codex_utils_absolute_path::AbsolutePathBuf;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

pub struct SweAgentCommandHandler;

impl ToolHandler for SweAgentCommandHandler {
    type Output = FunctionToolOutput;

    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Custom { .. })
    }

    async fn is_mutating(&self, invocation: &ToolInvocation) -> bool {
        let ToolPayload::Custom { input } = &invocation.payload else {
            return true;
        };
        !matches!(
            parse_swe_agent_command(input),
            Ok(SweAgentCommand::Editor(SweEditorCommand::View { .. }))
        )
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<Self::Output, FunctionCallError> {
        let ToolInvocation {
            session,
            turn,
            payload,
            ..
        } = invocation;
        let ToolPayload::Custom { input } = payload else {
            return Err(FunctionCallError::RespondToModel(
                "swe_agent_command received unsupported payload".to_string(),
            ));
        };
        let command = parse_swe_agent_command(&input)?;
        let file_system_policy =
            effective_turn_file_system_policy(session.as_ref(), turn.as_ref()).await;

        let output = match command {
            SweAgentCommand::Editor(command) => match command {
                SweEditorCommand::View { path } => {
                    ensure_readable_path(&file_system_policy, turn.as_ref(), &path)?;
                    if tokio::fs::metadata(path.as_path())
                        .await
                        .map(|metadata| metadata.is_dir())
                        .unwrap_or(false)
                    {
                        format_directory_view(&path).await?
                    } else {
                        let content =
                            tokio::fs::read_to_string(path.as_path())
                                .await
                                .map_err(|err| {
                                    FunctionCallError::RespondToModel(format!("view failed: {err}"))
                                })?;
                        format_file_view(&path, &content)
                    }
                }
                SweEditorCommand::Create { path, file_text } => {
                    ensure_writable_path(&file_system_policy, turn.as_ref(), &path)?;
                    if let Some(parent) = path.parent() {
                        tokio::fs::create_dir_all(parent.as_path())
                            .await
                            .map_err(|err| {
                                FunctionCallError::RespondToModel(format!("create failed: {err}"))
                            })?;
                    }
                    tokio::fs::write(path.as_path(), file_text)
                        .await
                        .map_err(|err| {
                            FunctionCallError::RespondToModel(format!("create failed: {err}"))
                        })?;
                    format!("File created successfully at: {}\n", path.display())
                }
                SweEditorCommand::StrReplace {
                    path,
                    old_str,
                    new_str,
                } => {
                    ensure_readable_path(&file_system_policy, turn.as_ref(), &path)?;
                    ensure_writable_path(&file_system_policy, turn.as_ref(), &path)?;
                    let content =
                        tokio::fs::read_to_string(path.as_path())
                            .await
                            .map_err(|err| {
                                FunctionCallError::RespondToModel(format!(
                                    "str_replace failed: {err}"
                                ))
                            })?;
                    let updated = content.replacen(&old_str, &new_str, 1);
                    tokio::fs::write(path.as_path(), updated.as_bytes())
                        .await
                        .map_err(|err| {
                            FunctionCallError::RespondToModel(format!("str_replace failed: {err}"))
                        })?;
                    format!(
                        "The file {} has been edited. Here's the result of running `cat -n` on a snippet of {}:\n{}Review the changes and make sure they are as expected. Edit the file again if necessary.\n",
                        path.display(),
                        path.display(),
                        cat_numbered(&updated)
                    )
                }
                SweEditorCommand::Insert {
                    path,
                    insert_line,
                    new_str,
                } => {
                    ensure_readable_path(&file_system_policy, turn.as_ref(), &path)?;
                    ensure_writable_path(&file_system_policy, turn.as_ref(), &path)?;
                    let content =
                        tokio::fs::read_to_string(path.as_path())
                            .await
                            .map_err(|err| {
                                FunctionCallError::RespondToModel(format!("insert failed: {err}"))
                            })?;
                    let updated = insert_line_after(&content, insert_line, &new_str)?;
                    tokio::fs::write(path.as_path(), updated.as_bytes())
                        .await
                        .map_err(|err| {
                            FunctionCallError::RespondToModel(format!("insert failed: {err}"))
                        })?;
                    format!(
                        "The file {} has been edited. Here's the result of running `cat -n` on a snippet of the edited file:\n{}Review the changes and make sure they are as expected (correct indentation, no duplicate lines, etc). Edit the file again if necessary.\n",
                        path.display(),
                        cat_numbered(&updated)
                    )
                }
                SweEditorCommand::UndoEdit { path } => {
                    ensure_writable_path(&file_system_policy, turn.as_ref(), &path)?;
                    format!("No edit history found for {}.\n", path.display())
                }
            },
            SweAgentCommand::Submit => format_submit_output(turn.cwd.as_path()).await?,
        };

        Ok(FunctionToolOutput::from_text(output, Some(true)))
    }
}

enum SweAgentCommand {
    Editor(SweEditorCommand),
    Submit,
}

enum SweEditorCommand {
    View {
        path: AbsolutePathBuf,
    },
    Create {
        path: AbsolutePathBuf,
        file_text: String,
    },
    StrReplace {
        path: AbsolutePathBuf,
        old_str: String,
        new_str: String,
    },
    Insert {
        path: AbsolutePathBuf,
        insert_line: usize,
        new_str: String,
    },
    UndoEdit {
        path: AbsolutePathBuf,
    },
}

fn parse_swe_agent_command(input: &str) -> Result<SweAgentCommand, FunctionCallError> {
    let parts = shlex::split(input).ok_or_else(|| {
        FunctionCallError::RespondToModel("failed to parse SWE-agent command".to_string())
    })?;
    match parts.as_slice() {
        [command] if command == "submit" => Ok(SweAgentCommand::Submit),
        [tool, command, path, rest @ ..] if tool == "str_replace_editor" => {
            let path = parse_absolute_path(path)?;
            let command = match command.as_str() {
                "view" => SweEditorCommand::View { path },
                "create" => SweEditorCommand::Create {
                    path,
                    file_text: required_option(rest, "--file_text")?,
                },
                "str_replace" => SweEditorCommand::StrReplace {
                    path,
                    old_str: required_option(rest, "--old_str")?,
                    new_str: required_option(rest, "--new_str")?,
                },
                "insert" => SweEditorCommand::Insert {
                    path,
                    insert_line: required_option(rest, "--insert_line")?
                        .parse::<usize>()
                        .map_err(|err| {
                            FunctionCallError::RespondToModel(format!("invalid insert_line: {err}"))
                        })?,
                    new_str: required_option(rest, "--new_str")?,
                },
                "undo_edit" => SweEditorCommand::UndoEdit { path },
                other => {
                    return Err(FunctionCallError::RespondToModel(format!(
                        "unsupported str_replace_editor command: {other}"
                    )));
                }
            };
            Ok(SweAgentCommand::Editor(command))
        }
        _ => Err(FunctionCallError::RespondToModel(format!(
            "unsupported SWE-agent command: {input}"
        ))),
    }
}

fn required_option(args: &[String], name: &str) -> Result<String, FunctionCallError> {
    let index = args.iter().position(|arg| arg == name).ok_or_else(|| {
        FunctionCallError::RespondToModel(format!("missing required option {name}"))
    })?;
    args.get(index + 1).cloned().ok_or_else(|| {
        FunctionCallError::RespondToModel(format!("missing value for option {name}"))
    })
}

async fn format_directory_view(path: &AbsolutePathBuf) -> Result<String, FunctionCallError> {
    let mut entries = vec![path.display().to_string()];
    collect_directory_entries(path.as_path(), &mut entries).await?;
    Ok(format!(
        "Here's the files and directories up to 2 levels deep in {}, excluding hidden items:\n{}\n\n\n",
        path.display(),
        entries.join("\n")
    ))
}

async fn collect_directory_entries(
    path: &Path,
    entries: &mut Vec<String>,
) -> Result<(), FunctionCallError> {
    let mut pending = vec![(path.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = pending.pop() {
        if depth >= 2 {
            continue;
        }
        let mut read_dir = tokio::fs::read_dir(&dir)
            .await
            .map_err(|err| FunctionCallError::RespondToModel(format!("view failed: {err}")))?;
        let mut children = Vec::new();
        while let Some(entry) = read_dir
            .next_entry()
            .await
            .map_err(|err| FunctionCallError::RespondToModel(format!("view failed: {err}")))?
        {
            if entry.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            children.push(entry.path());
        }
        children.sort();
        let mut dirs = Vec::new();
        for child in children {
            if child.is_dir() {
                dirs.push(child.clone());
            }
            entries.push(child.display().to_string());
        }
        for dir in dirs.into_iter().rev() {
            pending.push((dir, depth + 1));
        }
    }
    Ok(())
}

fn format_file_view(path: &AbsolutePathBuf, content: &str) -> String {
    format!(
        "Here's the result of running `cat -n` on {}:\n{}\n",
        path.display(),
        cat_numbered(content)
    )
}

fn cat_numbered(content: &str) -> String {
    let lines = content.lines().collect::<Vec<_>>();
    let lines = if lines.is_empty() { vec![""] } else { lines };
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| format!("{:>6}\t{line}\n", index + 1))
        .collect::<String>()
}

fn insert_line_after(
    content: &str,
    insert_line: usize,
    new_str: &str,
) -> Result<String, FunctionCallError> {
    let mut lines = content.lines().map(str::to_string).collect::<Vec<_>>();
    if insert_line > lines.len() {
        return Err(FunctionCallError::RespondToModel(format!(
            "insert failed: insert_line {insert_line} is past end of file"
        )));
    }
    lines.insert(insert_line, new_str.to_string());
    Ok(lines.join("\n"))
}

async fn format_submit_output(cwd: &Path) -> Result<String, FunctionCallError> {
    let cwd = cwd.to_path_buf();
    let diff = tokio::task::spawn_blocking(move || submit_diff(&cwd))
        .await
        .map_err(|err| FunctionCallError::RespondToModel(format!("submit failed: {err}")))?
        .map_err(FunctionCallError::RespondToModel)?;
    Ok(format!(
        "Thank you for your work on this issue. Please carefully follow the steps below to help review your changes.\n\n1. If you made any changes to your code after running the reproduction script, please run the reproduction script again.\n  If the reproduction script is failing, please revisit your changes and make sure they are correct.\n  If you have already removed your reproduction script, please ignore this step.\n2. Remove your reproduction script (if you haven't done so already).\n3. If you have modified any TEST files, please revert them to the state they had before you started fixing the issue.\n  You can do this with `git checkout -- /path/to/test/file.py`. Use below <diff> to find the files you need to revert.\n4. Run the submit command again to confirm.\n\nHere is a list of all of your changes:\n\n<diff>\n{diff}</diff>\n\n"
    ))
}

fn submit_diff(cwd: &Path) -> Result<String, String> {
    let mut paths = Vec::new();
    for path in git_lines(cwd, ["diff", "--name-only"])? {
        paths.push((path, false));
    }
    for path in git_lines(cwd, ["ls-files", "--others", "--exclude-standard"])? {
        paths.push((path, true));
    }
    paths.sort_by(|(left, _), (right, _)| left.cmp(right));

    let mut diff = String::new();
    for (path, is_untracked) in paths {
        if is_untracked {
            diff.push_str(&format_untracked_file_diff(cwd, &path)?);
        } else {
            diff.push_str(&git_stdout(
                cwd,
                ["diff", "--no-ext-diff", "--", path.as_str()],
            )?);
        }
    }
    Ok(diff)
}

fn git_lines<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<Vec<String>, String> {
    let stdout = git_stdout(cwd, args)?;
    Ok(stdout.lines().map(str::to_string).collect())
}

fn git_stdout<const N: usize>(cwd: &Path, args: [&str; N]) -> Result<String, String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|err| format!("submit failed: {err}"))?;
    if !output.status.success() {
        return Err(format!(
            "submit failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn format_untracked_file_diff(cwd: &Path, path: &str) -> Result<String, String> {
    let file_path = cwd.join(path);
    let content =
        std::fs::read_to_string(&file_path).map_err(|err| format!("submit failed: {err}"))?;
    let mode = file_mode(&file_path)?;
    let hash = git_stdout(cwd, ["hash-object", "--no-filters", path])?;
    let hash = hash.trim();
    let line_count = content.lines().count().max(1);
    let range = if line_count == 1 {
        "1".to_string()
    } else {
        format!("1,{line_count}")
    };

    let mut diff = format!(
        "diff --git a/{path} b/{path}\nnew file mode {mode}\nindex 0000000..{}\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +{range} @@\n",
        &hash[..7]
    );
    let has_trailing_newline = content.ends_with('\n');
    let lines = content.lines().collect::<Vec<_>>();
    if lines.is_empty() {
        diff.push_str("+\n");
    } else {
        for line in lines {
            diff.push('+');
            diff.push_str(line);
            diff.push('\n');
        }
    }
    if !has_trailing_newline {
        diff.push_str("\\ No newline at end of file\n");
    } else {
        diff.push('\n');
    }
    Ok(diff)
}

#[cfg(unix)]
fn file_mode(path: &PathBuf) -> Result<&'static str, String> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(path).map_err(|err| format!("submit failed: {err}"))?;
    if metadata.permissions().mode() & 0o111 == 0 {
        Ok("100644")
    } else {
        Ok("100755")
    }
}

#[cfg(not(unix))]
fn file_mode(path: &PathBuf) -> Result<&'static str, String> {
    let _ = std::fs::metadata(path).map_err(|err| format!("submit failed: {err}"))?;
    Ok("100644")
}
