// integration/openclaw/tools/jianghu_act/types.ts
// ============================================================================
// Type Definitions for OpenClaw - Cyber-Jianghu Integration
// ============================================================================
//
// 设计原则：
// 1. 最小化类型定义，只保留 TypeScript 类型安全所需
// 2. 所有接口以 crates/agent HTTP API 实际披露的为准
// 3. ActionType 是字符串，不硬编码具体值
// 4. 可用动作从 WorldState.available_actions 动态获取

/**
 * Action types - 完全数据驱动
 *
 * 具体可用动作从 WorldState.available_actions 获取
 */
export type ActionType = string;

/**
 * jianghu_act tool parameters
 */
export interface GameActionParams {
	action: ActionType;
	target?: string;
	data?: string;
	reasoning?: string;
}

/**
 * Intent structure (matches crates/protocol Intent)
 */
export interface Intent {
	agent_id: string;
	tick_id: number;
	action_type: string;
	thought_log?: string;
	action_data?: Record<string, unknown>;
	priority?: number;
}

/**
 * ActionResult - tool 执行结果
 */
export interface ActionResult {
	success: boolean;
	error?: string;
	rejection_type?: string;
	hint?: string;
	narrative?: string;
}

/**
 * Persona info for validation (matches crates/agent PersonaInfo)
 */
export interface PersonaInfo {
	gender: string;
	age: number;
	personality: string[];
	values: string[];
}

/**
 * WorldState structure (matches crates/protocol WorldState)
 */
export interface WorldState {
	event_type: string;
	tick_id: number;
	agent_id?: string;
	world_time?: WorldTime;
	location?: Location;
	self_state?: AgentSelfState;
	entities?: Entity[];
	nearby_items?: SceneItem[];
	events_log?: WorldEvent[];
	available_actions?: AvailableAction[];
}

/**
 * Available action from server (data-driven)
 */
export interface AvailableAction {
	action: string;
	description?: string;
	requires_target?: boolean;
	requires_data?: boolean;
	valid_targets?: string[];
}

export interface WorldTime {
	year: number;
	month: number;
	day: number;
	hour: number;
	minute: number;
	second: number;
	weather: string;
}

export interface Location {
	node_id: string;
	name: string;
	node_type?: string;
}

export interface AgentSelfState {
	attributes: Record<string, number>;
	status_effects: StatusEffect[];
	inventory: InventoryItem[];
}

export interface StatusEffect {
	name: string;
	remaining_ticks: number;
}

export interface InventoryItem {
	item_id: string;
	name: string;
	quantity: number;
	is_equipped: boolean;
}

export interface Entity {
	id: string;
	name: string;
	state?: string;
	distance?: number;
	hostile?: boolean;
}

export interface SceneItem {
	item_id: string;
	name: string;
	quantity: number;
	item_type?: string;
}

export interface WorldEvent {
	event_type: string;
	description: string;
	tick_id: number;
	metadata?: Record<string, unknown>;
}

/**
 * Memory entry structure (matches HTTP API response)
 */
export interface MemoryEntry {
	tick_id?: number;
	content: string;
	importance: number;
	created_at?: string;
}

/**
 * Relationship structure (matches HTTP API response)
 */
export interface Relationship {
	target_agent_id: string;
	target_name: string;
	favorability: number;
	key_events?: KeyEvent[];
}

export interface KeyEvent {
	tick_id: number;
	event_type: string;
	description: string;
	favorability_delta: number;
}

/**
 * Lifespan status (matches HTTP API response)
 */
export interface LifespanStatus {
	current_age: number;
	status: string;
	aging_effects?: string;
}

/**
 * Retry configuration
 */
export interface RetryConfig {
	maxRetries: number;
	baseDelayMs: number;
	maxDelayMs: number;
	backoffMultiplier: number;
}

/**
 * Default retry configuration
 */
export const DEFAULT_RETRY_CONFIG: RetryConfig = {
	maxRetries: 3,
	baseDelayMs: 500,
	maxDelayMs: 5000,
	backoffMultiplier: 2,
};
