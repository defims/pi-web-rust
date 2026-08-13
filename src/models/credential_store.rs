//! 对齐 `lib/provider-credential-store.ts`。provider 凭证的 auth.json 存储。
//!
//! - `ensureAuthFile`:递归建目录(0700)+ 缺失时写 `{}`(0600)
//! - `updateStoredCredentials`:跨进程锁(对齐 proper-lockfile 的重试/退避/stale/
//!   onCompromised 语义)→ 读改写(仅 changed 时落盘)
//! - `store_provider_credential` / `remove_stored_credential_if_type`
//!
//! 锁与 pi 的 AuthStorage 共享同一把锁文件,避免并发登录被过期 UI 请求误删。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

/// 对齐 `AUTH_FILE_WRITE_OPTIONS.mode = 0o600`。
const AUTH_FILE_MODE: u32 = 0o600;
/// 对齐 `mkdirSync(..., { recursive: true, mode: 0o700 })`。
const AUTH_DIR_MODE: u32 = 0o700;

/// 对齐 `ProviderCredentialType`。
pub type ProviderCredentialType = String;

/// 对齐 `CredentialRemovalResult`。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum CredentialRemovalResult {
    Removed,
    NotFound,
    #[serde(rename_all = "camelCase")]
    TypeMismatch {
        stored_type: String,
    },
}

/// 凭证存储/移除的错误(锁获取失败、锁被劫持、auth.json 损坏、IO 失败)。
#[derive(Debug)]
pub enum CredentialStoreError {
    /// proper-lockfile 的 retries 耗尽仍拿不到锁。
    LockTimeout,
    /// onCompromised:写前发现锁已被其他进程劫持。
    LockCompromised(String),
    /// auth.json 不是 JSON 对象。
    InvalidAuthFile,
    Io(io::Error),
}

impl std::fmt::Display for CredentialStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CredentialStoreError::LockTimeout => f.write_str("could not acquire auth.json lock"),
            CredentialStoreError::LockCompromised(msg) => write!(f, "lock compromised: {msg}"),
            CredentialStoreError::InvalidAuthFile => {
                f.write_str("Invalid auth.json: expected an object")
            }
            CredentialStoreError::Io(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for CredentialStoreError {}

impl From<io::Error> for CredentialStoreError {
    fn from(e: io::Error) -> Self {
        CredentialStoreError::Io(e)
    }
}

/// 对齐 proper-lockfile 的锁配置:重试 10 次,指数退避 100ms→10s,stale 30s。
struct LockOptions {
    retries: u32,
    factor: u64,
    min_timeout: Duration,
    max_timeout: Duration,
    stale: Duration,
}

impl Default for LockOptions {
    fn default() -> Self {
        Self {
            retries: 10,
            factor: 2,
            min_timeout: Duration::from_millis(100),
            max_timeout: Duration::from_secs(10),
            stale: Duration::from_secs(30),
        }
    }
}

/// 持有的锁(RAII:drop 时尝试释放)。
struct HeldLock {
    lock_path: PathBuf,
    token: String,
}

/// 获取 `<authPath>.lock` 的锁。pid + 随机 token 写入锁文件;
/// pid 已死且 mtime 超过 stale → 直接偷锁;否则按退避重试。
fn acquire_lock(auth_path: &Path, options: &LockOptions) -> Result<HeldLock, CredentialStoreError> {
    let lock_path = lock_path_for(auth_path);
    let token = format!(
        "{}:{}:{}",
        std::process::id(),
        lock_token_seed(),
        now_millis()
    );
    let mut attempt: u32 = 0;
    let mut delay = options.min_timeout;
    loop {
        // O_CREAT | O_EXCL:独占创建,已有锁文件时失败
        let created = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path);
        match created {
            Ok(mut file) => {
                use std::io::Write;
                let _ = file.write_all(token.as_bytes());
                return Ok(HeldLock { lock_path, token });
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                // 检查是否 stale(pid 已死且锁文件超龄)
                if let Some(pid) = lock_pid(&lock_path) {
                    if !pid_alive(pid) && lock_mtime_older_than(&lock_path, options.stale) {
                        let _ = fs::remove_file(&lock_path);
                        continue;
                    }
                }
                attempt += 1;
                if attempt > options.retries {
                    return Err(CredentialStoreError::LockTimeout);
                }
                // proper-lockfile 随机化抖动:delay * factor,封顶 max
                let jittered = delay.mul_f64(1.0 + (lock_token_seed() % 21) as f64 / 100.0);
                std::thread::sleep(jittered.min(options.max_timeout));
                delay = (delay * options.factor as u32).min(options.max_timeout);
            }
            Err(e) => return Err(CredentialStoreError::Io(e)),
        }
    }
}

fn lock_path_for(auth_path: &Path) -> PathBuf {
    let mut name = auth_path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

fn lock_token_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 ^ d.as_secs())
        .unwrap_or(0x5eed)
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn lock_pid(lock_path: &Path) -> Option<u32> {
    let content = fs::read_to_string(lock_path).ok()?;
    content.split(':').next()?.parse::<u32>().ok()
}

fn lock_mtime_older_than(lock_path: &Path, duration: Duration) -> bool {
    fs::metadata(lock_path)
        .and_then(|m| m.modified())
        .map(|modified| modified.elapsed().map(|e| e > duration).unwrap_or(false))
        .unwrap_or(false)
}

/// unix:pid 存活检查(kill(pid, 0));非 unix 平台一律视为存活。
fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        libc_kill(pid as i32, 0) == 0
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        true
    }
}

#[cfg(unix)]
fn libc_kill(pid: i32, sig: i32) -> i32 {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe { kill(pid, sig) }
}

impl HeldLock {
    /// 对齐 proper-lockfile 的 `check()`:写前确认锁仍归自己持有。
    fn verify(&self) -> Result<(), CredentialStoreError> {
        match fs::read_to_string(&self.lock_path) {
            Ok(content) if content == self.token => Ok(()),
            _ => Err(CredentialStoreError::LockCompromised(
                "auth.json lock was taken over by another process".to_string(),
            )),
        }
    }
}

impl Drop for HeldLock {
    fn drop(&mut self) {
        // 对齐 finally 里 release() 的"解锁失败静默吞掉"。
        let _ = fs::remove_file(&self.lock_path);
    }
}

/// 对齐 `ensureAuthFile`。递归建父目录(0700),缺失时写 `{}`(0600)。
///
/// 必须在持锁状态下调用(见 `update_stored_credentials`):文件创建是非原子的,
/// 在锁外并发写入会让拿到锁的另一方读到中间状态。
fn ensure_auth_file(auth_path: &Path) -> Result<(), CredentialStoreError> {
    if let Some(parent) = auth_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent)?;
            let _ = set_permissions(parent, AUTH_DIR_MODE);
        }
    }
    if !auth_path.exists() {
        fs::write(auth_path, "{}")?;
        let _ = set_permissions(auth_path, AUTH_FILE_MODE);
    }
    Ok(())
}

#[cfg(unix)]
fn set_permissions(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_permissions(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}

fn is_record(value: &Value) -> bool {
    value.is_object()
}

/// 对齐 `updateStoredCredentials<T>`。
fn update_stored_credentials<F>(auth_path: &Path, update: F) -> Result<Value, CredentialStoreError>
where
    F: FnOnce(&mut serde_json::Map<String, Value>) -> (Value, bool),
{
    // 先取锁再 ensure 文件:文件创建/初始写入与后续读改写共享同一把锁,
    // 避免锁外并发创建时另一持锁线程读到截断中的 auth.json。
    let lock = acquire_lock(auth_path, &LockOptions::default())?;
    let throw_if_compromised = || lock.verify();
    ensure_auth_file(auth_path)?;

    throw_if_compromised()?;
    let content = fs::read_to_string(auth_path)?;
    let mut parsed: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_e) => {
            return Err(CredentialStoreError::InvalidAuthFile);
        }
    };
    if !is_record(&parsed) {
        return Err(CredentialStoreError::InvalidAuthFile);
    }

    let obj = parsed.as_object_mut().unwrap();
    let (result, changed) = update(obj);
    if changed {
        throw_if_compromised()?;
        let pretty = serde_json::to_string_pretty(&parsed).unwrap_or_else(|_| "{}".to_string());
        fs::write(auth_path, pretty)?;
        let _ = set_permissions(auth_path, AUTH_FILE_MODE);
        throw_if_compromised()?;
    }
    Ok(result)
}

/// 对齐 `storeProviderCredential`。不触发 model-catalog 刷新。
pub fn store_provider_credential(
    provider_id: &str,
    credential: &Value,
    auth_path: &Path,
) -> Result<(), CredentialStoreError> {
    update_stored_credentials(auth_path, |credentials| {
        credentials.insert(provider_id.to_string(), credential.clone());
        (Value::Null, true)
    })?;
    Ok(())
}

/// 对齐 `removeStoredCredentialIfType`。仅当存储的 type 与期望一致时才删除。
pub fn remove_stored_credential_if_type(
    provider_id: &str,
    expected_type: &str,
    auth_path: &Path,
) -> Result<CredentialRemovalResult, CredentialStoreError> {
    update_stored_credentials(auth_path, |credentials| {
        if !credentials.contains_key(provider_id) {
            return (
                serde_json::to_value(CredentialRemovalResult::NotFound).unwrap(),
                false,
            );
        }

        let credential = &credentials[provider_id];
        let stored_type = match credential
            .as_object()
            .and_then(|obj| obj.get("type"))
            .and_then(|t| t.as_str())
        {
            Some(t) => t.to_string(),
            None => "unknown".to_string(),
        };
        if stored_type != expected_type {
            return (
                serde_json::to_value(CredentialRemovalResult::TypeMismatch { stored_type })
                    .unwrap(),
                false,
            );
        }

        credentials.remove(provider_id);
        (
            serde_json::to_value(CredentialRemovalResult::Removed).unwrap(),
            true,
        )
    })
    .and_then(|value| {
        serde_json::from_value(value).map_err(|_| CredentialStoreError::InvalidAuthFile)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_auth_path(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pi-web-cred-store-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("auth.json")
    }

    #[test]
    fn store_creates_file_and_entries() {
        let path = temp_auth_path("store");
        store_provider_credential(
            "openai",
            &json!({ "type": "api_key", "apiKey": "sk-1" }),
            &path,
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed["openai"]["type"], "api_key");
        assert_eq!(parsed["openai"]["apiKey"], "sk-1");
        // 再次存储覆盖
        store_provider_credential(
            "openai",
            &json!({ "type": "api_key", "apiKey": "sk-2" }),
            &path,
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(parsed["openai"]["apiKey"], "sk-2");
    }

    #[test]
    fn remove_type_mismatch_keeps_credential() {
        let path = temp_auth_path("mismatch");
        store_provider_credential("openai", &json!({ "type": "oauth", "token": "t" }), &path)
            .unwrap();
        let result = remove_stored_credential_if_type("openai", "api_key", &path).unwrap();
        assert_eq!(
            result,
            CredentialRemovalResult::TypeMismatch {
                stored_type: "oauth".to_string()
            }
        );
        // 凭证未被删除
        let parsed: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(parsed.get("openai").is_some());
    }

    #[test]
    fn remove_matching_type_deletes() {
        let path = temp_auth_path("remove");
        store_provider_credential(
            "openai",
            &json!({ "type": "api_key", "apiKey": "sk-1" }),
            &path,
        )
        .unwrap();
        let result = remove_stored_credential_if_type("openai", "api_key", &path).unwrap();
        assert_eq!(result, CredentialRemovalResult::Removed);
        let parsed: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(parsed.get("openai").is_none());
    }

    #[test]
    fn remove_not_found() {
        let path = temp_auth_path("notfound");
        store_provider_credential("openai", &json!({ "type": "api_key" }), &path).unwrap();
        let result = remove_stored_credential_if_type("anthropic", "api_key", &path).unwrap();
        assert_eq!(result, CredentialRemovalResult::NotFound);
    }

    #[test]
    fn unknown_stored_type() {
        let path = temp_auth_path("unknown");
        store_provider_credential("p", &json!({ "apiKey": "no-type-field" }), &path).unwrap();
        let result = remove_stored_credential_if_type("p", "api_key", &path).unwrap();
        assert_eq!(
            result,
            CredentialRemovalResult::TypeMismatch {
                stored_type: "unknown".to_string()
            }
        );
    }

    #[test]
    fn non_object_auth_file_errors() {
        let path = temp_auth_path("badjson");
        fs::write(&path, "[1,2,3]").unwrap();
        let err = store_provider_credential("p", &json!({ "type": "api_key" }), &path).unwrap_err();
        assert!(matches!(err, CredentialStoreError::InvalidAuthFile));
    }

    #[test]
    fn file_mode_600() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = temp_auth_path("mode");
            store_provider_credential("p", &json!({ "type": "api_key" }), &path).unwrap();
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn serialization_shapes() {
        let removed = serde_json::to_value(CredentialRemovalResult::Removed).unwrap();
        assert_eq!(removed["status"], "removed");
        let mismatch = serde_json::to_value(CredentialRemovalResult::TypeMismatch {
            stored_type: "oauth".to_string(),
        })
        .unwrap();
        assert_eq!(mismatch["status"], "type_mismatch");
        assert_eq!(mismatch["storedType"], "oauth");
    }

    #[test]
    fn concurrent_stores_serialize() {
        let path = temp_auth_path("concurrent");
        let mut handles = Vec::new();
        for i in 0..8 {
            let path = path.clone();
            handles.push(std::thread::spawn(move || {
                let provider = format!("p{i}");
                store_provider_credential(
                    &provider,
                    &json!({ "type": "api_key", "apiKey": format!("sk-{i}") }),
                    &path,
                )
                .unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let parsed: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        // 8 个 provider 全部写入,无丢失
        assert_eq!(parsed.as_object().unwrap().len(), 8);
        for i in 0..8 {
            assert_eq!(parsed[format!("p{i}")]["apiKey"], format!("sk-{i}"));
        }
    }
}
