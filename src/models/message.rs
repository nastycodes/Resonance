use serde::{Deserialize, Serialize};

use crate::services::attachment::AttachmentMetadata;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipientDeliveryState {
    Pending,
    Delivered,
    Failed,
    Expired,
    Rejected,
}

impl RecipientDeliveryState {
    pub fn from_api(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "delivered" => Some(Self::Delivered),
            "failed" => Some(Self::Failed),
            "expired" => Some(Self::Expired),
            "rejected" => Some(Self::Rejected),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Delivered => "Delivered",
            Self::Failed => "Failed",
            Self::Expired => "Expired",
            Self::Rejected => "Rejected",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupRecipientDelivery {
    pub recipient_id: String,
    pub state: RecipientDeliveryState,
    #[serde(default)]
    pub api_accepted: bool,
    #[serde(default)]
    pub api_failure_permanent: bool,
    #[serde(default)]
    pub attempts: u8,
    #[serde(default)]
    pub next_attempt_at: Option<String>,
    #[serde(default)]
    pub delivered_at: Option<String>,
    #[serde(default)]
    pub read_at: Option<String>,
    #[serde(default)]
    pub last_error: Option<String>,
    #[serde(default)]
    pub device_available: bool,
    #[serde(default)]
    pub active_devices: u32,
    #[serde(default)]
    pub pending_devices: u32,
    #[serde(default)]
    pub delivered_devices: u32,
    #[serde(default)]
    pub failed_devices: u32,
    #[serde(default)]
    pub rejected_devices: u32,
    #[serde(default)]
    pub expired_devices: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Message {
    pub id: String,
    pub conversation_id: String,
    pub author_id: String,
    pub author_name: String,
    pub body: String,
    #[serde(alias = "time")]
    pub sent_at: String,
    #[serde(default)]
    pub queue_sequence: Option<u64>,
    pub is_mine: bool,
    pub delivery_state: String,
    #[serde(default)]
    pub last_queued_at: Option<String>,
    #[serde(default)]
    pub delivered_at: Option<String>,
    #[serde(default)]
    pub read_at: Option<String>,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub attachment: Option<AttachmentMetadata>,
    #[serde(default)]
    pub attachments: Vec<AttachmentMetadata>,
    #[serde(default)]
    pub encrypted_payload: Option<String>,
    #[serde(default)]
    pub group_deliveries: Vec<GroupRecipientDelivery>,
}

impl Message {
    pub fn new(
        id: &str,
        conversation_id: &str,
        author_id: &str,
        author_name: &str,
        body: &str,
        sent_at: &str,
        is_mine: bool,
    ) -> Self {
        Self {
            id: id.to_string(),
            conversation_id: conversation_id.to_string(),
            author_id: author_id.to_string(),
            author_name: author_name.to_string(),
            body: body.to_string(),
            sent_at: sent_at.to_string(),
            queue_sequence: None,
            is_mine,
            delivery_state: if is_mine {
                "not_delivered".to_string()
            } else {
                "received".to_string()
            },
            last_queued_at: None,
            delivered_at: None,
            read_at: None,
            deleted: false,
            attachment: None,
            attachments: Vec::new(),
            encrypted_payload: None,
            group_deliveries: Vec::new(),
        }
    }

    pub fn attachments(&self) -> impl Iterator<Item = &AttachmentMetadata> {
        self.attachment.iter().chain(self.attachments.iter())
    }

    pub fn preview(&self) -> String {
        if !self.body.trim().is_empty() {
            return self.body.clone();
        }
        if self
            .attachments()
            .any(|attachment| attachment.kind == crate::services::attachment::AttachmentKind::Voice)
        {
            "Voice message".into()
        } else if self.attachments().next().is_some() {
            "File attachment".into()
        } else {
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Message;

    #[test]
    fn older_messages_without_attachment_lists_still_load() {
        let json = r#"{
            "id":"message",
            "conversation_id":"conversation",
            "author_id":"author",
            "author_name":"Author",
            "body":"hello",
            "sent_at":"1",
            "is_mine":false,
            "delivery_state":"received"
        }"#;
        let message: Message = serde_json::from_str(json).unwrap();
        assert!(message.attachments.is_empty());
        assert_eq!(message.preview(), "hello");
    }
}
