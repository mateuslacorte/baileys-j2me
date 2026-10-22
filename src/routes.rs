use tokio::io::AsyncWriteExt;
use warp::Filter;
use crate::models::{
    ChatsQuery, MarkReadRequest, MediaQuery, MessagesQuery, SendAudioRequest, SendMessageRequest,
};
use crate::formatting::to_ascii_escaped_json;
use crate::session::{get_or_create_device, DeviceRegistry};
use crate::store::{clear_unread_in_store, load_chats_for_device, load_messages_for_device};

pub fn create_routes(
    registry: DeviceRegistry,
) -> impl Filter<Extract = (impl warp::Reply,), Error = warp::Rejection> + Clone {
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

    status_route
        .or(qr_png_route)
        .or(media_route)
        .or(chats_route)
        .or(messages_route)
        .or(send_route)
        .or(send_audio_route)
        .or(mark_read_route)
        .or(sync_route)
}
