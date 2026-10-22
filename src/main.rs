use std::collections::HashMap;
use std::io::Cursor;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::{broadcast, Mutex};
use warp::Filter;
use serde::{Deserialize, Serialize};
use qrcode::QrCode;
use image::Luma;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub to: String,
    pub text: String,
    #[serde(default)]
    pub quoted_id: Option<String>,
    #[serde(default)]
    pub quoted_text: Option<String>,
    #[serde(default)]
    pub quoted_from_me: Option<bool>,
    #[serde(default)]
    pub quoted_participant: Option<String>,
    #[serde(default)]
    pub quoted_sender: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SendAudioRequest {
    pub to: String,
    pub base64: String,
    pub mimetype: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarkReadRequest {
    pub jid: String,
}

#[derive(Deserialize)]
pub struct ChatsQuery {
    pub limit: Option<usize>,
}

#[derive(Deserialize)]
pub struct MessagesQuery {
    pub jid: String,
    pub limit: Option<usize>,
    pub before_ts: Option<i64>,
}

#[derive(Deserialize)]
pub struct MediaQuery {
    pub w: Option<u32>,
}

pub struct DeviceSession {
    pub device_id: String,
    pub stdin: Arc<Mutex<Option<ChildStdin>>>,
    pub latest_qr_png: Arc<Mutex<Option<Vec<u8>>>>,
    pub latest_qr_raw: Arc<Mutex<Option<String>>>,
    pub status: Arc<Mutex<String>>,
    pub event_tx: broadcast::Sender<String>,
}

pub type DeviceRegistry = Arc<Mutex<HashMap<String, Arc<DeviceSession>>>>;

fn generate_qr_png(content: &str) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    let code = QrCode::new(content.as_bytes())?;
    let image = code.render::<Luma<u8>>().min_dimensions(200, 200).build();
    let mut bytes: Vec<u8> = Vec::new();
    image.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)?;
    Ok(bytes)
}

fn to_ascii_escaped_json(val: &serde_json::Value) -> String {
    let raw = serde_json::to_string(val).unwrap_or_else(|_| "{}".to_string());
    let mut out = String::with_capacity(raw.len() * 2);
    for ch in raw.chars() {
        let code = ch as u32;
        if code > 127 {
            if code <= 0xFFFF {
                out.push_str(&format!("\\u{:04x}", code));
            } else {
                let u = code - 0x10000;
                let w1 = 0xD800 + ((u >> 10) & 0x3FF);
                let w2 = 0xDC00 + (u & 0x3FF);
                out.push_str(&format!("\\u{:04x}\\u{:04x}", w1, w2));
            }
        } else {
            out.push(ch);
        }
    }
    out
}

async fn get_or_create_device(
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

fn load_disk_lid_mappings(device_id: &str, lid_map: &mut HashMap<String, String>) {
    let session_dir = format!("sessions/{}", device_id);
    if let Ok(entries) = std::fs::read_dir(&session_dir) {
        for entry in entries.flatten() {
            let fname = entry.file_name();
            let name_str = fname.to_string_lossy();
            if name_str.starts_with("lid-mapping-") {
                if name_str.ends_with("_reverse.json") {
                    let lid_num = name_str.replace("lid-mapping-", "").replace("_reverse.json", "");
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        let phone = content.trim().trim_matches('"');
                        if !phone.is_empty() && !lid_num.is_empty() {
                            lid_map.insert(format!("{}@lid", lid_num), format!("{}@s.whatsapp.net", phone));
                        }
                    }
                } else if name_str.ends_with(".json") {
                    let phone_num = name_str.replace("lid-mapping-", "").replace(".json", "");
                    if let Ok(content) = std::fs::read_to_string(entry.path()) {
                        let lid = content.trim().trim_matches('"');
                        if !lid.is_empty() && !phone_num.is_empty() {
                            lid_map.insert(format!("{}@lid", lid), format!("{}@s.whatsapp.net", phone_num));
                        }
                    }
                }
            }
        }
    }
}

fn format_phone_number(raw: &str) -> String {
    let s = raw.trim();
    if s.is_empty() {
        return String::new();
    }
    // If it's a group or already has letters, don't format as phone
    if s.contains("@g.us") || (s.contains('-') && s.contains('@')) {
        return s.to_string();
    }
    let user_part = s.split('@').next().unwrap_or(s);
    if user_part.chars().any(|c| c.is_alphabetic()) {
        return s.to_string();
    }
    // Check if it's already formatted
    if user_part.starts_with('+') && user_part.contains(' ') {
        return s.to_string();
    }

    let digits: String = user_part.chars().filter(|c| c.is_ascii_digit()).collect();
    let len = digits.len();

    // 1. Brazil (+55)
    if digits.starts_with("55") {
        if len == 13 {
            return format!("+55 {} {}-{}", &digits[2..4], &digits[4..9], &digits[9..13]);
        } else if len == 12 {
            return format!("+55 {} {}-{}", &digits[2..4], &digits[4..8], &digits[8..12]);
        }
    } else if len == 11 && !digits.starts_with('1') {
        return format!("+55 {} {}-{}", &digits[0..2], &digits[2..7], &digits[7..11]);
    } else if len == 10 && !digits.starts_with('1') {
        return format!("+55 {} {}-{}", &digits[0..2], &digits[2..6], &digits[6..10]);
    }

    // 2. US / Canada (+1)
    if digits.starts_with('1') && len == 11 {
        return format!("+1 {} {}-{}", &digits[1..4], &digits[4..7], &digits[7..11]);
    } else if digits.starts_with('1') && len == 10 {
        return format!("+1 {} {}-{}", &digits[1..4], &digits[4..7], &digits[7..10]);
    }

    // 3. UK (+44)
    if digits.starts_with("44") && len >= 11 && len <= 13 {
        return format!("+44 {} {}", &digits[2..6], &digits[6..]);
    }

    // 4. Portugal (+351)
    if digits.starts_with("351") && len == 12 {
        return format!("+351 {} {} {}", &digits[3..6], &digits[6..9], &digits[9..12]);
    }

    // Fallback: if length between 7 and 13 digits (never 14-15 which are WhatsApp internal LIDs)
    if len >= 7 && len <= 13 {
        return format!("+{}", digits);
    }

    raw.to_string()
}

fn resolve_single_mention(digits: &str, lid_map: &HashMap<String, String>, contacts: &HashMap<String, String>) -> String {
    let lid_key = format!("{}@lid", digits);
    let phone_jid_opt = lid_map.get(&lid_key);
    let phone_digits = if let Some(pj) = phone_jid_opt {
        Some(pj.split('@').next().unwrap_or(pj).to_string())
    } else if digits.len() <= 13 && !digits.starts_with('2') {
        Some(digits.to_string())
    } else {
        None
    };

    // 1. Check if contact name is found
    if let Some(ref pj) = phone_jid_opt {
        if let Some(name) = contacts.get(pj.as_str()) {
            if !name.trim().is_empty() && name.chars().any(|c| c.is_alphabetic()) && name != "~" {
                return format!("@{}", name.trim());
            }
        }
    }
    if let Some(ref pd) = phone_digits {
        let direct_phone_jid = format!("{}@s.whatsapp.net", pd);
        if let Some(name) = contacts.get(&direct_phone_jid).or_else(|| contacts.get(pd.as_str())) {
            if !name.trim().is_empty() && name.chars().any(|c| c.is_alphabetic()) && name != "~" {
                return format!("@{}", name.trim());
            }
        }
    }
    let direct_digits_jid = format!("{}@s.whatsapp.net", digits);
    if let Some(name) = contacts.get(&direct_digits_jid).or_else(|| contacts.get(digits)) {
        if !name.trim().is_empty() && name.chars().any(|c| c.is_alphabetic()) && name != "~" {
            return format!("@{}", name.trim());
        }
    }

    // 2. If phone number is available, format it
    if let Some(ref pd) = phone_digits {
        return format!("@{}", format_phone_number(pd));
    }

    // 3. Fallback for unresolved LID
    "@Participante".to_string()
}

fn resolve_mentions(text: &str, lid_map: &HashMap<String, String>, contacts: &HashMap<String, String>) -> String {
    if !text.contains('@') {
        return text.to_string();
    }
    let mut result = String::with_capacity(text.len() + 32);
    let chars: Vec<char> = text.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if chars[i] == '@' {
            let mut j = i + 1;
            while j < len && chars[j].is_ascii_digit() {
                j += 1;
            }
            let digit_count = j - (i + 1);
            let is_email_domain = j < len && chars[j] == '.' && j + 1 < len && chars[j + 1].is_alphabetic();
            if digit_count >= 7 && digit_count <= 25 && !is_email_domain {
                let digits: String = chars[i + 1..j].iter().collect();
                let resolved = resolve_single_mention(&digits, lid_map, contacts);
                result.push_str(&resolved);
                i = j;
                continue;
            }
        }
        result.push(chars[i]);
        i += 1;
    }
    result
}

fn resolve_sender_name(raw_sender: &str, lid_map: &HashMap<String, String>, contacts: &HashMap<String, String>) -> String {
    let s = raw_sender.trim();
    if s.is_empty() {
        return String::new();
    }
    // If it's already a saved name (has alphabetic characters) and not a raw JID
    if s.chars().any(|c| c.is_alphabetic()) && !s.ends_with("@lid") && !s.ends_with("@s.whatsapp.net") && s != "~" {
        return s.to_string();
    }

    let user_part = s.split('@').next().unwrap_or(s);
    let is_lid = s.ends_with("@lid") || (user_part.chars().all(|c| c.is_ascii_digit()) && user_part.len() >= 14 && user_part.starts_with('2'));

    if is_lid {
        let lid_key = format!("{}@lid", user_part);
        if let Some(phone_jid) = lid_map.get(&lid_key) {
            let phone_num = phone_jid.split('@').next().unwrap_or(phone_jid);
            if let Some(name) = contacts.get(phone_jid).or_else(|| contacts.get(phone_num)) {
                if !name.trim().is_empty() && name.chars().any(|c| c.is_alphabetic()) && name != "~" {
                    return name.trim().to_string();
                }
            }
            return format_phone_number(phone_num);
        }
        return "Participante".to_string();
    }

    // Try contacts lookup
    let phone_jid = format!("{}@s.whatsapp.net", user_part);
    if let Some(name) = contacts.get(&phone_jid).or_else(|| contacts.get(user_part)).or_else(|| contacts.get(s)) {
        if !name.trim().is_empty() && name.chars().any(|c| c.is_alphabetic()) && name != "~" {
            return name.trim().to_string();
        }
    }

    // If it's a numeric phone number (7-13 digits)
    if user_part.chars().all(|c| c.is_ascii_digit()) && user_part.len() >= 7 && user_part.len() <= 13 {
        return format_phone_number(user_part);
    }

    format_phone_number(s)
}

fn canonicalize_jid(jid: &str, lid_map: &HashMap<String, String>) -> String {
    let clean = jid.split(':').next().unwrap_or(jid);
    if clean.ends_with("@lid") {
        if let Some(phone_jid) = lid_map.get(clean) {
            return phone_jid.clone();
        }
    }
    clean.to_string()
}

async fn clear_unread_in_store(device_id: &str, target_jid: &str) {
    let path = format!("sessions/{}/chats_store.json", device_id);
    if let Ok(content) = tokio::fs::read_to_string(&path).await {
        if let Ok(mut val) = serde_json::from_str::<serde_json::Value>(&content) {
            let user = target_jid.split('@').next().unwrap_or(target_jid);
            let mut changed = false;
            if let Some(arr) = val.get_mut("chats").and_then(|c| c.as_array_mut()) {
                for c in arr.iter_mut() {
                    let jid = c.get("jid").and_then(|j| j.as_str()).unwrap_or("");
                    let u = jid.split('@').next().unwrap_or(jid);
                    if jid == target_jid || u == user {
                        if c.get("unread").and_then(|u| u.as_i64()).unwrap_or(0) > 0 {
                            c["unread"] = serde_json::json!(0);
                            changed = true;
                        }
                    }
                }
            }
            if changed {
                if let Ok(json_str) = serde_json::to_string_pretty(&val) {
                    let _ = tokio::fs::write(&path, json_str).await;
                }
            }
        }
    }
}

async fn load_chats_for_device(device_id: &str, limit: usize) -> serde_json::Value {
    let path = format!("sessions/{}/chats_store.json", device_id);
    if let Ok(content) = tokio::fs::read_to_string(&path).await {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
            let mut lid_map = HashMap::new();
            if let Some(lm) = val.get("lidMap").and_then(|m| m.as_object()) {
                for (k, v) in lm {
                    if let Some(v_str) = v.as_str() {
                        lid_map.insert(k.clone(), v_str.to_string());
                    }
                }
            }
            load_disk_lid_mappings(device_id, &mut lid_map);

            let mut contacts_map: HashMap<String, String> = HashMap::new();
            if let Some(cm) = val.get("contacts").and_then(|c| c.as_object()) {
                for (k, v) in cm {
                    if let Some(v_str) = v.as_str() {
                        contacts_map.insert(k.clone(), v_str.to_string());
                    }
                }
            }

            let msgs_map = val.get("messages").and_then(|m| m.as_object());
            let mut latest_per_chat: HashMap<String, (i64, String)> = HashMap::new();
            if let Some(mm) = msgs_map {
                for (k, list_val) in mm {
                    let k_canon = canonicalize_jid(k, &lid_map);
                    if let Some(m_arr) = list_val.as_array() {
                        for m in m_arr {
                            let m_ts = m.get("timestamp").and_then(|t| t.as_i64()).unwrap_or(0);
                            let txt = m.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string();
                            let entry = latest_per_chat.entry(k_canon.clone()).or_insert((0, String::new()));
                            if m_ts > entry.0 {
                                *entry = (m_ts, txt.clone());
                            }
                            let raw_entry = latest_per_chat.entry(k.clone()).or_insert((0, String::new()));
                            if m_ts > raw_entry.0 {
                                *raw_entry = (m_ts, txt);
                            }
                        }
                    }
                }
            }

            if let Some(arr) = val.get("chats").and_then(|c| c.as_array()) {
                let mut merged: HashMap<String, serde_json::Value> = HashMap::new();

                for c in arr {
                    let raw_jid = c.get("jid").and_then(|j| j.as_str()).unwrap_or("");
                    if raw_jid.is_empty() {
                        continue;
                    }
                    let canon_jid = canonicalize_jid(raw_jid, &lid_map);
                    let mut name = c.get("name").and_then(|n| n.as_str()).unwrap_or("").trim().to_string();
                    let unread = c.get("unread").and_then(|u| u.as_i64()).unwrap_or(0);

                    let is_group = canon_jid.ends_with("@g.us");
                    let is_unsaved = name.is_empty()
                        || name.chars().all(|ch| ch.is_ascii_digit())
                        || name.contains('\u{2219}')
                        || name.contains('∙')
                        || name == raw_jid
                        || name == canon_jid
                        || name.split('@').next().unwrap_or("") == raw_jid.split('@').next().unwrap_or("");

                    if is_unsaved && !is_group {
                        let phone = canon_jid.split('@').next().unwrap_or(&canon_jid);
                        name = format_phone_number(phone);
                    }

                    let mut real_ts = c.get("timestamp").and_then(|t| t.as_i64()).unwrap_or(0);
                    let mut real_last_msg = c.get("lastMessage").and_then(|m| m.as_str()).unwrap_or("").trim().to_string();

                    // Derive true timestamp and lastMessage from actual messages if available
                    if let Some((msg_ts, msg_txt)) = latest_per_chat.get(&canon_jid) {
                        if *msg_ts > real_ts || real_ts == 0 {
                            real_ts = *msg_ts;
                            if !msg_txt.is_empty() {
                                real_last_msg = msg_txt.clone();
                            }
                        }
                    }
                    if let Some((msg_ts, msg_txt)) = latest_per_chat.get(raw_jid) {
                        if *msg_ts > real_ts {
                            real_ts = *msg_ts;
                            if !msg_txt.is_empty() {
                                real_last_msg = msg_txt.clone();
                            }
                        }
                    }

                    real_last_msg = resolve_mentions(&real_last_msg, &lid_map, &contacts_map);

                    match merged.get_mut(&canon_jid) {
                        Some(existing) => {
                            let ex_ts = existing.get("timestamp").and_then(|t| t.as_i64()).unwrap_or(0);
                            let ex_name = existing.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
                            let ex_unread = existing.get("unread").and_then(|u| u.as_i64()).unwrap_or(0);

                            if real_ts >= ex_ts {
                                if !real_last_msg.is_empty() {
                                    existing["lastMessage"] = serde_json::json!(real_last_msg);
                                }
                                existing["timestamp"] = serde_json::json!(real_ts);
                            }

                            let ex_has_saved_name = !ex_name.is_empty()
                                && !ex_name.starts_with('+')
                                && !ex_name.chars().all(|ch| ch.is_ascii_digit());
                            let new_has_saved_name = !name.is_empty()
                                && !name.starts_with('+')
                                && !name.chars().all(|ch| ch.is_ascii_digit());

                            if new_has_saved_name {
                                existing["name"] = serde_json::json!(name);
                            } else if !ex_has_saved_name && !name.is_empty() {
                                existing["name"] = serde_json::json!(name);
                            }
                            existing["unread"] = serde_json::json!(ex_unread.max(unread));
                        }
                        None => {
                            let mut item = c.clone();
                            item["jid"] = serde_json::json!(canon_jid);
                            item["name"] = serde_json::json!(name);
                            item["timestamp"] = serde_json::json!(real_ts);
                            item["lastMessage"] = serde_json::json!(real_last_msg);
                            merged.insert(canon_jid, item);
                        }
                    }
                }

                // Add any chats that have messages but weren't in the chats array
                for (k_canon, (msg_ts, msg_txt)) in &latest_per_chat {
                    if !merged.contains_key(k_canon) && *msg_ts > 0 {
                        let is_group = k_canon.ends_with("@g.us");
                        let phone = k_canon.split('@').next().unwrap_or(k_canon);
                        let name = if is_group { phone.to_string() } else { format_phone_number(phone) };
                        merged.insert(k_canon.clone(), serde_json::json!({
                            "jid": k_canon,
                            "name": name,
                            "timestamp": *msg_ts,
                            "lastMessage": msg_txt,
                            "unread": 0
                        }));
                    }
                }

                let mut chats = merged.into_iter().map(|(_, v)| v).collect::<Vec<_>>();

                // Second pass to guarantee any unsaved contact is properly formatted
                for ch in chats.iter_mut() {
                    let jid = ch.get("jid").and_then(|j| j.as_str()).unwrap_or("");
                    if !jid.ends_with("@g.us") {
                        let cur_name = ch.get("name").and_then(|n| n.as_str()).unwrap_or("");
                        if cur_name.is_empty()
                            || cur_name.chars().all(|c| c.is_ascii_digit())
                            || cur_name.contains('\u{2219}')
                            || cur_name.contains('∙')
                            || cur_name == jid
                        {
                            let phone = jid.split('@').next().unwrap_or(jid);
                            ch["name"] = serde_json::json!(format_phone_number(phone));
                        }
                    }
                }

                // STRICT CHRONOLOGICAL ORDER: Most recent first!
                chats.sort_by(|a, b| {
                    let a_ts = a.get("timestamp").and_then(|t| t.as_i64()).unwrap_or(0);
                    let b_ts = b.get("timestamp").and_then(|t| t.as_i64()).unwrap_or(0);
                    b_ts.cmp(&a_ts)
                });

                let sliced = chats.into_iter().take(limit).collect::<Vec<_>>();
                return serde_json::json!({
                    "type": "chats",
                    "chats": sliced
                });
            }
        }
    }
    serde_json::json!({
        "type": "chats",
        "chats": []
    })
}

async fn load_messages_for_device(device_id: &str, jid: &str, limit: usize, before_ts: Option<i64>) -> serde_json::Value {
    let path = format!("sessions/{}/chats_store.json", device_id);
    if let Ok(content) = tokio::fs::read_to_string(&path).await {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&content) {
            let mut lid_map = HashMap::new();
            let mut phone_to_lid = HashMap::new();
            if let Some(lm) = val.get("lidMap").and_then(|m| m.as_object()) {
                for (k, v) in lm {
                    if let Some(v_str) = v.as_str() {
                        lid_map.insert(k.clone(), v_str.to_string());
                        phone_to_lid.insert(v_str.to_string(), k.clone());
                    }
                }
            }
            load_disk_lid_mappings(device_id, &mut lid_map);
            for (lid, phone) in &lid_map {
                phone_to_lid.insert(phone.clone(), lid.clone());
            }

            let mut contacts_map: HashMap<String, String> = HashMap::new();
            if let Some(cm) = val.get("contacts").and_then(|c| c.as_object()) {
                for (k, v) in cm {
                    if let Some(v_str) = v.as_str() {
                        contacts_map.insert(k.clone(), v_str.to_string());
                    }
                }
            }

            let canon_jid = canonicalize_jid(jid, &lid_map);
            let user = canon_jid.split('@').next().unwrap_or(&canon_jid);
            let alt_lid = phone_to_lid.get(&canon_jid).cloned().unwrap_or_default();

            if let Some(msgs_map) = val.get("messages").and_then(|m| m.as_object()) {
                let mut all_msgs: Vec<serde_json::Value> = Vec::new();

                for (k, list_val) in msgs_map {
                    let k_canon = canonicalize_jid(k, &lid_map);
                    let k_user = k.split('@').next().unwrap_or(k);

                    if k == jid || k_canon == canon_jid || (!alt_lid.is_empty() && k == &alt_lid) || k_user == user {
                        if let Some(arr) = list_val.as_array() {
                            for m in arr {
                                let m_id = m.get("id").and_then(|i| i.as_str()).unwrap_or("");
                                let m_fm = m.get("fromMe").and_then(|f| f.as_bool()).unwrap_or(false);
                                let m_text = m.get("text").and_then(|t| t.as_str()).unwrap_or("");
                                let m_ts = m.get("timestamp").and_then(|t| t.as_i64()).unwrap_or(0);

                                if let Some(bts) = before_ts {
                                    if m_ts >= bts {
                                        continue;
                                    }
                                }

                                let duplicate = all_msgs.iter().any(|ex| {
                                    let ex_id = ex.get("id").and_then(|i| i.as_str()).unwrap_or("");
                                    let ex_fm = ex.get("fromMe").and_then(|f| f.as_bool()).unwrap_or(false);
                                    let ex_text = ex.get("text").and_then(|t| t.as_str()).unwrap_or("");
                                    let ex_ts = ex.get("timestamp").and_then(|t| t.as_i64()).unwrap_or(0);

                                    (!m_id.is_empty() && ex_id == m_id) ||
                                    (ex_fm == m_fm && ex_text == m_text && (ex_ts - m_ts).abs() < 15000)
                                });

                                if !duplicate {
                                    let mut m_obj = m.clone();
                                    let part_opt = m_obj.get("participant").and_then(|p| p.as_str()).map(|s| s.to_string());
                                    if let Some(part) = part_opt {
                                        let canon_part = canonicalize_jid(&part, &lid_map);
                                        m_obj["participant"] = serde_json::Value::String(canon_part.clone());

                                        let cur_name = m_obj.get("senderName").and_then(|s| s.as_str()).unwrap_or("").trim().to_string();
                                        if cur_name.is_empty() || cur_name.chars().all(|c| c.is_ascii_digit()) || cur_name == "~" || cur_name.ends_with("@lid") {
                                            let resolved = resolve_sender_name(&canon_part, &lid_map, &contacts_map);
                                            if !resolved.is_empty() {
                                                m_obj["senderName"] = serde_json::Value::String(resolved);
                                            }
                                        }
                                    }

                                    // Resolve mentions in message text
                                    if let Some(txt) = m_obj.get("text").and_then(|t| t.as_str()) {
                                        let res_txt = resolve_mentions(txt, &lid_map, &contacts_map);
                                        m_obj["text"] = serde_json::Value::String(res_txt);
                                    }

                                    // Resolve mentions in quoted text
                                    if let Some(q_txt) = m_obj.get("quotedText").and_then(|t| t.as_str()) {
                                        let res_qtxt = resolve_mentions(q_txt, &lid_map, &contacts_map);
                                        m_obj["quotedText"] = serde_json::Value::String(res_qtxt);
                                    }

                                    // Resolve quoted sender name or phone format
                                    if let Some(q_sender) = m_obj.get("quotedSender").and_then(|s| s.as_str()) {
                                        let res_qsender = resolve_sender_name(q_sender, &lid_map, &contacts_map);
                                        m_obj["quotedSender"] = serde_json::Value::String(res_qsender);
                                    }

                                    let has_img = m_obj.get("hasImage").and_then(|h| h.as_bool()).unwrap_or(false)
                                        || m_text == "[Imagem]"
                                        || tokio::fs::try_exists(&format!("sessions/{}/media/{}.jpg", device_id, m_id)).await.unwrap_or(false);
                                    if has_img {
                                        m_obj["hasImage"] = serde_json::Value::Bool(true);
                                    }

                                    all_msgs.push(m_obj);
                                }
                            }
                        }
                    }
                }

                // STRICT CHRONOLOGICAL ORDER: Oldest -> Newest!
                all_msgs.sort_by(|a, b| {
                    let a_ts = a.get("timestamp").and_then(|t| t.as_i64()).unwrap_or(0);
                    let b_ts = b.get("timestamp").and_then(|t| t.as_i64()).unwrap_or(0);
                    a_ts.cmp(&b_ts)
                });

                let len = all_msgs.len();
                let start = if len > limit { len - limit } else { 0 };
                let sliced = all_msgs[start..].to_vec();
                let has_more = start > 0;

                if before_ts.is_none() {
                    clear_unread_in_store(device_id, &canon_jid).await;
                }

                return serde_json::json!({
                    "type": "messages",
                    "jid": canon_jid,
                    "messages": sliced,
                    "hasMore": has_more
                });
            }
        }
    }
    serde_json::json!({
        "type": "messages",
        "jid": jid,
        "messages": [],
        "hasMore": false
    })
}

#[tokio::main]
async fn main() {
    println!("=====================================================");
    println!("   Baileys-J2ME Multi-Device Server & Bridge");
    println!("=====================================================");

    let registry: DeviceRegistry = Arc::new(Mutex::new(HashMap::new()));

    // Route: GET /api/device/:device_id/status
    let reg1 = registry.clone();
    let status_route = warp::path!("api" / "device" / String / "status")
        .and(warp::get())
        .and_then(move |device_id: String| {
            let reg = reg1.clone();
            async move {
                let session = get_or_create_device(reg, device_id).await;
                let status = session.status.lock().await.clone();
                let has_qr = session.latest_qr_png.lock().await.is_some();
                let res = serde_json::json!({
                    "device_id": session.device_id,
                    "status": status,
                    "has_qr": has_qr
                });
                Ok::<_, warp::Rejection>(
                    warp::http::Response::builder()
                        .header("Content-Type", "application/json; charset=utf-8")
                        .body(to_ascii_escaped_json(&res))
                        .unwrap()
                )
            }
        });

    // Route: GET /api/device/:device_id/qr.png
    let reg2 = registry.clone();
    let qr_png_route = warp::path!("api" / "device" / String / "qr.png")
        .and(warp::get())
        .and_then(move |device_id: String| {
            let reg = reg2.clone();
            async move {
                let session = get_or_create_device(reg, device_id).await;
                let png_opt = session.latest_qr_png.lock().await.clone();
                match png_opt {
                    Some(bytes) => {
                        Ok::<_, warp::Rejection>(
                            warp::http::Response::builder()
                                .header("Content-Type", "image/png")
                                .body(bytes)
                                .unwrap()
                        )
                    }
                    None => {
                        Ok::<_, warp::Rejection>(
                            warp::http::Response::builder()
                                .status(warp::http::StatusCode::NOT_FOUND)
                                .body(Vec::new())
                                .unwrap()
                        )
                    }
                }
            }
        });

    // Route: GET /api/device/:device_id/media/:msg_id
    let media_route = warp::path!("api" / "device" / String / "media" / String)
        .and(warp::get())
        .and(warp::query::<MediaQuery>())
        .and_then(move |device_id: String, msg_id: String, query: MediaQuery| {
            async move {
                let possible_paths = [
                    format!("sessions/{}/media/{}.jpg", device_id, msg_id),
                    format!("sessions/{}/media/{}.png", device_id, msg_id),
                    format!("sessions/{}/media/{}.jpeg", device_id, msg_id),
                ];

                let mut found_path = None;
                for p in &possible_paths {
                    if tokio::fs::try_exists(p).await.unwrap_or(false) {
                        found_path = Some(p.clone());
                        break;
                    }
                }

                if let Some(path) = found_path {
                    if let Ok(bytes) = tokio::fs::read(&path).await {
                        let max_w = query.w.unwrap_or(240);
                        let response_bytes = if let Ok(dyn_img) = image::load_from_memory(&bytes) {
                            let cur_w = dyn_img.width();
                            let cur_h = dyn_img.height();
                            if cur_w > max_w && cur_w > 0 {
                                let target_h = (cur_h * max_w) / cur_w;
                                let target_h = if target_h < 1 { 1 } else { target_h };
                                let resized = dyn_img.resize_exact(max_w, target_h, image::imageops::FilterType::Triangle);
                                let mut out = Vec::new();
                                let mut cursor = std::io::Cursor::new(&mut out);
                                if resized.write_to(&mut cursor, image::ImageFormat::Jpeg).is_ok() {
                                    out
                                } else {
                                    bytes
                                }
                            } else {
                                bytes
                            }
                        } else {
                            bytes
                        };

                        return Ok::<_, warp::Rejection>(
                            warp::http::Response::builder()
                                .header("Content-Type", "image/jpeg")
                                .header("Cache-Control", "public, max-age=86400")
                                .body(warp::hyper::body::Bytes::from(response_bytes))
                                .unwrap()
                        );
                    }
                }

                Ok::<_, warp::Rejection>(
                    warp::http::Response::builder()
                        .status(warp::http::StatusCode::NOT_FOUND)
                        .body(warp::hyper::body::Bytes::from_static(b"Media not found"))
                        .unwrap()
                )
            }
        });

    // Route: GET /api/device/:device_id/chats?limit=50
    let chats_route = warp::path!("api" / "device" / String / "chats")
        .and(warp::get())
        .and(warp::query::<ChatsQuery>())
        .and_then(move |device_id: String, query: ChatsQuery| {
            async move {
                let limit = query.limit.unwrap_or(50);
                let json_res = load_chats_for_device(&device_id, limit).await;
                Ok::<_, warp::Rejection>(
                    warp::http::Response::builder()
                        .header("Content-Type", "application/json; charset=utf-8")
                        .body(to_ascii_escaped_json(&json_res))
                        .unwrap()
                )
            }
        });

    // Route: GET /api/device/:device_id/messages?jid=...&limit=15&before_ts=...
    let reg_msg = registry.clone();
    let messages_route = warp::path!("api" / "device" / String / "messages")
        .and(warp::get())
        .and(warp::query::<MessagesQuery>())
        .and_then(move |device_id: String, query: MessagesQuery| {
            let reg = reg_msg.clone();
            async move {
                let limit = query.limit.unwrap_or(15);
                let json_res = load_messages_for_device(&device_id, &query.jid, limit, query.before_ts).await;

                if query.jid.contains("@g.us") {
                    if let Some(msgs) = json_res.get("messages").and_then(|m| m.as_array()) {
                        let missing_ids: Vec<String> = msgs.iter().filter_map(|m| {
                            let fm = m.get("fromMe").and_then(|f| f.as_bool()).unwrap_or(false);
                            let part = m.get("participant").and_then(|p| p.as_str()).unwrap_or("");
                            if !fm && part.is_empty() {
                                m.get("id").and_then(|i| i.as_str()).map(|s| s.to_string())
                            } else {
                                None
                            }
                        }).collect();
                        if !missing_ids.is_empty() {
                            let session = get_or_create_device(reg, device_id).await;
                            let cursor_id = msgs.last().and_then(|m| m.get("id")).and_then(|i| i.as_str()).unwrap_or("");
                            let cursor_ts = msgs.last().and_then(|m| m.get("timestamp")).and_then(|t| t.as_i64()).unwrap_or(0);
                            let cmd = serde_json::json!({
                                "action": "fetch_history",
                                "jid": query.jid,
                                "oldestId": cursor_id,
                                "oldestTs": cursor_ts,
                                "missingIds": missing_ids
                            });
                            let mut guard = session.stdin.lock().await;
                            if let Some(stdin) = guard.as_mut() {
                                let mut cmd_str = cmd.to_string();
                                cmd_str.push('\n');
                                let _ = stdin.write_all(cmd_str.as_bytes()).await;
                                let _ = stdin.flush().await;
                            }
                        }
                    }
                }

                Ok::<_, warp::Rejection>(
                    warp::http::Response::builder()
                        .header("Content-Type", "application/json; charset=utf-8")
                        .body(to_ascii_escaped_json(&json_res))
                        .unwrap()
                )
            }
        });

    // Route: POST /api/device/:device_id/send
    let reg4 = registry.clone();
    let send_route = warp::path!("api" / "device" / String / "send")
        .and(warp::post())
        .and(warp::body::json())
        .and_then(move |device_id: String, req: SendMessageRequest| {
            let reg = reg4.clone();
            async move {
                let session = get_or_create_device(reg, device_id).await;
                let cmd = serde_json::json!({
                    "action": "send_message",
                    "to": req.to,
                    "text": req.text,
                    "quotedId": req.quoted_id,
                    "quotedText": req.quoted_text,
                    "quotedFromMe": req.quoted_from_me,
                    "quotedParticipant": req.quoted_participant,
                    "quotedSender": req.quoted_sender,
                    "id": format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis())
                });
                let mut guard = session.stdin.lock().await;
                if let Some(stdin) = guard.as_mut() {
                    let mut cmd_str = cmd.to_string();
                    cmd_str.push('\n');
                    let _ = stdin.write_all(cmd_str.as_bytes()).await;
                    let _ = stdin.flush().await;
                }
                Ok::<_, warp::Rejection>(
                    warp::http::Response::builder()
                        .header("Content-Type", "application/json; charset=utf-8")
                        .body(to_ascii_escaped_json(&serde_json::json!({"status": "queued"})))
                        .unwrap()
                )
            }
        });

    // Route: POST /api/device/:device_id/send_audio
    let reg_audio = registry.clone();
    let send_audio_route = warp::path!("api" / "device" / String / "send_audio")
        .and(warp::post())
        .and(warp::body::json())
        .and_then(move |device_id: String, req: SendAudioRequest| {
            let reg = reg_audio.clone();
            async move {
                let session = get_or_create_device(reg, device_id).await;
                let cmd = serde_json::json!({
                    "action": "send_audio",
                    "to": req.to,
                    "base64": req.base64,
                    "mimetype": req.mimetype.unwrap_or_else(|| "audio/amr".to_string()),
                    "id": format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis())
                });
                let mut guard = session.stdin.lock().await;
                if let Some(stdin) = guard.as_mut() {
                    let mut cmd_str = cmd.to_string();
                    cmd_str.push('\n');
                    let _ = stdin.write_all(cmd_str.as_bytes()).await;
                    let _ = stdin.flush().await;
                }
                Ok::<_, warp::Rejection>(
                    warp::http::Response::builder()
                        .header("Content-Type", "application/json; charset=utf-8")
                        .body(to_ascii_escaped_json(&serde_json::json!({"status": "queued"})))
                        .unwrap()
                )
            }
        });

    // Route: GET /api/device/:device_id/sync (Long polling for J2ME background sync)
    let reg5 = registry.clone();
    let sync_route = warp::path!("api" / "device" / String / "sync")
        .and(warp::get())
        .and_then(move |device_id: String| {
            let reg = reg5.clone();
            async move {
                let session = get_or_create_device(reg, device_id).await;
                let mut rx = session.event_tx.subscribe();
                let event = tokio::time::timeout(tokio::time::Duration::from_secs(20), rx.recv()).await;
                match event {
                    Ok(Ok(ev_str)) => {
                        Ok::<_, warp::Rejection>(
                            warp::http::Response::builder()
                                .header("Content-Type", "application/json; charset=utf-8")
                                .body(ev_str)
                                .unwrap()
                        )
                    }
                    _ => {
                        Ok::<_, warp::Rejection>(
                            warp::http::Response::builder()
                                .header("Content-Type", "application/json; charset=utf-8")
                                .body("{\"type\":\"heartbeat\"}".to_string())
                                .unwrap()
                        )
                    }
                }
            }
        });

    // Route: POST /api/device/:device_id/mark_read
    let reg_read = registry.clone();
    let mark_read_route = warp::path!("api" / "device" / String / "mark_read")
        .and(warp::post())
        .and(warp::body::json())
        .and_then(move |device_id: String, req: MarkReadRequest| {
            let reg = reg_read.clone();
            async move {
                let session = get_or_create_device(reg, device_id.clone()).await;
                let cmd = serde_json::json!({
                    "action": "mark_read",
                    "jid": req.jid
                });
                let mut guard = session.stdin.lock().await;
                if let Some(stdin) = guard.as_mut() {
                    let mut cmd_str = cmd.to_string();
                    cmd_str.push('\n');
                    let _ = stdin.write_all(cmd_str.as_bytes()).await;
                    let _ = stdin.flush().await;
                }
                clear_unread_in_store(&device_id, &req.jid).await;
                Ok::<_, warp::Rejection>(
                    warp::http::Response::builder()
                        .header("Content-Type", "application/json; charset=utf-8")
                        .body(to_ascii_escaped_json(&serde_json::json!({"status": "ok"})))
                        .unwrap()
                )
            }
        });

    let routes = status_route
        .or(qr_png_route)
        .or(media_route)
        .or(chats_route)
        .or(messages_route)
        .or(send_route)
        .or(send_audio_route)
        .or(mark_read_route)
        .or(sync_route);

    let port = std::env::var("PORT").ok().and_then(|p| p.parse::<u16>().ok()).unwrap_or(8085);
    println!("Starting server on 0.0.0.0:{}...", port);
    warp::serve(routes).run(([0, 0, 0, 0], port)).await;
}
