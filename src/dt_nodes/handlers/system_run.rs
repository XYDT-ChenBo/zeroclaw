use crate::dt_nodes::handlers::InvokeOutcome;
use serde::Deserialize;
use serde_json::Value;
use tokio::process::Command;
use tokio::time::{timeout, Duration};

const OUTPUT_CAP: usize = 200_000;

#[derive(Deserialize)]
struct SystemRunParams {
    #[serde(rename = "command")]
    #[serde(default)]
    command: Vec<String>,
    #[serde(rename = "rawCommand")]
    #[serde(default)]
    raw_command: Option<String>,
    #[serde(rename = "cwd")]
    #[serde(default)]
    cwd: Option<String>,
    #[serde(rename = "env")]
    #[serde(default)]
    env: Option<Value>,
    #[serde(rename = "timeoutMs")]
    #[serde(default)]
    timeout_ms: Option<i64>,
}

#[derive(serde::Serialize)]
struct RunResult {
    #[serde(rename = "exitCode")]
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    #[serde(rename = "timedOut")]
    timed_out: bool,
    success: bool,
    stdout: String,
    stderr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    truncated: bool,
}

pub async fn handle_system_run(params_json: &str) -> InvokeOutcome {
    let parsed = match parse_params(params_json) {
        Ok(p) => p,
        Err(err) => {
            return InvokeOutcome {
                ok: false,
                payload_json: None,
                error: Some(err),
            }
        }
    };

    match run_command(parsed).await {
        Ok(run) => {
            let success = run.success;
            let payload_json =
                serde_json::to_string(&run).unwrap_or_else(|_| "{}".to_string());
            InvokeOutcome {
                ok: success,
                payload_json: Some(payload_json),
                error: None,
            }
        }
        Err(err) => InvokeOutcome {
            ok: false,
            payload_json: None,
            error: Some(err),
        },
    }
}

fn parse_params(params_json: &str) -> Result<SystemRunParams, Value> {
    let trimmed = params_json.trim();
    if trimmed.is_empty() {
        return Err(invalid_request("paramsJSON required"));
    }
    serde_json::from_str::<SystemRunParams>(trimmed).map_err(|e| {
        invalid_request(&format!("invalid paramsJSON: {}", e))
    })
}

fn invalid_request(msg: &str) -> Value {
    serde_json::json!({
        "code": "INVALID_REQUEST",
        "message": msg,
    })
}

async fn run_command(params: SystemRunParams) -> Result<RunResult, Value> {
    let mut argv = params.command;

    if argv.is_empty() {
        if let Some(raw) = params.raw_command.as_ref().map(|s| s.trim()).filter(|s| !s.is_empty())
        {
            let (shell, args) = shell_exec();
            let mut full = Vec::with_capacity(args.len() + 2);
            full.push(shell.to_string());
            full.extend(args.iter().map(|s| s.to_string()));
            full.push(raw.to_string());
            argv = full;
        }
    }

    if argv.is_empty() {
        return Err(invalid_request("command required"));
    }

    let mut cmd = Command::new(&argv[0]);
    if argv.len() > 1 {
        cmd.args(&argv[1..]);
    }

    if let Some(cwd) = params.cwd {
        if !cwd.trim().is_empty() {
            cmd.current_dir(cwd);
        }
    }

    if let Some(env) = params.env {
        if let Some(obj) = env.as_object() {
            for (k, v) in obj {
                if let Some(s) = v.as_str() {
                    cmd.env(k, s);
                } else {
                    cmd.env(k, v.to_string());
                }
            }
        }
    }

    let timeout_ms = params.timeout_ms.unwrap_or(60_000);
    let timeout_ms = if timeout_ms <= 0 { 60_000 } else { timeout_ms };

    let output = match timeout(
        Duration::from_millis(timeout_ms as u64),
        cmd.output(),
    )
    .await
    {
        Ok(Ok(out)) => out,
        Ok(Err(e)) => {
            return Ok(RunResult {
                exit_code: None,
                timed_out: false,
                success: false,
                stdout: String::new(),
                stderr: String::new(),
                error: Some(e.to_string()),
                truncated: false,
            })
        }
        Err(_) => {
            return Ok(RunResult {
                exit_code: None,
                timed_out: true,
                success: false,
                stdout: String::new(),
                stderr: String::new(),
                error: Some("command timeout".to_string()),
                truncated: false,
            })
        }
    };

    let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let mut truncated = false;

    if stdout.len() > OUTPUT_CAP {
        stdout = format!(
            "... (truncated) {}",
            &stdout[stdout.len() - OUTPUT_CAP..]
        );
        truncated = true;
    }
    if stderr.len() > OUTPUT_CAP {
        stderr = format!(
            "... (truncated) {}",
            &stderr[stderr.len() - OUTPUT_CAP..]
        );
        truncated = true;
    }

    let exit_code = output.status.code().unwrap_or(-1);
    let success = output.status.success();

    Ok(RunResult {
        exit_code: Some(exit_code),
        timed_out: false,
        success,
        stdout,
        stderr,
        error: None,
        truncated,
    })
}

fn shell_exec() -> (&'static str, [&'static str; 1]) {
    if cfg!(target_os = "windows") {
        ("cmd.exe", ["/c"])
    } else {
        ("/bin/sh", ["-c"])
    }
}

