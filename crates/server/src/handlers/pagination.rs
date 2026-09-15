//! 分页偏移量计算
//!
//! 统一在此处做 i64 域内运算并钳位，避免 `(page - 1) * limit` 在 i32 域内相乘
//! 溢出：debug 构建直接 panic（请求连接被丢弃，而不是返回 500），release 构建
//! 回绕成负偏移（PostgreSQL 对负 OFFSET 直接报错）。曾实测 `?page=2147483647`
//! 使经历日志端点整体不可用。
//!
//! 语义约定：`page` 小于 1 视作 1，`limit` 为负视作 0；偏移量钳到 i32 上限，
//! 即"翻到极远的页"，查询自然返回空页。

/// 计算分页偏移量（i64，供 SQL 的 OFFSET 参数使用）
pub fn offset_of(page: i32, limit: i32) -> i64 {
    let page = page.max(1) as i64;
    let limit = limit.max(0) as i64;
    (page - 1).saturating_mul(limit)
}

/// 同 [`offset_of`]，但返回 i32（供形参为 i32 的存储层函数使用）
pub fn offset_of_i32(page: i32, limit: i32) -> i32 {
    offset_of(page, limit).min(i32::MAX as i64) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamps_page_and_limit_into_valid_range() {
        assert_eq!(offset_of(1, 20), 0);
        assert_eq!(offset_of(3, 20), 40);
        assert_eq!(offset_of(0, 20), 0, "page < 1 视作第 1 页");
        assert_eq!(offset_of(-5, 20), 0);
        assert_eq!(offset_of(2, -1), 0, "limit < 0 视作 0");
        assert_eq!(offset_of(2, 0), 0);
    }

    #[test]
    fn extreme_page_never_overflows() {
        // i32::MAX * i32::MAX 远超 i32，旧实现此处 panic（debug）或回绕成负偏移（release）
        assert_eq!(
            offset_of(i32::MAX, i32::MAX),
            (i32::MAX as i64 - 1) * i32::MAX as i64
        );
        assert_eq!(offset_of_i32(i32::MAX, i32::MAX), i32::MAX);
        assert_eq!(offset_of(i32::MAX, 100), (i32::MAX as i64 - 1) * 100);
        assert_eq!(offset_of_i32(i32::MIN, i32::MIN), 0);
    }
}
