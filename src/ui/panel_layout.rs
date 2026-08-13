//! 对齐 `lib/panel-layout.ts`。面板布局宽度计算。

pub const MOBILE_MAX_WIDTH: i64 = 640;
pub const SPLIT_PANEL_MIN_WIDTH: i64 = 960;

pub const SIDEBAR_DEFAULT_WIDTH: i64 = 260;
pub const SIDEBAR_MIN_WIDTH: i64 = 180;
pub const SIDEBAR_MAX_WIDTH: i64 = 480;

pub const RIGHT_PANEL_FALLBACK_WIDTH: i64 = 560;
pub const RIGHT_PANEL_MIN_WIDTH: i64 = 300;
pub const RIGHT_PANEL_MAX_WIDTH: i64 = 1200;

const COMPACT_CHAT_MIN_WIDTH: i64 = 320;
const DESKTOP_CHAT_MIN_WIDTH: i64 = 420;

/// 对齐 `clampPanelWidth`。
pub fn clamp_panel_width(width: i64, min_width: i64, max_width: i64) -> i64 {
    let finite = width; // i64 恒 finite
    let effective_max = min_width.max(max_width);
    finite.clamp(min_width, effective_max)
}

/// 对齐 `getDefaultRightPanelWidth`。
pub fn get_default_right_panel_width(viewport_width: i64) -> i64 {
    clamp_panel_width((viewport_width as f64 * 0.42) as i64, 360, 640)
}

/// 对齐 `getSidebarMaxWidth`。
pub fn get_sidebar_max_width(
    viewport_width: i64,
    right_panel_open: bool,
    right_panel_width: i64,
) -> i64 {
    if viewport_width <= MOBILE_MAX_WIDTH {
        return SIDEBAR_MAX_WIDTH;
    }
    let compact = viewport_width < SPLIT_PANEL_MIN_WIDTH;
    let chat_width = if compact {
        COMPACT_CHAT_MIN_WIDTH
    } else {
        DESKTOP_CHAT_MIN_WIDTH
    };
    let visible_right = if !compact && right_panel_open {
        right_panel_width
    } else {
        0
    };
    SIDEBAR_MAX_WIDTH.min(viewport_width - chat_width - visible_right)
}

/// 对齐 `getRightPanelMaxWidth`。
pub fn get_right_panel_max_width(
    viewport_width: i64,
    sidebar_open: bool,
    sidebar_width: i64,
) -> i64 {
    if viewport_width < SPLIT_PANEL_MIN_WIDTH {
        return RIGHT_PANEL_MAX_WIDTH;
    }
    let visible_sidebar = if sidebar_open { sidebar_width } else { 0 };
    RIGHT_PANEL_MAX_WIDTH.min(viewport_width - DESKTOP_CHAT_MIN_WIDTH - visible_sidebar)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp() {
        assert_eq!(clamp_panel_width(300, 180, 480), 300);
        assert_eq!(clamp_panel_width(100, 180, 480), 180);
        assert_eq!(clamp_panel_width(600, 180, 480), 480);
    }

    #[test]
    fn sidebar_max_mobile() {
        assert_eq!(get_sidebar_max_width(500, true, 400), SIDEBAR_MAX_WIDTH);
    }

    #[test]
    fn sidebar_max_desktop() {
        let w = get_sidebar_max_width(1920, true, 560);
        assert!(w <= SIDEBAR_MAX_WIDTH);
        assert!(w > 0);
    }
}
