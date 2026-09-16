// ============================================================================
// Prompt 模板加载 / 持久化 / 搜索路径
// ============================================================================

use super::*;

impl CognitiveEngine {
    /// 加载 prompt 模板配置
    ///
    /// 查找路径：
    /// 1. $CYBER_JIANGHU_CONFIG_DIR/prompt_templates.json
    /// 2. ~/.cyber-jianghu/config/prompt_templates.json
    /// 3. 内置默认路径（编译时嵌入或同级 config/）
    ///
    /// 本地文件不存在时返回空壳配置，等待 WS ConfigUpdate 覆盖。
    pub(super) fn load_prompt_template() -> PromptTemplateConfig {
        let search_paths = Self::prompt_template_search_paths();

        for path_opt in &search_paths {
            if let Some(path) = path_opt
                && path.exists()
            {
                match super::super::prompt_template::load_prompt_template_from_file(path) {
                    Ok(config) => {
                        info!("已加载 prompt 模板: {:?}", path);
                        return config;
                    }
                    Err(e) => {
                        warn!(
                            "Prompt 模板文件解析失败 ({}): {}，等待 Server WS 下发",
                            path.display(),
                            e
                        );
                    }
                }
            }
        }
        warn!(
            "未找到 prompt_templates.json，搜索路径: {:?}，等待 Server WS 下发",
            search_paths
                .iter()
                .filter_map(|p| p.as_ref().map(|x| x.display().to_string()))
                .collect::<Vec<_>>()
        );
        PromptTemplateConfig {
            version: super::super::prompt_template::EMPTY_FALLBACK_VERSION.to_string(),
            description: String::new(),
            templates: std::collections::HashMap::new(),
            memory_narrative: None,
            rule_sections: None,
        }
    }

    /// prompt_templates.json 搜索路径（load 和 save 共用）
    /// 第一优先级：CYBER_JIANGHU_DATA_DIR（Server 写入路径，与 Server 写盘目标对称）
    pub(super) fn prompt_template_search_paths() -> [Option<std::path::PathBuf>; 4] {
        [
            std::env::var("CYBER_JIANGHU_DATA_DIR")
                .ok()
                .map(|d| std::path::PathBuf::from(d).join("prompt_templates.json")),
            std::env::var("CYBER_JIANGHU_CONFIG_DIR")
                .ok()
                .map(|d| std::path::PathBuf::from(d).join("prompt_templates.json")),
            dirs::home_dir().map(|h| {
                h.join(".cyber-jianghu")
                    .join("config")
                    .join("prompt_templates.json")
            }),
            Some(std::path::PathBuf::from("config/prompt_templates.json")),
        ]
    }

    /// 获取 Prompt 模板配置（运行时覆盖优先）
    ///
    /// 优先返回 Server ConfigUpdate 下发的配置，其次返回本地 JSON 配置。
    pub fn prompt_template(&self) -> PromptTemplateConfig {
        // runtime override 优先
        if let Some(runtime) = self
            .runtime_prompt_template
            .read()
            .expect("rwlock poisoned")
            .as_ref()
        {
            return runtime.clone();
        }
        self.prompt_template.clone()
    }

    /// 从 Server 下发的 PromptTemplateConfig 直接更新（JSON 路径）
    /// 锁顺序: rule_cache -> runtime_prompt_template（必须与 build_system_message 一致）
    pub fn update_prompt_template_from_config(&self, config: PromptTemplateConfig) {
        self.sync_rule_cache(&config);
        *self
            .runtime_prompt_template
            .write()
            .expect("rwlock poisoned") = Some(config);
        info!("Prompt 模板已从 Server JSON ConfigUpdate 更新");
    }

    /// 将当前 prompt 模板配置持久化到本地磁盘（供下次启动加载）
    pub fn save_prompt_template_to_disk(&self) {
        let config = self.prompt_template();
        if config.version == super::super::prompt_template::EMPTY_FALLBACK_VERSION {
            return;
        }

        let json_bytes = match config.to_json_bytes() {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!("Prompt 模板序列化失败: {}", e);
                return;
            }
        };

        // 使用与 load_prompt_template() 相同的路径优先级：
        // 优先写入已存在的路径，否则写入最高优先级的路径
        let search_paths = Self::prompt_template_search_paths();
        let save_path = search_paths
            .iter()
            .find_map(|p| p.as_ref().filter(|path| path.exists()))
            .cloned()
            .or_else(|| search_paths.into_iter().find_map(|p| p))
            .unwrap_or_else(|| std::path::PathBuf::from("config/prompt_templates.json"));

        if let Some(parent) = save_path.parent()
            && !parent.exists()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            tracing::warn!("创建配置目录失败 {}: {}", parent.display(), e);
            return;
        }

        match std::fs::write(&save_path, &json_bytes) {
            Ok(()) => tracing::info!(
                "prompt_templates.json 已写盘: {} ({} bytes)",
                save_path.display(),
                json_bytes.len()
            ),
            Err(e) => tracing::warn!("prompt_templates.json 写盘失败: {}", e),
        }
    }
}
