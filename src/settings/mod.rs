//! settings 模块 — 启动偏好持久化。
//!
//! 对齐 `lib/startup-preferences.ts`:把浏览器里的显式选择持久化,而不重复调用
//! AgentSession 的 setter(会话构造器已记录生效模型与 thinking level,重复调用
//! 会追加重复会话条目并重复发扩展事件)。

pub mod startup_preferences;

pub use startup_preferences::{
    persist_explicit_startup_preferences, EffectiveStartupPreferences, ExplicitStartupPreferences,
    SettingsOps, StartupPreferencesOutcome,
};
