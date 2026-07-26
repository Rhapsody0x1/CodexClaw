use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IncomingMessage {
    pub sender_openid: String,
    pub message_id: String,
    pub text: String,
    pub quote: Option<QuotedMessage>,
    pub images: Vec<IncomingAttachment>,
    pub files: Vec<IncomingAttachment>,
    pub mentions: Vec<Mention>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuotedMessage {
    pub message_id: Option<String>,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IncomingAttachment {
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub source_url: String,
    pub local_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Mention {
    pub target_id: Option<String>,
    pub display: Option<String>,
    pub is_self: bool,
}
