use serde::Deserialize;

pub(crate) const DISPATCH_EVENT: u32 = 0;
pub(crate) const HEARTBEAT_EVENT: u32 = 1;
pub(crate) const IDENTIFY_EVENT: u32 = 2;
pub(crate) const RESUME_EVENT: u32 = 6;
pub(crate) const RECONNECT_EVENT: u32 = 7;
pub(crate) const INVALID_SESSION_EVENT: u32 = 9;
pub(crate) const HELLO_EVENT: u32 = 10;
pub(crate) const HEARTBEAT_ACK_EVENT: u32 = 11;
pub(crate) const MSG_TYPE_QUOTE: u32 = 103;
pub(crate) const INTENT_GROUP_AND_C2C: u32 = 1 << 25;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GatewayEnvelope {
    pub(crate) op: u32,
    #[serde(default)]
    pub(crate) d: serde_json::Value,
    #[serde(default)]
    pub(crate) s: Option<u64>,
    #[serde(default)]
    pub(crate) t: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GatewayInfo {
    pub(crate) url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct HelloPayload {
    pub(crate) heartbeat_interval: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct ReadyPayload {
    pub(crate) session_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct C2CMessageEvent {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) content: String,
    pub(crate) author: EventAuthor,
    #[serde(default)]
    pub(crate) attachments: Vec<MessageAttachment>,
    #[serde(default)]
    pub(crate) message_type: Option<u32>,
    #[serde(default)]
    pub(crate) msg_elements: Vec<MsgElement>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct EventAuthor {
    pub(crate) user_openid: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MessageAttachment {
    pub(crate) content_type: String,
    pub(crate) url: String,
    #[serde(default)]
    pub(crate) filename: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct MsgElement {
    #[serde(default)]
    pub(crate) msg_idx: Option<String>,
    #[serde(default)]
    pub(crate) content: Option<String>,
    #[serde(default)]
    pub(crate) attachments: Vec<MessageAttachment>,
    #[serde(default)]
    pub(crate) msg_elements: Vec<MsgElement>,
}
