#[derive(Debug, Clone, Eq, PartialEq, Default)]
pub enum Harness {
    #[default]
    Native,
    ClaudeCode,
    ClaudeCodeBare,
    DeepSeekTui,
    KimiCli,
    QwenCode,
    SweAgent,
    Minimal,
    Other(String),
}

impl Harness {
    pub fn from_config_name(name: Option<&str>) -> Self {
        match name {
            None | Some("") => Self::Native,
            Some("claude-code") => Self::ClaudeCode,
            Some("claude-code-bare") => Self::ClaudeCodeBare,
            Some("deepseek-tui") => Self::DeepSeekTui,
            Some("kimi-cli") => Self::KimiCli,
            Some("qwen-code") => Self::QwenCode,
            Some("swe-agent") => Self::SweAgent,
            Some("minimal") => Self::Minimal,
            Some(other) => Self::Other(other.to_string()),
        }
    }

    pub fn is_claude_code(&self) -> bool {
        matches!(self, Self::ClaudeCode | Self::ClaudeCodeBare)
    }

    pub fn is_claude_code_bare(&self) -> bool {
        matches!(self, Self::ClaudeCodeBare)
    }

    pub fn is_kimi_cli(&self) -> bool {
        matches!(self, Self::KimiCli)
    }

    pub fn is_deepseek_tui(&self) -> bool {
        matches!(self, Self::DeepSeekTui)
    }

    pub fn is_qwen_code(&self) -> bool {
        matches!(self, Self::QwenCode)
    }

    pub fn is_swe_agent(&self) -> bool {
        matches!(self, Self::SweAgent)
    }

    pub fn is_minimal(&self) -> bool {
        matches!(self, Self::Minimal)
    }
}

#[cfg(test)]
mod tests {
    use super::Harness;
    use pretty_assertions::assert_eq;

    #[test]
    fn from_config_name_parses_known_harnesses() {
        assert_eq!(Harness::from_config_name(None), Harness::Native);
        assert_eq!(
            Harness::from_config_name(Some("claude-code")),
            Harness::ClaudeCode
        );
        assert_eq!(
            Harness::from_config_name(Some("claude-code-bare")),
            Harness::ClaudeCodeBare
        );
        assert_eq!(
            Harness::from_config_name(Some("deepseek-tui")),
            Harness::DeepSeekTui
        );
        assert_eq!(
            Harness::from_config_name(Some("kimi-cli")),
            Harness::KimiCli
        );
        assert_eq!(
            Harness::from_config_name(Some("qwen-code")),
            Harness::QwenCode
        );
        assert_eq!(
            Harness::from_config_name(Some("swe-agent")),
            Harness::SweAgent
        );
        assert_eq!(Harness::from_config_name(Some("minimal")), Harness::Minimal);
    }
}
