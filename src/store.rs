use std::collections::HashMap;
use crate::formatting::{
    canonicalize_jid, format_phone_number, load_disk_lid_mappings, resolve_mentions, resolve_sender_name,
};

pub async fn clear_unread_in_store(device_id: &str, target_jid: &str) {
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

pub async fn load_chats_for_device(device_id: &str, limit: usize) -> serde_json::Value {
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

pub async fn load_messages_for_device(device_id: &str, jid: &str, limit: usize, before_ts: Option<i64>) -> serde_json::Value {
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
