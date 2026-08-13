//! 对齐 `lib/npx.ts`。npx-cli.js 定位 + 无 shell 调用。
//!
//! 上游:Windows 上 PATH 里的 `npx` 是 `npx.cmd`,Node ≥20.12 拒绝在无
//! `shell: true` 时 spawn;走 shell 又会引入用户参数引号问题。因此直接找到
//! 随 Node 分发的 `npx-cli.js`,用当前 node 二进制直接调用(全平台一致,
//! 无 shell)。Rust 版移植:候选路径定位 + 进程调用抽象(宿主注入,
//! 避免把具体 spawn 方式绑死)。

/// 对齐 `findNpxCli`:返回绝对路径的 npx-cli.js,找不到为 None。
///
/// Node 的 `path.join` 会折叠 `..`(`join(bin, "..", "lib", ...)` → `.../lib/...`),
/// 这里用 `path_resolve` 对候选路径做同样的归一化。
pub fn find_npx_cli(exec_path: &str) -> Option<String> {
    let node_dir = std::path::Path::new(exec_path).parent()?;
    let candidates = [
        // Windows MSI 布局:node.exe 与 node_modules 同目录
        node_dir
            .join("node_modules")
            .join("npm")
            .join("bin")
            .join("npx-cli.js"),
        // Unix 布局:.../bin/node + .../lib/node_modules/npm/bin/npx-cli.js
        node_dir
            .join("..")
            .join("lib")
            .join("node_modules")
            .join("npm")
            .join("bin")
            .join("npx-cli.js"),
    ];
    for candidate in candidates {
        let candidate = crate::skills::skill_lock::path_resolve(&candidate.to_string_lossy());
        if std::path::Path::new(&candidate).is_file() {
            return Some(candidate);
        }
    }
    None
}

/// 对齐 `runNpx` 的决策:`{ command, commandArgs }`。
pub struct NpxInvocation {
    pub command: String,
    pub command_args: Vec<String>,
}

/// 对齐 `runNpx` 的进程命令构造(execFile 调用交给宿主注入)。
pub fn build_npx_invocation(args: &[String], exec_path: &str) -> NpxInvocation {
    match find_npx_cli(exec_path) {
        Some(npx_cli) => NpxInvocation {
            command: exec_path.to_string(),
            command_args: std::iter::once(npx_cli)
                .chain(args.iter().cloned())
                .collect(),
        },
        None => NpxInvocation {
            command: "npx".to_string(),
            command_args: args.to_vec(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_npx_missing_returns_none() {
        assert_eq!(find_npx_cli("/nonexistent/bin/node"), None);
    }

    #[test]
    fn build_invocation_fallback_to_npx() {
        let inv = build_npx_invocation(&["--version".to_string()], "/nonexistent/bin/node");
        assert_eq!(inv.command, "npx");
        assert_eq!(inv.command_args, vec!["--version".to_string()]);
    }

    #[test]
    fn build_invocation_with_found_cli() {
        // 构造假的 Unix 布局:tmp/bin/node + tmp/lib/node_modules/npm/bin/npx-cli.js
        let dir = std::env::temp_dir().join(format!("pi-npx-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let node_dir = dir.join("bin");
        std::fs::create_dir_all(&node_dir).unwrap();
        std::fs::write(node_dir.join("node"), "fake").unwrap();
        let cli = dir.join("lib").join("node_modules").join("npm").join("bin");
        std::fs::create_dir_all(&cli).unwrap();
        std::fs::write(cli.join("npx-cli.js"), "fake").unwrap();

        let exec = node_dir.join("node").to_string_lossy().to_string();
        let inv = build_npx_invocation(&["ls".to_string(), "pkg".to_string()], &exec);
        assert_eq!(inv.command, exec);
        assert_eq!(
            inv.command_args[0],
            cli.join("npx-cli.js").to_string_lossy().to_string()
        );
        assert_eq!(inv.command_args[1..], ["ls".to_string(), "pkg".to_string()]);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
