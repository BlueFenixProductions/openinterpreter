pub(super) const CLAUDE_READ_MAX_WHOLE_FILE_BYTES: usize = 256 * 1024;
pub(super) const CLAUDE_GLOB_MAX_RESULTS: usize = 100;
pub(super) const CLAUDE_GLOB_TRUNCATED_MESSAGE: &str =
    "(Results are truncated. Consider using a more specific path or pattern.)";

pub(super) fn claude_large_read_file_message(content_len: usize) -> String {
    format!(
        "File content ({}) exceeds maximum allowed size (256KB). Use offset and limit parameters to read specific portions of the file, or search for specific content instead of reading the whole file.",
        decimal_mb(content_len)
    )
}

pub(super) fn format_claude_glob_results(paths: Vec<String>) -> String {
    if paths.is_empty() {
        return "No files found".to_string();
    }

    let is_truncated = paths.len() > CLAUDE_GLOB_MAX_RESULTS;
    let mut lines = paths
        .into_iter()
        .take(CLAUDE_GLOB_MAX_RESULTS)
        .collect::<Vec<_>>();
    if is_truncated {
        lines.push(CLAUDE_GLOB_TRUNCATED_MESSAGE.to_string());
    }
    lines.join("\n")
}

fn decimal_mb(bytes: usize) -> String {
    format!("{:.1}MB", bytes as f64 / 1_048_576.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_results_truncate_after_captured_limit() {
        let paths = (1..=101)
            .map(|index| format!("glob-large/item_{index:05}.txt"))
            .collect::<Vec<_>>();

        let output = format_claude_glob_results(paths);

        assert!(output.contains("glob-large/item_00001.txt"));
        assert!(output.contains("glob-large/item_00100.txt"));
        assert!(!output.contains("glob-large/item_00101.txt"));
        assert!(output.ends_with(CLAUDE_GLOB_TRUNCATED_MESSAGE));
    }

    #[test]
    fn read_file_message_matches_captured_limit_text() {
        assert_eq!(
            claude_large_read_file_message(1_590_000),
            "File content (1.5MB) exceeds maximum allowed size (256KB). Use offset and limit parameters to read specific portions of the file, or search for specific content instead of reading the whole file."
        );
    }
}
