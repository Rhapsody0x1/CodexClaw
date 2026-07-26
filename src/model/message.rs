use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct IncomingMessage {
    pub(crate) sender_openid: String,
    pub(crate) message_id: String,
    pub(crate) text: String,
    pub(crate) quote: Option<QuotedMessage>,
    pub(crate) images: Vec<IncomingAttachment>,
    pub(crate) files: Vec<IncomingAttachment>,
    pub(crate) mentions: Vec<Mention>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct QuotedMessage {
    pub(crate) message_id: Option<String>,
    pub(crate) text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct IncomingAttachment {
    pub(crate) filename: Option<String>,
    pub(crate) content_type: Option<String>,
    pub(crate) source_url: String,
    pub(crate) local_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Mention {
    pub(crate) target_id: Option<String>,
    pub(crate) display: Option<String>,
    pub(crate) is_self: bool,
}
