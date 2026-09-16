// ============================================================================
// 角色配置（CharacterConfig / LanguageStyleConfig / GoalsConfig）
// ============================================================================

use super::*;

// ============================================================================
// 角色配置（侠客）
// ============================================================================

/// 语言风格配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LanguageStyleConfig {
    /// 语调：豪迈/温和/冷漠/狡黠
    #[serde(default)]
    pub tone: Option<String>,

    /// 说话特点
    #[serde(default)]
    pub speech_patterns: Vec<String>,
}

/// 角色目标配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GoalsConfig {
    /// 短期目标
    #[serde(default)]
    pub short_term: Option<String>,

    /// 长远目标
    #[serde(default)]
    pub long_term: Option<String>,
}

/// 角色状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CharacterStatus {
    /// 存活
    #[default]
    Alive,
    /// 死亡
    Dead,
    /// 归隐（转生）
    Retired,
}

/// 角色配置（侠客）
///
/// 通过 Web 面板或 HTTP API 创建。
/// 角色死亡后可以转世，此时 agent_id 会变化。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CharacterConfig {
    /// 服务器分配的角色 ID（注册后由服务器返回）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<Uuid>,

    // === 基本信息 ===
    /// 姓名
    pub name: String,

    /// 年龄
    #[serde(default = "default_age")]
    pub age: u8,

    /// 性别
    #[serde(default = "default_gender")]
    pub gender: String,

    /// 外貌描述
    #[serde(default)]
    pub appearance: Option<String>,

    /// 身份背景（如：江湖游侠、商人、书生）
    #[serde(default)]
    pub identity: Option<String>,

    // === 性格特征 ===
    #[serde(default)]
    pub personality: Vec<String>,

    // === 核心价值观 ===
    #[serde(default)]
    pub values: Vec<String>,

    // === 语言风格 ===
    #[serde(default)]
    pub language_style: LanguageStyleConfig,

    // === 当前目标 ===
    #[serde(default)]
    pub goals: GoalsConfig,

    // === 系统提示词（自动生成或自定义） ===
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    // === 注册时服务器返回的信息 ===
    /// 注册时间（注册成功时记录）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registered_at: Option<chrono::DateTime<chrono::Utc>>,

    /// 先天属性（注册时从服务器获取，用于对比成长）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub birth_attributes: Option<std::collections::HashMap<String, i32>>,

    // === 服务器关联 ===
    /// 所属服务器的 HTTP URL（用于区分不同服务器的角色）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,

    /// 最近一次连接时的现实时间
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_connected_real_time: Option<chrono::DateTime<chrono::Utc>>,

    /// 最近一次连接时的游戏时间
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_connected_world_time: Option<cyber_jianghu_protocol::WorldTime>,

    /// 角色状态
    #[serde(default)]
    pub status: CharacterStatus,

    /// 纪传体传记（死亡/归隐时由 LLM 生成，汇总经历日志）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub biography: Option<String>,
}

fn default_age() -> u8 {
    25
}

fn default_gender() -> String {
    "男".to_string()
}

impl CharacterConfig {
    /// 生成系统提示词
    ///
    /// 如果用户没有提供自定义 system_prompt，则根据角色信息自动生成
    pub fn generate_system_prompt(&self) -> String {
        if let Some(ref prompt) = self.system_prompt {
            return prompt.clone();
        }

        let mut parts = vec![];

        // 基本信息
        parts.push(format!(
            "你是{}，一位{}岁的{}。",
            self.name, self.age, self.gender
        ));

        // 外貌
        if let Some(ref appearance) = self.appearance {
            parts.push(format!("外貌：{}。", appearance));
        }

        // 身份
        if let Some(ref identity) = self.identity {
            parts.push(format!("身份：{}。", identity));
        }

        // 性格
        if !self.personality.is_empty() {
            parts.push(format!("性格：{}。", self.personality.join("、")));
        }

        // 价值观
        if !self.values.is_empty() {
            parts.push(format!("核心价值观：{}。", self.values.join("；")));
        }

        // 语言风格
        if let Some(ref tone) = self.language_style.tone {
            parts.push(format!("说话风格{}。", tone));
        }
        if !self.language_style.speech_patterns.is_empty() {
            parts.push(format!(
                "语言特点：{}。",
                self.language_style.speech_patterns.join("，")
            ));
        }

        // 目标
        if let Some(ref short_term) = self.goals.short_term {
            parts.push(format!("当前目标：{}。", short_term));
        }
        if let Some(ref long_term) = self.goals.long_term {
            parts.push(format!("长远目标：{}。", long_term));
        }

        parts.join("\n")
    }

    /// 检查角色是否已注册
    pub fn is_registered(&self) -> bool {
        self.agent_id.is_some()
    }

    /// 从文件加载角色配置
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read character config from {:?}", path.as_ref()))?;
        serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse character config from {:?}", path.as_ref()))
    }

    /// 保存角色配置到文件（原子写入：先写临时文件再 rename）
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let content =
            serde_yaml::to_string(self).context("Failed to serialize character config")?;
        let path = path.as_ref();
        let tmp_path = path.with_extension("tmp");
        std::fs::write(&tmp_path, &content)
            .with_context(|| format!("Failed to write character config to {:?}", tmp_path))?;
        std::fs::rename(&tmp_path, path)
            .with_context(|| format!("Failed to rename character config at {:?}", path))?;
        Ok(())
    }
}
