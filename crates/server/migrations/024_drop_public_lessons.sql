-- 跨 Agent 传承 Layer 2（教训聚合广播）移除：死亡知识传播改为纯涌现
-- （目击 → 记忆 → 目击者自主"说" → 同位置者听见 → 随行走扩散），
-- 服务器不再聚合死因教训，public_lessons 表随之废弃。
-- 死因/存活统计保留在引擎侧（death_metadata 日志 + reward 结算）。
DROP TABLE IF EXISTS public_lessons;
