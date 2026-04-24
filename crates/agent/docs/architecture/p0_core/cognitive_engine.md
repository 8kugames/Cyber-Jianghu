# 认知流转引擎 (CognitiveEngine)

**级别**: P0 核心基石
**模块**: `crates/agent`

## 1. 设计目标
将环境感知（WorldState）转化为具体行动（Intent）的思考中枢，追求单次 LLM 调用的最大信息密度和最低延迟。

## 2. 核心机制
### 2.1 单次推理 (One-Shot Reasoning)
- 摒弃了传统的“感知-反思-规划-行动”需要调用 4 次 LLM 的低效循环。
- 通过精细的 System Prompt 和 Context 组装，一次 LLM 流式输出完成“感知环境→产生动机→制定规划→决定动作”。

### 2.2 认知链追踪 (Cognitive Chain)
- 在单次推理中，LLM 被强制要求按结构输出 JSON 或特定格式。
- 引擎提取出每一步的推导逻辑文本（如“我看到仇人张三，我感到愤怒，我决定攻击他”），记录为 `CognitiveChain`。
- 该链路被打包进 `SoulCycleReport`，用于控制台展示和调试。

### 2.3 上下文组装与加速
- **滑动窗口摘要**：截取最近 N 个 Tick 的历史行为摘要。
- **模板渲染**：基于 `prompt_templates.yaml` 动态渲染。
- **Persona 缓存**：缓存角色的静态背景和性格，避免每 Tick 重复构建字符串。

### 2.4 别名翻译映射 (Alias Translation)
- 内置中英翻译字典，纠正 LLM 产生的格式幻觉。
- **动作名映射**：如 LLM 幻觉的"攻击某人"映射为"attack"。
- **字段映射**：如 LLM 幻觉的 "destination" 映射为 "target_location"。
- **对象 ID 解析**：从 WorldState 字典中查找周围实体名称，自动将其转换为 UUID / NodeID / ItemID。

## 3. 架构约束
- 必须具备极高的容错性，对 LLM 输出的脏数据进行清洗映射，不能因为多了一个括号就导致整个 Agent 崩溃重启。
- 推理过程必须是异步的，且需设置严格的超时机制。

## 4. 代码入口
- 认知引擎主类: `crates/agent/src/soul/actor/engine.rs`
- 翻译与幻觉修正: `crates/agent/src/soul/actor/translation.rs`
