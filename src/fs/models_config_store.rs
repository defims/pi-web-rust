//! 对齐 `lib/models-config-store.ts`。models.json 读写(带原子写入)。

use std::path::{Path, PathBuf};

/// 对齐 `getModelsConfigPath`。默认 ~/.pi/agent/models.json。
pub fn get_models_config_path(agent_dir: Option<&str>) -> PathBuf {
    let dir = agent_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs_home().join(".pi/agent")
        });
    dir.join("models.json")
}

/// 对齐 `readModelsConfig`。读 models.json;不存在或解析失败返回 { providers: {} }。
pub fn read_models_config(models_path: &Path) -> serde_json::Value {
    if !models_path.exists() {
        return serde_json::json!({ "providers": {} });
    }
    match std::fs::read_to_string(models_path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_else(|_| {
            serde_json::json!({ "providers": {} })
        }),
        Err(_) => serde_json::json!({ "providers": {} }),
    }
}

/// 对齐 `writeModelsConfig`。原子写入(委托 atomic_file)。
pub async fn write_models_config(
    data: &serde_json::Value,
    models_path: &Path,
) -> std::io::Result<()> {
    let content = serde_json::to_string_pretty(data)
        .map_err(std::io::Error::other)?;
    if let Some(parent) = models_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    super::atomic_file::write_private_file_atomic_blocking(models_path, &content)
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn write_and_read() {
        let dir = std::env::temp_dir();
        let path = dir.join("pi_web_rust_models_config_test.json");
        let data = json!({"providers": {"test": {"apiKey": "xxx"}}});
        write_models_config(&data, &path).await.unwrap();
        let read = read_models_config(&path);
        assert_eq!(read["providers"]["test"]["apiKey"], "xxx");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn read_nonexistent() {
        let result = read_models_config(Path::new("/nonexistent/models.json"));
        assert_eq!(result["providers"], json!({}));
    }
}
