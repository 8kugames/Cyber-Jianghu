-- 012_chinese_ids.sql
-- 将英文 ID 迁移为中文主键
-- 顺序：items（父表）→ agent_inventory/ground_items（子表）→ agent_states

-- 1. items 表 item_id 迁移（必须先于外键引用表）
UPDATE items SET item_id = '馒头' WHERE item_id = 'mantou';
UPDATE items SET item_id = '水' WHERE item_id = 'water';
UPDATE items SET item_id = '银子' WHERE item_id = 'silver';
UPDATE items SET item_id = '刀' WHERE item_id = 'knife';
UPDATE items SET item_id = '木剑' WHERE item_id = 'wooden_sword';
UPDATE items SET item_id = '面粉' WHERE item_id = 'flour';
UPDATE items SET item_id = '小麦' WHERE item_id = 'wheat';
UPDATE items SET item_id = '生面团' WHERE item_id = 'dough';
UPDATE items SET item_id = '木材' WHERE item_id = 'wood';
UPDATE items SET item_id = '李广杏' WHERE item_id = 'li_guang_apricot';
UPDATE items SET item_id = '李广杏干' WHERE item_id = 'dried_li_guang_apricot';

-- 2. 位置 node_id 迁移
UPDATE agent_states SET node_id = '河西走廊' WHERE node_id = 'hexi_corridor';
UPDATE agent_states SET node_id = '龙门客栈' WHERE node_id = 'longmen_inn';
UPDATE agent_states SET node_id = '龙门大堂' WHERE node_id = 'longmen_lobby';
UPDATE agent_states SET node_id = '龙门后院' WHERE node_id = 'longmen_backyard';
UPDATE agent_states SET node_id = '龙门厨房' WHERE node_id = 'longmen_kitchen';
UPDATE agent_states SET node_id = '荒漠' WHERE node_id = 'desert';
UPDATE agent_states SET node_id = '酒泉' WHERE node_id = 'jiuquan';

-- 3. 背包 item_id 迁移
UPDATE agent_inventory SET item_id = '馒头' WHERE item_id = 'mantou';
UPDATE agent_inventory SET item_id = '水' WHERE item_id = 'water';
UPDATE agent_inventory SET item_id = '银子' WHERE item_id = 'silver';
UPDATE agent_inventory SET item_id = '刀' WHERE item_id = 'knife';
UPDATE agent_inventory SET item_id = '木剑' WHERE item_id = 'wooden_sword';
UPDATE agent_inventory SET item_id = '面粉' WHERE item_id = 'flour';
UPDATE agent_inventory SET item_id = '小麦' WHERE item_id = 'wheat';
UPDATE agent_inventory SET item_id = '生面团' WHERE item_id = 'dough';
UPDATE agent_inventory SET item_id = '木材' WHERE item_id = 'wood';
UPDATE agent_inventory SET item_id = '李广杏' WHERE item_id = 'li_guang_apricot';
UPDATE agent_inventory SET item_id = '李广杏干' WHERE item_id = 'dried_li_guang_apricot';

-- 4. 地面物品 item_id 迁移
UPDATE ground_items SET item_id = '馒头' WHERE item_id = 'mantou';
UPDATE ground_items SET item_id = '水' WHERE item_id = 'water';
UPDATE ground_items SET item_id = '银子' WHERE item_id = 'silver';
UPDATE ground_items SET item_id = '刀' WHERE item_id = 'knife';
UPDATE ground_items SET item_id = '木剑' WHERE item_id = 'wooden_sword';
UPDATE ground_items SET item_id = '面粉' WHERE item_id = 'flour';
UPDATE ground_items SET item_id = '小麦' WHERE item_id = 'wheat';
UPDATE ground_items SET item_id = '生面团' WHERE item_id = 'dough';
UPDATE ground_items SET item_id = '木材' WHERE item_id = 'wood';
UPDATE ground_items SET item_id = '李广杏' WHERE item_id = 'li_guang_apricot';
UPDATE ground_items SET item_id = '李广杏干' WHERE item_id = 'dried_li_guang_apricot';

-- 5. 修改默认 node_id 为中文
ALTER TABLE agent_states ALTER COLUMN node_id SET DEFAULT '龙门大堂';
