use anyhow::Result;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::signal;

mod executor;
mod handlers;
mod node_runtime_trace;
mod ws_client;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeIdentityFile {
    pub device_id: String,
    pub public_key_b64: String,
    pub private_key_b64: String,
    pub gateway: GatewayConfig,
    #[serde(default)]
    pub display_name: Option<String>,
}

fn identity_path(workspace_dir: &PathBuf) -> PathBuf {
    let mut dir = workspace_dir.clone();
    dir.push("identity");
    std::fs::create_dir_all(&dir).ok();
    dir.push("device.json");
    dir
}

fn load_or_create_identity(
    workspace_dir: &PathBuf,
    display_name: &str,
    host: String,
    port: u16,
    token: Option<String>,
) -> Result<NodeIdentityFile> {
    let path = identity_path(workspace_dir);
    if path.exists() {
        let data = std::fs::read_to_string(&path)?;
        let mut id: NodeIdentityFile = serde_json::from_str(&data)?;
        // backfill gateway from CLI/config if missing
        id.gateway.host = if id.gateway.host.is_empty() {
            host
        } else {
            id.gateway.host
        };
        id.gateway.port = if id.gateway.port == 0 { port } else { id.gateway.port };
        if token.is_some() {
            id.gateway.token = token;
        }
        // update display_name from CLI if provided
        if !display_name.is_empty() {
            id.display_name = Some(display_name.to_string());
        }
        let updated = serde_json::to_string_pretty(&id)?;
        std::fs::write(&path, updated)?;
        return Ok(id);
    }

    // generate opaque random bytes as key material
    let pub_bytes: [u8; 32] = rand::random();
    let priv_bytes: [u8; 64] = rand::random();

    let device_id = format!("zeroclaw-node-{}", uuid::Uuid::new_v4());
    let public_key_b64 =
        base64::engine::general_purpose::STANDARD.encode(pub_bytes);
    let private_key_b64 =
        base64::engine::general_purpose::STANDARD.encode(priv_bytes);

    let id = NodeIdentityFile {
        device_id,
        public_key_b64,
        private_key_b64,
        gateway: GatewayConfig { host, port, token },
        display_name: Some(display_name.to_string()),
    };

    let json = serde_json::to_string_pretty(&id)?;
    std::fs::write(&path, json)?;
    Ok(id)
}

pub async fn run_node(
    config: &crate::config::Config,
    init: bool,
    config_path: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    name: Option<String>,
    token: Option<String>,
) -> Result<()> {
    #[derive(Deserialize)]
    struct NodeConfigFile {
        #[serde(default)]
        display_name: Option<String>,
        #[serde(default)]
        gateway: Option<GatewayConfig>,
    }

    let mut display_name = name.clone();
    let mut cfg_host: Option<String> = None;
    let mut cfg_port: Option<u16> = None;
    let mut cfg_token: Option<String> = None;

    if let Some(path) = config_path.as_deref() {
        if !path.trim().is_empty() {
            let path_buf = PathBuf::from(path);
            if path_buf.exists() {
                let data = std::fs::read_to_string(&path_buf)?;
                let file_cfg: NodeConfigFile = serde_json::from_str(&data)?;
                if let Some(dn) = file_cfg.display_name {
                    if !dn.trim().is_empty() {
                        display_name = Some(dn);
                    }
                }
                if let Some(gw) = file_cfg.gateway {
                    if !gw.host.trim().is_empty() {
                        cfg_host = Some(gw.host);
                    }
                    if gw.port != 0 {
                        cfg_port = Some(gw.port);
                    }
                    cfg_token = gw.token;
                }
            }
        }
    }

    let gateway_host = host
        .or(cfg_host)
        .unwrap_or_else(|| config.gateway.host.clone());
    let gateway_port = port.or(cfg_port).unwrap_or(config.gateway.port);
    let final_token = token.or(cfg_token);

    let workspace_dir = config.workspace_dir.clone();
    let effective_name = display_name.unwrap_or_else(|| {
        hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "zeroclaw-node".to_string())
    });

    let identity = load_or_create_identity(
        &workspace_dir,
        &effective_name,
        gateway_host.clone(),
        gateway_port,
        final_token,
    )?;

    if init {
        println!(
            "Initialized node identity at {} (device_id={})",
            identity_path(&workspace_dir).display(),
            identity.device_id
        );
        return Ok(());
    }

    let url = format!("ws://{}:{}/", identity.gateway.host, identity.gateway.port);
    let stop = signal::ctrl_c();

    ws_client::run_loop(url, &identity, stop).await
}

