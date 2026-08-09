//! 对齐 `lib/chat-lazy-load.ts`。消息列表懒加载窗口计算。

pub const VISIBLE_PAGE_SIZE: usize = 50;

/// 对齐 `getVisibleRenderWindow`。
pub fn get_visible_render_window(total_count: usize, visible_count: usize) -> (usize, bool) {
    let clamped = visible_count.min(total_count);
    let start_index = (total_count as isize - clamped as isize).max(0) as usize;
    (start_index, start_index > 0)
}

/// 对齐 `getNextVisibleCount`。
pub fn get_next_visible_count(current: usize, page_size: Option<usize>) -> usize {
    current + page_size.unwrap_or(VISIBLE_PAGE_SIZE)
}

/// 对齐 `captureScrollDistance`。
pub fn capture_scroll_distance(scroll_height: i64, scroll_top: i64) -> i64 {
    scroll_height - scroll_top
}

/// 对齐 `restoreScrollTop`。
pub fn restore_scroll_top(scroll_height: i64, saved_distance: i64) -> i64 {
    (scroll_height - saved_distance).max(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_window() {
        assert_eq!(get_visible_render_window(100, 50), (50, true));
        assert_eq!(get_visible_render_window(30, 50), (0, false));
    }

    #[test]
    fn next_visible() {
        assert_eq!(get_next_visible_count(50, None), 100);
        assert_eq!(get_next_visible_count(50, Some(10)), 60);
    }
}
