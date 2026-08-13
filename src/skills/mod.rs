//! skills 模块 — 技能加载、安装信息标注、更新检测与 npx 调用。
//!
//! 对齐 `lib/skill-lock.ts` + `lib/skills-service.ts` + `lib/skill-updates.ts`
//! + `lib/npx.ts`:
//! - `annotate_skills_with_install_info`:skills-lock.json(全局 XDG/homedir +
//!   项目 cwd)标注安装来源/版本哈希/可更新性
//! - `load_skills_with_install_info`:技能加载编排(引擎注入回调)
//! - `check_skill_update(s)`:GitHub trees API / skills.sh snapshot + git 回退
//! - `find_npx_cli` / `build_npx_invocation`:无 shell 的 npx-cli.js 定位

pub mod npx;
pub mod skill_lock;
pub mod skill_updates;
pub mod skills_service;

pub use npx::{build_npx_invocation, find_npx_cli, NpxInvocation};
pub use skill_lock::{
    annotate_skills_with_install_info, build_skills_sh_url, find_lock_entry,
    get_global_skills_lock_path, get_install_info, is_within, normalize_source, path_resolve,
    read_skill_lock, SkillInfo, SkillInstallInfo, SkillInstallScope, SourceInfo,
};
pub use skill_updates::{
    build_skill_update_args, check_global_skill, check_project_skill, check_skill_update,
    check_skill_updates, result, skill_folder, skill_name_from_package, skill_slug,
    skill_update_key, HttpError, SkillUpdateIo, SkillUpdateIoError, SkillUpdateResult,
    SkillUpdateState, CHECK_TIMEOUT_MS, DEFAULT_SKILLS_API_BASE, GIT_CHECK_TIMEOUT_MS,
};
pub use skills_service::{
    load_skills_with_install_info, ResourceDiagnostic, SkillsLoader, SkillsResponse,
};
