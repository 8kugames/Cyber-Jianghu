// ============================================================================
// 子系统配置集合（记忆/token 优化/跳帧/反思/注意力/角色生成/更新等 + AgentConfig）
// ============================================================================

use super::*;

// ============================================================================
// 记忆系统配置
// ============================================================================

/// 记忆系统配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// 是否启用记忆系统
    #[serde(default = "default_memory_enabled")]
    pub enabled: bool,

    /// 工作记忆容量（保留最近 N 条事件）
    #[serde(default = "default_working_memory_size")]
    pub working_memory_size: usize,

    /// 情景记忆保存阈值（重要性 >= 此值的事件会被保存）
    #[serde(default = "default_episodic_threshold")]
    pub episodic_threshold: f32,

    /// 遗忘机制运行间隔（tick 数）
    /// 基于 tick_duration=60s 时，84 ticks ≈ 84 分钟
    #[serde(default = "default_forgetting_interval_ticks")]
    pub forgetting_interval_ticks: i64,

    /// 艾宾浩斯遗忘曲线参数（可选，不填则用默认值）
    /// R = e^(-decay_rate * ticks / strength)
    #[serde(default)]
    pub ebbinghaus: Option<EbbinghausConfig>,

    /// OutcomeMemory 每种 action 的 prompt 注入条数上限
    #[serde(default = "default_outcome_prompt_limit")]
    pub outcome_prompt_limit: usize,

    /// OutcomeMemory 最大记录数（FIFO 清理）
    #[serde(default = "default_outcome_max_records")]
    pub outcome_max_records: usize,
}

fn default_memory_enabled() -> bool {
    true
}

fn default_working_memory_size() -> usize {
    20
}

fn default_episodic_threshold() -> f32 {
    0.3
}

fn default_forgetting_interval_ticks() -> i64 {
    84
}

fn default_outcome_prompt_limit() -> usize {
    10
}

fn default_outcome_max_records() -> usize {
    1000
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            working_memory_size: 20,
            episodic_threshold: 0.3,
            forgetting_interval_ticks: 84,
            ebbinghaus: None,
            outcome_prompt_limit: 10,
            outcome_max_records: 1000,
        }
    }
}

// ============================================================================
// Token 优化配置
// ============================================================================

/// Token 优化总开关与子模块配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TokenOptimizationConfig {
    /// 总开关（默认开启）
    #[serde(default = "default_token_opt_enabled")]
    pub enabled: bool,
    /// ReflectorSoul 优化：消灭重试循环
    pub reflector: ReflectorOptConfig,
    /// Attention Controller
    pub attention: AttentionConfig,
    /// Delta Engine
    pub delta: DeltaConfig,
    /// 空转跳过（delta 无显著变化时跳过认知循环，昼夜节律默认开启）
    pub idle_skip: IdleSkipConfig,
}

impl Default for TokenOptimizationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            reflector: ReflectorOptConfig::default(),
            attention: AttentionConfig::default(),
            delta: DeltaConfig::default(),
            idle_skip: IdleSkipConfig::default(),
        }
    }
}

/// 空转跳过配置
///
/// delta 无 Important/Critical 级变化时跳过本 tick 认知循环（零 LLM 消耗）。
/// 生存属性变化、实体出现、新事件、活跃对话会话均会唤醒思考（v1 保守豁免层级）。
/// 昼夜节律：夜间抑制 Info 空转（无 whim），黎明无条件唤醒，白天按 1/N 概率保留 whim。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IdleSkipConfig {
    /// 是否启用（默认开启：昼夜节律空转跳过随部署默认生效；置 false 可整体关闭）
    pub enabled: bool,
    /// 连续跳过上限（任何时段兜底，夜间同样受约束），达到后强制执行一次认知循环（防长眠；也兼兑 night_hours 误配为全天）
    pub max_consecutive_skips: usize,
    /// 白天全 Info 空转时按 1/N 概率照常思考（保留自发性 whim；<=1 表示空转全部思考）
    pub whim_wake_divisor: usize,
    /// 昼夜节律配置
    pub night: NightSkipConfig,
}

impl Default for IdleSkipConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_consecutive_skips: 4,
            whim_wake_divisor: 2,
            night: NightSkipConfig::default(),
        }
    }
}

/// 夜间节律配置
///
/// 夜间时段：Info 空转跳过（无 whim；跳过上限兜底仍适用），黎明第一个 tick 无条件完整思考。
/// Important 级变化夜间仍然唤醒（v1 保守：不制造记忆盲区）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NightSkipConfig {
    /// 昼夜节律是否启用（idle_skip.enabled 开启后随父开关生效）
    pub enabled: bool,
    /// 夜间游戏小时列表（hours_per_day=12，hour 取值 0..=11，支持跨零点；i32 对齐 WorldTime.hour）
    pub night_hours: Vec<i32>,
}

impl Default for NightSkipConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            night_hours: vec![9, 10, 11, 0, 1, 2],
        }
    }
}

/// ReflectorSoul 优化配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ReflectorOptConfig {
    /// 启用 self-correction：被驳回后调用 LLM 纠正一次
    pub self_correction: bool,
    /// 双重拒绝后直接 chaos_fallback（不再重试）
    pub chaos_on_double_reject: bool,
    /// self-correction LLM 失败累计达到此值后，跳过 self_correct 直接 chaos_fallback
    pub chaos_on_llm_fail: u32,
    /// layer0 空 item_id 且背包唯一候选时自动回填（零 token 自愈，
    /// 默认关闭：存在语义偏移风险——模型想用的未必是唯一候选）
    pub auto_fill_unique_item: bool,
}

/// Attention Controller 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AttentionConfig {
    pub max_focus_items: usize,
    pub first_tick_focus_cap: usize,
    pub critical_auto_include: bool,
}

/// Delta Engine 配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DeltaConfig {
    pub change_percentage_threshold: f32,
    /// 生存驱动 Critical 阈值（>= 此值标 Critical，< 此值标 Important）
    pub survival_critical_urgency_threshold: u8,
}

impl Default for ReflectorOptConfig {
    fn default() -> Self {
        Self {
            self_correction: true,
            chaos_on_double_reject: true,
            chaos_on_llm_fail: 2,
            auto_fill_unique_item: false,
        }
    }
}

impl Default for AttentionConfig {
    fn default() -> Self {
        Self {
            // 5 → 3：上限只约束 Info 级候选（Important/Critical/社交目标均强制包含），
            // 3 已足够覆盖无序环境噪声，同时显著减少 volatile prompt 体积
            max_focus_items: 3,
            first_tick_focus_cap: 15,
            critical_auto_include: true,
        }
    }
}

impl Default for DeltaConfig {
    fn default() -> Self {
        Self {
            change_percentage_threshold: 0.1,
            survival_critical_urgency_threshold: 5,
        }
    }
}

/// 角色生成约束 -- schema 驱动的单一数据源。
///
/// YAML 定义字段规格（path + type + constraints），prompt 和 validate 均从同一 schema 动态生成。
/// 添加/修改约束只需改 YAML，零代码改动。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CharacterGenerationConfig {
    pub world_setting: String,
    pub fields: Vec<FieldSpec>,
}

/// 字段约束类型 -- 4 种覆盖当前所有字段
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FieldConstraints {
    String {
        required: bool,
        #[serde(default)]
        min_chars: usize,
        max_chars: usize,
        #[serde(default)]
        prompt_text: Option<String>,
    },
    Integer {
        required: bool,
        min: u32,
        max: u32,
    },
    Enum {
        required: bool,
        options: Vec<String>,
        #[serde(default)]
        prompt_text: Option<String>,
    },
    EnumArray {
        required: bool,
        options: Vec<String>,
        min_count: usize,
        max_count: usize,
        #[serde(default)]
        extra_prompt: Option<String>,
    },
}

/// 字段规格 -- path 支持 dot notation（如 language_style.tone）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldSpec {
    pub path: String,
    #[serde(flatten)]
    pub constraints: FieldConstraints,
}

// ============================================================================
// 自动更新配置（GitHub Release 自更新，逻辑见 infra/updater.rs）
// ============================================================================

fn default_update_enabled() -> bool {
    true
}

fn default_update_auto_apply() -> bool {
    true
}

fn default_update_check_interval_secs() -> u64 {
    // 6 小时，叠加后台任务的随机抖动错峰
    21_600
}

fn default_update_repo() -> String {
    "8kugames/Cyber-Jianghu".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UpdateConfig {
    /// 后台自动更新总开关（关闭后不再周期检查，CLI/HTTP 手动触发仍可用）
    #[serde(default = "default_update_enabled")]
    pub enabled: bool,

    /// 发现新版本后是否自动下载安装并重启；false = 仅检查并记录日志
    #[serde(default = "default_update_auto_apply")]
    pub auto_apply: bool,

    /// 检查间隔（秒），运行时下限钳位 600s
    #[serde(default = "default_update_check_interval_secs")]
    pub check_interval_secs: u64,

    /// 更新源仓库（owner/repo，须为公开仓库的 GitHub Release）
    #[serde(default = "default_update_repo")]
    pub repo: String,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: default_update_enabled(),
            auto_apply: default_update_auto_apply(),
            check_interval_secs: default_update_check_interval_secs(),
            repo: default_update_repo(),
        }
    }
}

// ============================================================================
// 完整配置
// ============================================================================

/// 完整配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// 服务器配置
    #[serde(default)]
    pub server: ServerConfig,

    /// 运行时配置
    #[serde(default)]
    pub runtime: RuntimeConfig,

    /// LLM 配置（Cognitive 模式直连 LLM，Claw 模式通过 OpenClawBridge）
    #[serde(default)]
    pub llm: LlmConfig,

    /// ReflectorSoul LLM 配置（可选，未配置时继承 llm）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_reflector: Option<LlmConfig>,

    /// 记忆系统配置
    #[serde(default)]
    pub memory: MemoryConfig,

    /// 游戏规则（从服务器获取）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_rules: Option<GameRules>,

    /// 配置文件路径（运行时设置，不序列化）
    #[serde(skip)]
    pub config_path: PathBuf,

    /// 服务器数据目录
    /// 默认 ~/.cyber-jianghu/servers/
    #[serde(default)]
    pub servers_dir: PathBuf,

    /// 地魂（EarthSoul）配置 — tool result 预算 & 循环检测
    #[serde(default)]
    pub earth_soul: crate::soul::earth::config::EarthSoulConfig,

    /// Token 优化配置（总开关默认开启；serde 缺省与代码 Default 一致）
    #[serde(default)]
    pub token_optimization: TokenOptimizationConfig,

    /// 自动更新配置（GitHub Release 自更新）
    #[serde(default)]
    pub update: UpdateConfig,

    /// 角色生成约束（必填，缺失时 serde 报错 fail-fast）
    pub character_generation: CharacterGenerationConfig,
}

impl Config {
    /// 从文件加载配置
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path_display = path.as_ref().display().to_string();
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read config file: {}", path_display))?;

        let config: Config =
            serde_yaml::from_str(&content).with_context(|| "Failed to parse config file")?;

        Ok(config)
    }

    /// 保存配置到文件（原子写入：先写临时文件，再 rename 替换）
    ///
    /// 避免进程中断时文件被截断为空。
    pub fn save_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path = path.as_ref();
        let path_display = path.display().to_string();
        let yaml =
            serde_yaml::to_string(self).with_context(|| "Failed to serialize config to YAML")?;

        // 确保目录存在
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("Failed to create config directory: {}", parent.display())
            })?;
        }

        // 原子写入：先写临时文件，再 rename
        let tmp_path = path.with_extension("tmp");
        fs::write(&tmp_path, &yaml)
            .with_context(|| format!("Failed to write temp config file: {}", tmp_path.display()))?;

        if let Err(e) = fs::rename(&tmp_path, path) {
            let _ = fs::remove_file(&tmp_path);
            anyhow::bail!("Failed to replace config file {}: {}", path_display, e);
        }

        Ok(())
    }

    /// 获取重生延迟 tick 数（0 = 不自动重生）
    pub fn rebirth_delay_ticks(&self) -> i32 {
        self.game_rules
            .as_ref()
            .map(|r| r.rebirth_delay_ticks)
            .unwrap_or(0)
    }

    /// 更新游戏规则
    pub fn update_game_rules(&mut self, game_rules: GameRules) {
        // 保存 available_actions 到本地文件
        let cdir = config_dir();
        let actions_path = cdir.join("actions.json");

        // 确保目录存在
        if let Err(e) = fs::create_dir_all(&cdir) {
            tracing::warn!("创建配置目录失败: {}", e);
        } else {
            // 序列化并保存
            match serde_json::to_string_pretty(&game_rules.available_actions) {
                Ok(json) => {
                    if let Err(e) = fs::write(&actions_path, json) {
                        tracing::warn!("保存 actions.json 失败: {}", e);
                    } else {
                        tracing::debug!(
                            "已保存 {} 个动作到 {:?}",
                            game_rules.available_actions.len(),
                            actions_path
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!("序列化 actions 失败: {}", e);
                }
            }
        }

        self.game_rules = Some(game_rules);
    }

    /// 获取 ReflectorSoul LLM 配置（带回退逻辑）
    pub fn get_reflector_llm_config(&self) -> &LlmConfig {
        self.llm_reflector.as_ref().unwrap_or(&self.llm)
    }

    /// 获取指定服务器的数据目录
    pub fn server_dir(&self, ws_url: &str) -> PathBuf {
        self.servers_dir.join(server_key(ws_url))
    }

    /// 获取指定服务器的 device.yaml 路径
    pub fn device_yaml_path(&self, ws_url: &str) -> PathBuf {
        self.server_dir(ws_url).join("device.yaml")
    }
}

// ============================================================================
// 测试
// ============================================================================
