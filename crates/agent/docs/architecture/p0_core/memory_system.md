# 三级记忆系统

**级别**: P0 核心基石
**模块**: `crates/agent`

## 1. 设计目标
模拟人类记忆机制，构建随时间衰退与具备联想能力的存储结构，赋予 Agent 记忆长期历史和短期细节的能力。

## 2. 核心机制
### 2.1 工作记忆 (Working Memory)
- **实现**：基于有界 FIFO 队列。
- **功能**：维护当前短期上下文（如最近听到的几句话、刚刚发生的攻击），直接随每个 Tick 的 Context 喂给 LLM。
- **生命周期**：容量满了后，最旧的记忆会被挤出或归档。

### 2.2 情景记忆 (Episodic Memory)
- **实现**：利用 SQLite 持久化存储，带时间戳的离散事件记录。
- **机制**：
  - 自动基于事件类型（如濒死、闲聊）与元数据为其进行**重要度评分**（1-10分）。
  - 包含基于**艾宾浩斯遗忘曲线**的记忆衰退和归档机制。低重要度的事件随着时间推移会被遗忘，高重要度事件被长期保留。

### 2.3 语义记忆 (Semantic Memory)
- **实现**：采用 HNSW (Hierarchical Navigable Small World) 向量索引实现相似度联想。
- **机制**：将归档的情景记忆进行 Embedding，供 LLM 在遇到特定场景或人物时进行向量召回（如“我以前见过这把剑吗？”）。
- **降级**：若向量搜索服务失败，自动降级为 SQLite 的全文检索 (FTS)。

### 2.4 地魂接入 (Memory Tools)
- 将 `MemoryManager` 通过 `Arc<RwLock>` 共享给主循环与地魂。
- 地魂工具池暴露 `search_memory` 和 `recall_archived` 接口，允许 LLM 主动检索。

## 3. 架构约束
- 记忆读取必须足够快，严禁在构建每一帧的 Context 时进行耗时的全表扫描。
- 并发安全：主循环写入和地魂异步读取时，必须正确使用异步读写锁。

## 4. 代码入口
- 记忆管理器: `crates/agent/src/component/memory/manager.rs`
- 地魂检索工具: `crates/agent/src/soul/earth/memory_tool.rs`
