//! 经历日志（agent_action_logs 的读侧聚合）
//!
//! 三个子模块按读取视角划分：
//! - [`stream`]：全局流水（每 tick 一张卡片，卡片级筛选与成败徽章）
//! - [`agent`]：单角色经历（设备归属校验 + tick 分页）
//! - [`card`]：两种视角共用的卡片级聚合判定（行级成败、execution_results
//!   注入、模型归一）。判定只此一处，筛选与徽章才不会分叉。
//!
//! items / display 两族端点与经历日志无关，拆至 `dashboard::items` /
//! `dashboard::display`。

mod agent;
mod card;
mod stream;

pub use agent::*;
pub use stream::*;
