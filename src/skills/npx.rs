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

// ── runNpx 执行器 ────────────────────────────────────────────────────────

/// 对齐 `RunNpxOptions`(timeout/cwd/env 覆盖项)。
pub struct RunNpxOptions {
    pub timeout: std::time::Duration,
    pub cwd: Option<String>,
    /// 在继承的环境上追加/覆盖(上游 `{ ...process.env, ...opts.env }`)。
    pub env: Vec<(String, String)>,
}

impl Default for RunNpxOptions {
    fn default() -> Self {
        Self { timeout: std::time::Duration::from_secs(60), cwd: None, env: Vec::new() }
    }
}

/// 对齐 `RunNpxResult`。
#[derive(Debug)]
pub struct RunNpxResult {
    pub stdout: String,
    pub stderr: String,
}

/// 失败携带部分输出(上游 catch 读 `err.stdout/err.stderr` 拼 error 文案)。
#[derive(Debug)]
pub struct RunNpxError {
    pub stdout: String,
    pub stderr: String,
    pub message: String,
}

/// 对齐 `runNpx`:无 shell 调用(用户参数永不被 shell 解释);超时击杀。
///
/// 与上游的差异:上游用 `process.execPath`(服务进程自己的 node)定位
/// npx-cli.js;Rust 宿主无自带 node → PATH 上的 `node` 定位,找不到则回退
/// PATH 上的 `npx`(与上游 fallback 相同)。
pub fn run_npx(args: &[String], opts: &RunNpxOptions) -> Result<RunNpxResult, RunNpxError> {
    let node_on_path = which_on_path("node");
    let inv = match &node_on_path {
        Some(node) => build_npx_invocation(args, node),
        None => NpxInvocation { command: "npx".to_string(), command_args: args.to_vec() },
    };
    let mut cmd = std::process::Command::new(&inv.command);
    cmd.args(&inv.command_args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    for (k, v) in &opts.env {
        cmd.env(k, v);
    }
    if let Some(cwd) = &opts.cwd {
        cmd.current_dir(cwd);
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return Err(RunNpxError {
                stdout: String::new(),
                stderr: String::new(),
                message: format!("spawn {} failed: {e}", inv.command),
            })
        }
    };
    // 双线程收管道:子进程写满 pipe 缓冲而我们不读 → 死锁,必须并发收
    let mut out_pipe = child.stdout.take().expect("piped stdout");
    let mut err_pipe = child.stderr.take().expect("piped stderr");
    let out_t = std::thread::spawn(move || read_all(&mut out_pipe));
    let err_t = std::thread::spawn(move || read_all(&mut err_pipe));

    let deadline = std::time::Instant::now() + opts.timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if std::time::Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(RunNpxError {
                    stdout: String::new(),
                    stderr: String::new(),
                    message: format!("wait failed: {e}"),
                });
            }
        }
    };
    let stdout = out_t.join().unwrap_or_default();
    let stderr = err_t.join().unwrap_or_default();
    match status {
        Some(s) if s.success() => Ok(RunNpxResult { stdout, stderr }),
        Some(s) => Err(RunNpxError {
            stdout,
            stderr,
            message: format!("npx exited with {s}"),
        }),
        None => Err(RunNpxError { stdout, stderr, message: "npx timed out".to_string() }),
    }
}

fn read_all<R: std::io::Read>(r: &mut R) -> String {
    let mut buf = Vec::new();
    let _ = std::io::Read::read_to_end(r, &mut buf);
    String::from_utf8_lossy(&buf).into_owned()
}

fn which_on_path(bin: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(bin);
        if candidate.is_file() {
            return Some(candidate.to_string_lossy().into_owned());
        }
    }
    None
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
    fn run_npx_missing_binary_reports_spawn_failure() {
        // PATH 里没有的命令 → spawn 失败,message 含 spawn
        let e = run_npx(
            &["--version".to_string()],
            &RunNpxOptions {
                timeout: std::time::Duration::from_secs(5),
                cwd: Some("/nonexistent-dir-for-pi-npx-test".to_string()),
                env: Vec::new(),
            },
        )
        .expect_err("must fail");
        assert!(e.message.contains("spawn"), "msg: {}", e.message);
    }

    #[test]
    fn run_npx_captures_output_and_env() {
        // 环境里必有 cat/echo 类基础工具;用 sh -c 不行(无 shell),直接调 npx
        // 可能不存在 —— 此测试验证的是"成功路径捕获输出":若机器无 node/npx
        // 则跳过(开发机/CI 均有 node)。
        if which_on_path("node").is_none() && which_on_path("npx").is_none() {
            eprintln!("node/npx not on PATH; skipping");
            return;
        }
        let r = run_npx(
            &["--version".to_string()],
            &RunNpxOptions {
                timeout: std::time::Duration::from_secs(60),
                cwd: None,
                env: vec![("FORCE_COLOR".to_string(), "0".to_string())],
            },
        )
        .expect("npx --version should succeed on a machine with node");
        assert!(!r.stdout.trim().is_empty() || !r.stderr.trim().is_empty());
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
