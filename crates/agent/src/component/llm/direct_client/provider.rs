// ============================================================================
// LLM Provider 枚举（OpenClaw / OpenAI Compatible / Ollama，全部走 OpenAI 兼容接口）
// ============================================================================

/// LLM Provider 类型
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LlmProvider {
    /// 使用宿主 OpenClaw 已配置（通过 OpenClaw Gateway）
    OpenClaw,
    /// 兼容 OpenAI 接口（需要手动指定 URL 和模型）
    OpenAICompatible,
    /// Ollama 本地部署
    Ollama,
}

impl LlmProvider {
    /// 获取 provider 的字符串表示
    pub fn as_str(&self) -> &str {
        match self {
            LlmProvider::OpenClaw => "openclaw",
            LlmProvider::OpenAICompatible => "openai_compatible",
            LlmProvider::Ollama => "ollama",
        }
    }

    /// 从字符串解析
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "openclaw" => Some(Self::OpenClaw),
            "openai_compatible" | "openai-compatible" => Some(Self::OpenAICompatible),
            "ollama" => Some(Self::Ollama),
            _ => None,
        }
    }

    /// 默认 Base URL（如果有的话）
    pub(super) fn default_base_url(&self) -> Option<&'static str> {
        match self {
            Self::OpenClaw => None,         // 从配置文件读取
            Self::OpenAICompatible => None, // 必须手动指定
            Self::Ollama => Some("http://localhost:11434/v1"),
        }
    }

    /// 默认模型（如果有的话）
    pub(super) fn default_model(&self) -> Option<&'static str> {
        match self {
            Self::OpenClaw => None,         // 从配置文件读取
            Self::OpenAICompatible => None, // 必须手动指定
            Self::Ollama => None,           // 不指定默认模型
        }
    }

    /// 是否需要 API Key
    pub fn requires_api_key(&self) -> bool {
        match self {
            Self::OpenClaw => true, // OpenClaw 读取 Gateway 配置，但 API Key 需用户输入
            Self::OpenAICompatible => true, // OpenAI 兼容接口通常需要 key
            Self::Ollama => false,  // Ollama 本地通常不需要
        }
    }

    /// 是否需要手动指定 Base URL
    pub fn requires_base_url(&self) -> bool {
        matches!(self, Self::OpenAICompatible)
    }

    /// 是否需要手动指定模型
    pub fn requires_model(&self) -> bool {
        matches!(self, Self::OpenAICompatible)
    }

    /// 所有 Provider 变体（用于 UI 下拉等枚举场景）
    pub const ALL: &[LlmProvider] = &[
        LlmProvider::Ollama,
        LlmProvider::OpenClaw,
        LlmProvider::OpenAICompatible,
    ];

    /// UI 显示标签
    pub fn display_label(&self) -> &str {
        match self {
            Self::Ollama => "Ollama",
            Self::OpenClaw => "OpenClaw Gateway",
            Self::OpenAICompatible => "OpenAI Compatible",
        }
    }

    /// 是否从配置文件读取
    pub fn reads_from_config(&self) -> bool {
        matches!(self, Self::OpenClaw)
    }
}
