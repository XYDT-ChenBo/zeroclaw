use crate::dt_nodes::handlers::{Handler, InvokeOutcome};
use base64::Engine;
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

pub struct CameraSnapHandler;

impl CameraSnapHandler {
    pub fn new() -> Self {
        Self
    }
}

impl Handler for CameraSnapHandler {
    fn handle(&self, _params_json: &str) -> InvokeOutcome {
        // 当前实现对齐 picoclaw-node 中的简化版本：从固定路径读取图片并返回 base64。
        let dest_path = default_snapshot_path();

        let data = match read_with_retry(&dest_path, 20, Duration::from_millis(100)) {
            Ok(d) => d,
            Err(e) => {
                return InvokeOutcome {
                    ok: false,
                    payload_json: None,
                    error: Some(unavailable(&format!(
                        "camera.snap: read file {}: {}",
                        dest_path.display(),
                        e
                    ))),
                }
            }
        };

        let b64 = base64::engine::general_purpose::STANDARD.encode(&data);
        // picoclaw 的示例中宽高固定 640x360
        let width = 640;
        let height = 360;

        let ext = dest_path
            .extension()
            .and_then(|v| v.to_str())
            .map(|s| s.trim_start_matches('.').to_ascii_lowercase())
            .unwrap_or_else(|| "jpg".to_string());
        let format = if ext == "jpeg" { "jpg" } else { &ext };

        let payload = serde_json::json!({
            "format": format,
            "base64": b64,
            "width": width,
            "height": height,
        });

        InvokeOutcome {
            ok: true,
            payload_json: Some(payload.to_string()),
            error: None,
        }
    }
}

fn unavailable(msg: &str) -> Value {
    serde_json::json!({
        "code": "UNAVAILABLE",
        "message": msg,
    })
}

fn default_snapshot_path() -> PathBuf {
    // 对齐当前 picoclaw-node 示例中的硬编码路径，方便在现有环境下直接复用。
    // 后续可以改为基于 workDir 的可配置路径。
    PathBuf::from("/home/0668000637/1.png")
}

fn read_with_retry(path: &PathBuf, attempts: usize, delay: Duration) -> std::io::Result<Vec<u8>> {
    let mut last_err = None;
    for _ in 0..attempts {
        match fs::read(path) {
            Ok(data) => return Ok(data),
            Err(e) => {
                last_err = Some(e);
                std::thread::sleep(delay);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "snapshot not found")
    }))
}

