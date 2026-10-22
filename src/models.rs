use serde::{Deserialize, Serialize};

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
