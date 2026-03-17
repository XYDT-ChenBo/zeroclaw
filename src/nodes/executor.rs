use crate::nodes::handlers::{camera_snap, file_save, system_run, Handler, InvokeOutcome};

pub async fn handle_invoke(command: &str, params_json: &str) -> InvokeOutcome {
    match command {
        "system.run" => system_run::handle_system_run(params_json).await,
        "media.saveImage" => file_save::FileSaveHandler::new().handle(params_json),
        "camera.snap" => camera_snap::CameraSnapHandler::new().handle(params_json),
        other => InvokeOutcome {
            ok: false,
            payload_json: None,
            error: Some(serde_json::json!({
                "code": "unsupported_command",
                "message": format!("command '{other}' is not implemented on zeroclaw node"),
            })),
        },
    }
}

