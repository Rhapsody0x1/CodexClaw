use serde::Deserialize;

pub const DISPATCH_EVENT: u32 = 0;
pub const HEARTBEAT_EVENT: u32 = 1;
pub const IDENTIFY_EVENT: u32 = 2;
pub const RESUME_EVENT: u32 = 6;
pub const RECONNECT_EVENT: u32 = 7;
pub const INVALID_SESSION_EVENT: u32 = 9;
pub const HELLO_EVENT: u32 = 10;
pub const HEARTBEAT_ACK_EVENT: u32 = 11;
pub const MSG_TYPE_QUOTE: u32 = 103;
pub const INTENT_GROUP_AND_C2C: u32 = 1 << 25;

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayEnvelope {
    pub op: u32,
    #[serde(default)]
    pub d: serde_json::Value,
    #[serde(default)]
    pub s: Option<u64>,
    #[serde(default)]
    pub t: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GatewayInfo {
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct HelloPayload {
    pub heartbeat_interval: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReadyPayload {
    pub session_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct C2CMessageEvent {
    pub id: String,
    #[serde(default)]
    pub content: String,
    pub author: EventAuthor,
    #[serde(default)]
    pub attachments: Vec<MessageAttachment>,
    #[serde(default)]
    pub message_type: Option<u32>,
    #[serde(default)]
    pub msg_elements: Vec<MsgElement>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventAuthor {
    pub user_openid: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageAttachment {
    pub content_type: String,
    pub url: String,
    #[serde(default)]
    pub filename: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MsgElement {
    #[serde(default)]
    pub msg_idx: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub attachments: Vec<MessageAttachment>,
    #[serde(default)]
    pub msg_elements: Vec<MsgElement>,
}
