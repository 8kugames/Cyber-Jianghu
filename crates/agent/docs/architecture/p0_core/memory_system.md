# 三级记忆系统 (Memory System)

在虚境：江湖中，为了解决 LLM 上下文窗口限制与长期记忆遗忘的问题，我们构建了一个受人类认知心理学启发的三级记忆系统，并配合基于 Ebbinghaus 遗忘曲线的遗忘机制。

相关代码路径：`crates/agent/src/component/memory/`

## 整体架构

Agent 内部维护了一个线程安全的 `MemoryManager`，它统一管理三种类型的记忆。

1. **工作记忆 (Working Memory)**：短期高保真记忆。
2. **情景记忆 (Episodic Memory)**：长期事件记忆。
3. **语义记忆 (Semantic Memory)**：向量化的抽象知识与经验。

### 1. 工作记忆 (Working Memory)

工作记忆类似于人类的“短期记忆”。
- **存储介质**：内存（程序启动时从 SQLite 加载近期记录，运行时保存在内存列表中）。
- **生命周期**：容量有限（例如 `working_memory_size = 20`）。当工作记忆塞满时，最旧的记忆会被触发“遗忘机制”并转入情景记忆。
- **作用**：每次 LLM 决策时，工作记忆会**完整无损地**注入到 Prompt 中，让 Agent 清楚地记得刚刚发生了什么。

### 2. 情景记忆 (Episodic Memory)

情景记忆类似于人类的“长期事件回忆”。
- **存储介质**：本地 SQLite (`client_memories` 表)。
- **存储内容**：被工作记忆淘汰下来的事件。
- **提取方式**：不再完整注入 Prompt，而是根据 `MemoryScorer`（结合时间衰减、情绪唤醒度、重要性）对所有情景记忆进行打分，仅将**Top N 最重要的记忆**（默认 10 条）提取并注入到 Prompt。

### 3. 语义记忆 (Semantic Memory)

语义记忆是海量的、被抽象化的经验和知识库。
- **存储介质**：内存中的 HNSW 向量索引 (`instant-distance` crate) + SQLite 持久化。
- **提取方式**：被动触发。每次 Tick 时**绝不会自动注入**，而是通过地魂 (EarthSoul) 的 `search_memory` 工具，由大模型在思考时主动发出查询（Tool Calling）。
- **特点**：支持千万级别记忆的毫秒级检索，是 Agent 成为“百晓生”的基础。

## 记忆遗忘与归档 (Forgetting Mechanism)

由于数据量会无限增长，我们引入了基于 Ebbinghaus（艾宾浩斯）曲线的遗忘系统（`crates/agent/src/component/memory/forgetting.rs`）。

1. **触发时机**：每 N 个 Tick（默认 84），`run_forgetting` 会被调用一次。
2. **打分衰减**：系统会遍历所有未归档的情景记忆，计算其衰减后的保留分数。
3. **向量化**：当一条情景记忆的分数低于阈值（如 30 分）时，它会被从情景检索池中“剔除”，并被**异步向量化 (Embedding)**，存入语义记忆。
4. **彻底遗忘**：这标志着该事件从“我记得具体发生了什么”变成了“我脑海中有个模糊的印象（仅能通过搜索唤起）”。

## 与认知引擎的集成

在 `CognitiveEngine` 构建上下文时，会调用 `manager.build_llm_context()`，它将输出如下结构的 Markdown：

```markdown
## 长期记忆 (Top 10 重要)
[Tick 100] 我在客栈被李四打了一拳 (重要度: 85)
...

## 短期记忆 (最近发生)
[Tick 500] 我走到客栈门口。
[Tick 501] 店小二问我客官打尖还是住店。
```

这种设计在消耗极少 Token 的前提下，给予了 Agent 极强的时空连续感。
