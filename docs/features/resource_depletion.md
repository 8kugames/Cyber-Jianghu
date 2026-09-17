# 资源点存量/枯竭模型

> **实施状态（2026-09-17）**：阶段 2 已实施（含阶段 1 内存配额被 stock 机制替代）——
> migration 026 `resource_nodes` 表、`game_data/resource_stock.rs`（DB 权威 + 内存缓存，
> 采集 Saga 事务内扣减、日再生受四季加成）、GatherableItem 广播 `stock` 字段
> （PROTOCOL_VERSION 3.3.0 minor bump）、agent 感知「（余 N）」渲染、locations.yaml
> 生产配额改写为 gatherable_stocks。守卫测试见 sqlx_live_schema_guard_test（活库跑）。
> 阶段 1 的 gatherable_daily_quotas 内存机制已由本阶段替代移除。

2026-09-14 审计遗留「资源点无枯竭/存量模型」。本文固化调查结论与两阶段设计，
供拍板后实施。当前现实：**采集无限**——take(source_type=resource) 校验
item ∈ 当前节点 gatherable_items 后凭空入包（`basic.rs:177-233`），无存量、
无冷却、无枯竭；`gatherable_items` 仅为 `Vec<String>` 物品名列表
（`protocol/src/types/locations.rs:85`），运行时原样进 LocationRegistry，
无 DB 表（migrations 001-025 无 resource 表）。

## 设计目标

1. 生存压力真实化：食物/水/材料有获取成本，采集点有稀缺性，驱动迁移与竞争（涌现源头）。
2. 数据驱动：存量参数全部进 locations.yaml，加载期 fail-fast 校验（同 resource_growth_rate 先例）。
3. 契约兼容：协议扩展走 optional 字段，PROTOCOL_VERSION minor bump。

## 阶段 1：每节点每日采集配额（内存态，不动 schema/协议）

- locations.yaml `gatherable_items` 从 `Vec<String>` 扩展为可选结构：
  `gatherable: { items: [馒头, 野果], daily_quota: 20 }`（旧 `Vec<String>` 形态
  serde alias 兼容，daily_quota 缺省 = 不限）。
- 状态：`LocationRegistry` 旁加 `DashMap<(Uuid, String), i32>` 内存余量；
  `tick_counter` 级别的每日重置（对齐日结算的日历日号递增检测，避免同款相位错位）。
- 执行：basic.rs resource 分支先查余量，扣减成功才发 ItemAcquired；
  配额不足返回失败（消息「此地的野果今日已采尽」——LLM 可感知，驱动换地/换时）。
- Saga 回滚注意：扣减必须发生在 StateChange 应用成功之后（mutator 侧），
  避免回滚后配额白扣；预检在 executor，扣减在 mutator。
- 已知限制：重启丢余量（回满）。阶段 2 落 DB 后消除。

## 阶段 2：存量 + 再生 + 广播（DB 态，协议 minor bump）

- migration 026：`resource_nodes(node_id, item_id, stock, max_stock,
  regen_per_game_day, updated_at)`，纳入采集 Saga 事务。
- 再生：日结算钩子按 `regen_per_game_day` 回补（受四季 resource_growth_rate
  加成——冬季枯竭春季复苏，与既有季节经济对齐）。
- 广播：GatherableItem 加 `stock: Option<u32>`（optional；None = 不限），
  world_state.rs 三处渲染 + PROTOCOL_VERSION minor bump；agent 侧
  可采集列表渲染显示「野果（余 3）」，避免「可采集但枯竭」的意图空转。
- 初始存量：locations.yaml 配 max_stock 与初始比例；迁移时对存量节点
  按 max_stock 全量初始化。

## 决策点（拍板后实施）

1. 阶段 1 是否先行（内存态、可逆、零协议影响），还是直接一步到阶段 2？
2. daily_quota / regen 数值：建议先保守（馒头 20/日、野果 10/日、再生 20%/日），
   联调观察后调。
3. 稀缺导致的竞争烈度：是否允许「取他人背包」与采集竞争叠加（现有授权判定不变）。
4. 涌现面板/经济统计是否展示资源存量（dashboard 加卡片）。
