use std::collections::HashMap;
use std::io::Cursor;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, Mutex};
use qrcode::QrCode;
use image::Luma;
use crate::formatting::to_ascii_escaped_json;

pub struct DeviceSession {
    pub device_id: String,
    pub stdin: Arc<Mutex<Option<ChildStdin>>>,
    pub latest_qr_png: Arc<Mutex<Option<Vec<u8>>>>,
    pub latest_qr_raw: Arc<Mutex<Option<String>>>,
    pub status: Arc<Mutex<String>>,
    pub event_tx: broadcast::Sender<String>,
}

pub type DeviceRegistry = Arc<Mutex<HashMap<String, Arc<DeviceSession>>>>;

pub fn generate_qr_png(content: &str) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let code = QrCode::new(content.as_bytes())?;
    let image = code.render::<Luma<u8>>().min_dimensions(200, 200).build();
    let mut bytes: Vec<u8> = Vec::new();
    image.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)?;
    Ok(bytes)
}

pub async fn get_or_create_device(
    registry: DeviceRegistry,
    device_id: String,
) -> Arc<DeviceSession> {
    let mut reg = registry.lock().await;
    if let Some(session) = reg.get(&device_id) {
        return session.clone();
    }

    let (tx, _) = broadcast::channel(500);
    let session = Arc::new(DeviceSession {
        device_id: device_id.clone(),
        stdin: Arc::new(Mutex::new(None)),
        latest_qr_png: Arc::new(Mutex::new(None)),
        latest_qr_raw: Arc::new(Mutex::new(None)),
        status: Arc::new(Mutex::new("initializing".to_string())),
        event_tx: tx,
    });

    reg.insert(device_id.clone(), session.clone());

    let node_path = std::env::var("NODE_PATH").unwrap_or_else(|_| {
        if std::path::Path::new("/Users/lacorte/.nvm/versions/node/v24.16.0/bin/node").exists() {
            "/Users/lacorte/.nvm/versions/node/v24.16.0/bin/node".to_string()
        } else {
            "node".to_string()
        }
    });
    let current_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let script_path = current_dir.join("node-bridge").join("bridge.js");

    let mut child: Child = Command::new(&node_path)
        .arg(&script_path)
        .arg(&device_id)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("Failed to spawn node baileys bridge process");

    let child_stdin = child.stdin.take().expect("Failed to open child stdin");
    *session.stdin.lock().await = Some(child_stdin);

    let child_stdout = child.stdout.take().expect("Failed to open child stdout");
    let session_clone = session.clone();

    tokio::spawn(async move {
        let mut reader = BufReader::new(child_stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            eprintln!("[bridge] {}", line);
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                let msg_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match msg_type {
                    "qr" => {
                        if let Some(qr_str) = v.get("qr").and_then(|q| q.as_str()) {
                            *session_clone.latest_qr_raw.lock().await = Some(qr_str.to_string());
                            if let Ok(png_bytes) = generate_qr_png(qr_str) {
                                *session_clone.latest_qr_png.lock().await = Some(png_bytes);
                            }
                            *session_clone.status.lock().await = "qr_ready".to_string();
                            let _ = session_clone.event_tx.send(to_ascii_escaped_json(&v));
                        }
                    }
                    "connection" => {
                        let status = v.get("status").and_then(|s| s.as_str()).unwrap_or("");
                        if status == "open" {
                            *session_clone.status.lock().await = "connected".to_string();
                            *session_clone.latest_qr_png.lock().await = None;
                            *session_clone.latest_qr_raw.lock().await = None;
                        } else if status == "close" {
                            *session_clone.status.lock().await = "disconnected".to_string();
                        }
                        let _ = session_clone.event_tx.send(to_ascii_escaped_json(&v));
                    }
                    _ => {
                        let _ = session_clone.event_tx.send(to_ascii_escaped_json(&v));
                    }
                }
            }
        }
    });

    session
}
