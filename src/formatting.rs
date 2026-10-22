use std::collections::HashMap;

pub fn to_ascii_escaped_json(val: &serde_json::Value) -> String {
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

pub fn format_phone_number(raw: &str) -> String {
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

pub fn load_disk_lid_mappings(device_id: &str, lid_map: &mut HashMap<String, String>) {
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

pub fn resolve_single_mention(digits: &str, lid_map: &HashMap<String, String>, contacts: &HashMap<String, String>) -> String {
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

pub fn resolve_mentions(text: &str, lid_map: &HashMap<String, String>, contacts: &HashMap<String, String>) -> String {
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

pub fn resolve_sender_name(raw_sender: &str, lid_map: &HashMap<String, String>, contacts: &HashMap<String, String>) -> String {
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

pub fn canonicalize_jid(jid: &str, lid_map: &HashMap<String, String>) -> String {
    let clean = jid.split(':').next().unwrap_or(jid);
    if clean.ends_with("@lid") {
        if let Some(phone_jid) = lid_map.get(clean) {
            return phone_jid.clone();
        }
    }
    clean.to_string()
}
