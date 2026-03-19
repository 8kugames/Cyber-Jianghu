// ============================================================================
// 认知流程编排
// ============================================================================

use anyhow::Result;
use std::sync::Arc;
use tracing::debug;

use super::chain::CognitiveChain;
use super::stages::{CognitiveStage, StageOutput};
use crate::models::WorldState;

/// 阶段处理器 Trait
///
/// 定义认知阶段的处理接口，支持 COI 架构
#[async_trait::async_trait]
pub trait StageProcessor: Send + Sync {
    /// 处理指定阶段
    async fn process(
        &self,
        stage: CognitiveStage,
        world_state: &WorldState,
        chain: &CognitiveChain,
    ) -> Result<StageOutput>;

    /// 获取处理器名称
    fn name(&self) -> &str {
        "StageProcessor"
    }
}

/// 认知流程编排器
///
/// 负责协调各个认知阶段的执行顺序
pub struct CognitivePipeline {
    /// 阶段处理器列表
    processors: Vec<Arc<dyn StageProcessor>>,
}

impl CognitivePipeline {
    /// 创建新的认知流程
    pub fn new() -> Self {
        Self {
            processors: Vec::new(),
        }
    }

    /// 添加阶段处理器
    pub fn with_processor(mut self, processor: Arc<dyn StageProcessor>) -> Self {
        self.processors.push(processor);
        self
    }

    /// 执行完整认知流程
    pub async fn execute(&self, world_state: &WorldState) -> Result<CognitiveChain> {
        let start_time = std::time::Instant::now();
        let tick_id = world_state.tick_id;

        debug!("[Pipeline] 开始认知流程，Tick {}", tick_id);

        // 创建空的认知链
        let mut chain = CognitiveChain::new("未知".to_string(), "默认人设".to_string(), tick_id);

        // 按顺序执行各个阶段
        for stage in CognitiveStage::all() {
            debug!("执行阶段: {}", stage.name());

            // 查找对应的处理器
            let processor = self.processors.iter().find(|p| p.can_handle(&stage));

            if let Some(proc) = processor {
                let output = proc.process(stage, world_state, &chain).await?;
                chain.add_stage(output);
            } else {
                return Err(anyhow::anyhow!("找不到阶段 {:?} 的处理器", stage));
            }
        }

        // 记录耗时
        chain.duration_ms = start_time.elapsed().as_millis() as u64;

        debug!("[Pipeline] 认知流程完成，耗时 {}ms", chain.duration_ms);

        Ok(chain)
    }

    /// 使用已有认知链执行流程（支持流式处理）
    pub async fn execute_with_chain(
        &self,
        world_state: &WorldState,
        mut chain: CognitiveChain,
    ) -> Result<CognitiveChain> {
        debug!(
            "[Pipeline] 基于已有链继续执行，Tick {}",
            world_state.tick_id
        );

        // 确定需要执行的阶段
        let completed_stages = chain.stages.len() as u8;
        let all_stages = CognitiveStage::all();

        for stage in all_stages.iter().skip(completed_stages as usize) {
            debug!("执行阶段: {}", stage.name());

            let processor = self.processors.iter().find(|p| p.can_handle(stage));

            if let Some(proc) = processor {
                let output = proc.process(*stage, world_state, &chain).await?;
                chain.add_stage(output);
            } else {
                return Err(anyhow::anyhow!("找不到阶段 {:?} 的处理器", stage));
            }
        }

        Ok(chain)
    }
}

impl Default for CognitivePipeline {
    fn default() -> Self {
        Self::new()
    }
}

/// 默认的阶段处理器扩展 Trait
pub trait StageProcessorExt {
    /// 检查处理器是否可以处理指定阶段
    fn can_handle(&self, stage: &CognitiveStage) -> bool;
}

// 为所有 StageProcessor 实现默认行为
impl<T: StageProcessor + ?Sized> StageProcessorExt for T {
    fn can_handle(&self, _stage: &CognitiveStage) -> bool {
        // 默认实现：所有处理器都可以处理所有阶段
        // 实际使用时应该根据具体实现覆盖
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockProcessor {
        name: String,
    }

    #[async_trait::async_trait]
    impl StageProcessor for MockProcessor {
        async fn process(
            &self,
            stage: CognitiveStage,
            _world_state: &WorldState,
            _chain: &CognitiveChain,
        ) -> Result<StageOutput> {
            Ok(StageOutput::new(
                stage,
                format!("Mock output for {:?}", stage),
            ))
        }

        fn name(&self) -> &str {
            &self.name
        }
    }

    fn create_test_world_state() -> WorldState {
        use cyber_jianghu_protocol::{AgentSelfState, Location, WorldTime};
        use std::collections::HashMap;

        WorldState {
            event_type: "world_state".to_string(),
            tick_id: 1,
            agent_id: Some(uuid::Uuid::new_v4()),
            world_time: WorldTime {
                year: 2024,
                month: 3,
                day: 15,
                hour: 12,
                minute: 0,
                second: 0,
                weather: "晴".to_string(),
            },
            location: Location {
                node_id: "test_loc".to_string(),
                name: "测试地点".to_string(),
                node_type: "indoor".to_string(),
                adjacent_nodes: vec![],
            },
            self_state: AgentSelfState {
                attributes: HashMap::new(),
                attribute_descriptions: HashMap::new(),
                status_effects: vec![],
                inventory: vec![],
            },
            entities: vec![],
            nearby_items: vec![],
            events_log: vec![],
            available_actions: vec![],
        }
    }

    #[tokio::test]
    async fn test_pipeline_creation() {
        let pipeline = CognitivePipeline::new();
        assert_eq!(pipeline.processors.len(), 0);
    }

    #[tokio::test]
    async fn test_pipeline_with_processor() {
        let processor = Arc::new(MockProcessor {
            name: "TestProcessor".to_string(),
        });

        let pipeline = CognitivePipeline::new().with_processor(processor);

        assert_eq!(pipeline.processors.len(), 1);
    }

    #[tokio::test]
    async fn test_pipeline_execute() {
        let processor = Arc::new(MockProcessor {
            name: "TestProcessor".to_string(),
        });

        let pipeline = CognitivePipeline::new().with_processor(processor);

        let world_state = create_test_world_state();

        let result = pipeline.execute(&world_state).await;
        assert!(result.is_ok());

        let chain = result.unwrap();
        assert!(chain.is_complete());
        assert_eq!(chain.stages.len(), 4);
    }

    #[tokio::test]
    async fn test_pipeline_execute_with_chain() {
        let processor = Arc::new(MockProcessor {
            name: "TestProcessor".to_string(),
        });

        let pipeline = CognitivePipeline::new().with_processor(processor);

        let world_state = create_test_world_state();
        let mut chain = CognitiveChain::new("测试".to_string(), "测试人设".to_string(), 1);

        // 添加第一个阶段
        chain.add_stage(StageOutput::new(
            CognitiveStage::Perception,
            "感知内容".to_string(),
        ));

        // 继续执行剩余阶段
        let result = pipeline.execute_with_chain(&world_state, chain).await;
        assert!(result.is_ok());

        let final_chain = result.unwrap();
        assert!(final_chain.is_complete());
        assert_eq!(final_chain.stages.len(), 4);
    }
}
