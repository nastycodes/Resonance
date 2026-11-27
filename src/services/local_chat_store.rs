use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    hkdf,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{
    models::{
        conversation::Conversation,
        message::{GroupRecipientDelivery, Message, RecipientDeliveryState},
        user::User,
    },
    services::relay::{RelayDelivery, RelayEnvelope},
};

const FILE_MAGIC: &[u8; 8] = b"RSCHAT01";
const NONCE_LEN: usize = 12;
const MAX_ATTACHMENT_CIPHERTEXT_BYTES: usize = crate::services::attachment::MAX_ATTACHMENT_BYTES;
const MAX_ATTACHMENT_CHUNK_BYTES: usize = 256 * 1024;

fn timestamp_sort_key(timestamp: &str) -> Option<i64> {
    if let Ok(numeric) = timestamp.parse::<i64>() {
        let seconds = if numeric.abs() >= 100_000_000_000 {
            numeric.div_euclid(1000)
        } else {
            numeric
        };
        if let Some(datetime) = chrono::DateTime::<chrono::Utc>::from_timestamp(seconds, 0) {
            return Some(datetime.timestamp());
        }
    }
    chrono::DateTime::parse_from_rfc3339(timestamp)
        .ok()
        .map(|datetime| datetime.timestamp())
}

fn sort_conversations_newest_first(conversations: &mut [Conversation]) {
    conversations.sort_by(|left, right| {
        let left_key = timestamp_sort_key(&left.updated_at);
        let right_key = timestamp_sort_key(&right.updated_at);
        right_key
            .unwrap_or(i64::MIN)
            .cmp(&left_key.unwrap_or(i64::MIN))
            .then_with(|| left.id.cmp(&right.id))
    });
}

fn refresh_group_message_state(message: &mut Message) {
    if message
        .group_deliveries
        .iter()
        .all(|recipient| recipient.state == RecipientDeliveryState::Delivered)
    {
        message.delivery_state = "delivered".into();
    } else if message
        .group_deliveries
        .iter()
        .any(|recipient| recipient.state == RecipientDeliveryState::Pending)
    {
        message.delivery_state = "relayed".into();
    } else {
        message.delivery_state = "failed".into();
    }

    if message
        .group_deliveries
        .iter()
        .all(|recipient| recipient.delivered_at.is_some())
    {
        message.delivered_at = message
            .group_deliveries
            .iter()
            .filter_map(|recipient| recipient.delivered_at.as_ref())
            .max()
            .cloned();
    }
    if message
        .group_deliveries
        .iter()
        .all(|recipient| recipient.read_at.is_some())
    {
        message.read_at = message
            .group_deliveries
            .iter()
            .filter_map(|recipient| recipient.read_at.as_ref())
            .max()
            .cloned();
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct ChatData {
    conversations: Vec<Conversation>,
    messages: Vec<Message>,
    #[serde(default)]
    mls_storage: Vec<u8>,
    #[serde(default)]
    mls_welcomes: HashMap<String, MlsWelcomeRecord>,
    #[serde(default)]
    pending_group_changes: HashMap<String, PendingGroupMembershipChange>,
    #[serde(default)]
    pending_group_commit_deliveries: HashMap<String, Vec<String>>,
    #[serde(default)]
    known_friend_ids: Vec<String>,
    #[serde(default)]
    friend_list_initialized: bool,
    #[serde(default)]
    contact_identities: HashMap<String, ContactIdentity>,
    #[serde(default)]
    mls_credential_changes: HashMap<String, Vec<String>>,
    #[serde(default)]
    attachment_transfer_receipts: HashMap<String, Vec<String>>,
    #[serde(default)]
    pending_inbound_relays: HashMap<String, PendingInboundRelay>,
    #[serde(default)]
    device_identity: Option<DeviceIdentity>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityTrustStatus {
    Unverified,
    Verified,
    Changed,
    Revoked,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactIdentity {
    pub observed_public_key: String,
    pub trusted_public_key: Option<String>,
    pub status: IdentityTrustStatus,
    #[serde(default)]
    pub previous_public_keys: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MlsWelcomeRecord {
    pub welcome: String,
    pub message_id: String,
    pub sent_at: String,
    pub queued_recipients: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PendingGroupMembershipChange {
    pub group_id: String,
    pub operation_id: String,
    pub expected_version: u64,
    pub members: Vec<User>,
    pub deliveries: Vec<RelayDelivery>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingInboundRelay {
    pub sender_public_key: String,
    pub envelope: RelayEnvelope,
    #[serde(default)]
    pub lease_token: Option<String>,
    #[serde(default)]
    pub queue_sequence: Option<u64>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceIdentity {
    pub id: String,
    pub secret: String,
    #[serde(default)]
    pub signing_private_key: String,
    #[serde(default)]
    pub encryption_private_key: String,
    #[serde(default)]
    pub enrollment_request_id: String,
    #[serde(default)]
    pub enrollment_nonce: String,
    #[serde(default = "default_device_name")]
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryAttachment {
    pub attachment_id: String,
    pub ciphertext: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistorySnapshot {
    pub version: u8,
    pub account_public_key: String,
    pub source_device_id: String,
    pub target_device_id: String,
    pub conversations: Vec<Conversation>,
    pub messages: Vec<Message>,
    pub mls_storage: Vec<u8>,
    pub attachments: Vec<HistoryAttachment>,
    pub unavailable_attachment_ids: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HistoryImportResult {
    pub imported_conversations: usize,
    pub imported_messages: usize,
    pub imported_attachments: usize,
    pub unavailable_attachment_ids: Vec<String>,
}

fn default_device_name() -> String {
    "Resonance desktop".into()
}

#[derive(Clone)]
pub struct LocalChatStore {
    path: Option<PathBuf>,
    data: Arc<Mutex<ChatData>>,
    account_id: Option<String>,
    encryption_key: Option<Arc<Zeroizing<[u8; 32]>>>,
    attachment_serving_progress: Arc<Mutex<HashMap<(String, String), AttachmentServingProgress>>>,
}

struct AttachmentServingProgress {
    ranges: Vec<(u64, u64)>,
    total_size: u64,
    started: Instant,
}

impl LocalChatStore {
    pub fn open(account_id: &str, identity_key: &[u8]) -> Result<Self, String> {
        let root = std::env::var_os("LOCALAPPDATA")
            .or_else(|| std::env::var_os("HOME"))
            .or_else(|| std::env::var_os("USERPROFILE"))
            .ok_or_else(|| {
                "Could not locate a per-user data directory for local messages.".to_string()
            })?;
        let directory = PathBuf::from(root).join("Resonance").join("chats");
        Self::load(
            directory.join(format!("{account_id}.json")),
            account_id,
            identity_key,
        )
    }

    #[cfg(test)]
    pub(crate) fn open_at(path: PathBuf, identity_key: &[u8]) -> Result<Self, String> {
        Self::load(path, "test-account", identity_key)
    }

    fn load(path: PathBuf, account_id: &str, identity_key: &[u8]) -> Result<Self, String> {
        let directory = path
            .parent()
            .ok_or_else(|| "The local chat path has no parent directory.".to_string())?;
        fs::create_dir_all(directory)
            .map_err(|error| format!("Could not create the local chat directory: {error}"))?;
        let backup = path.with_extension("bak");
        if !path.exists() && backup.exists() {
            fs::rename(&backup, &path)
                .map_err(|error| format!("Could not restore local chat history: {error}"))?;
        }
        let encryption_key = Arc::new(derive_storage_key(identity_key)?);
        let (data, migrate_legacy) = if path.exists() {
            let bytes = fs::read(&path)
                .map_err(|error| format!("Could not read local chat history: {error}"))?;
            if bytes.starts_with(FILE_MAGIC) {
                let plaintext = decrypt_history(&bytes, account_id, &encryption_key)?;
                let data = serde_json::from_slice(&plaintext)
                    .map_err(|error| format!("Local chat history is invalid: {error}"))?;
                (data, false)
            } else {
                let data = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("Local chat history is invalid: {error}"))?;
                (data, true)
            }
        } else {
            (ChatData::default(), false)
        };
        let store = Self {
            path: Some(path),
            data: Arc::new(Mutex::new(data)),
            account_id: Some(account_id.to_string()),
            encryption_key: Some(encryption_key),
            attachment_serving_progress: Arc::new(Mutex::new(HashMap::new())),
        };
        if migrate_legacy {
            let data = store
                .data
                .lock()
                .map_err(|_| "Local chat history is unavailable.".to_string())?;
            store.persist(&data)?;
        } else if backup.exists() {
            fs::remove_file(&backup)
                .map_err(|error| format!("Could not remove the recovered chat backup: {error}"))?;
        }
        Ok(store)
    }

    #[cfg(test)]
    pub(crate) fn in_memory() -> Self {
        Self {
            path: None,
            data: Arc::new(Mutex::new(ChatData::default())),
            account_id: None,
            encryption_key: None,
            attachment_serving_progress: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn conversations(&self) -> Result<Vec<Conversation>, String> {
        self.data
            .lock()
            .map(|data| {
                let mut conversations = data.conversations.clone();
                sort_conversations_newest_first(&mut conversations);
                conversations
            })
            .map_err(|_| "Local chat history is unavailable.".into())
    }

    pub fn messages(&self) -> Result<Vec<Message>, String> {
        self.data
            .lock()
            .map(|data| data.messages.clone())
            .map_err(|_| "Local chat history is unavailable.".into())
    }

    pub fn set_message_queue_sequence(
        &self,
        message_id: &str,
        queue_sequence: u64,
    ) -> Result<(), String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let Some(message) = data.messages.iter_mut().find(|message| message.id == message_id) else {
            return Ok(());
        };
        if message.queue_sequence == Some(queue_sequence) {
            return Ok(());
        }
        message.queue_sequence = Some(queue_sequence);
        self.persist(&data)
    }

    pub fn save_pending_inbound_relay(
        &self,
        sender_public_key: &str,
        envelope: &RelayEnvelope,
        lease_token: Option<&str>,
        queue_sequence: Option<u64>,
    ) -> Result<(), String> {
        if envelope.message_id.len() != 32
            || !envelope
                .message_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || sender_public_key != envelope.sender_id
        {
            return Err("The pending inbound relay is invalid.".into());
        }
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        if let Some(existing) = data.pending_inbound_relays.get_mut(&envelope.message_id) {
            if existing.sender_public_key != sender_public_key || existing.envelope != *envelope {
                return Err("The pending message identifier conflicts with another relay.".into());
            }
            if let Some(lease_token) = lease_token {
                if existing.lease_token.as_deref() != Some(lease_token) {
                    existing.lease_token = Some(lease_token.to_string());
                    self.persist(&data)?;
                }
            }
            return Ok(());
        }
        if data.pending_inbound_relays.len() >= 10_000 {
            return Err("The encrypted inbound message quarantine is full.".into());
        }
        data.pending_inbound_relays.insert(
            envelope.message_id.clone(),
            PendingInboundRelay {
                sender_public_key: sender_public_key.to_string(),
                envelope: envelope.clone(),
                lease_token: lease_token.map(str::to_string),
                queue_sequence,
            },
        );
        self.persist(&data)
    }

    pub fn device_identity(&self) -> Result<DeviceIdentity, String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let mut identity = data.device_identity.clone().unwrap_or(DeviceIdentity {
            id: new_id()?,
            secret: format!("{}{}", new_id()?, new_id()?),
            signing_private_key: String::new(),
            encryption_private_key: String::new(),
            enrollment_request_id: String::new(),
            enrollment_nonce: String::new(),
            name: default_device_name(),
        });
        if identity.signing_private_key.is_empty() || identity.encryption_private_key.is_empty() {
            let (signing_key, encryption_key) =
                crate::services::device_enrollment::generate_device_keys()?;
            identity.signing_private_key = signing_key;
            identity.encryption_private_key = encryption_key;
        }
        if identity.enrollment_request_id.is_empty() {
            identity.enrollment_request_id = new_id()?;
        }
        if identity.enrollment_nonce.is_empty() {
            identity.enrollment_nonce = format!("{}{}", new_id()?, new_id()?);
        }
        if identity.name.is_empty() {
            identity.name = default_device_name();
        }
        data.device_identity = Some(identity.clone());
        self.persist(&data)?;
        Ok(identity)
    }

    pub fn rename_local_device(&self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
            return Err("The device name must contain 1–80 non-control characters.".into());
        }
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let identity = data
            .device_identity
            .as_mut()
            .ok_or_else(|| "The local device identity is unavailable.".to_string())?;
        identity.name = name.to_string();
        self.persist(&data)
    }

    pub fn restart_device_enrollment(&self) -> Result<DeviceIdentity, String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let identity = data
            .device_identity
            .as_mut()
            .ok_or_else(|| "The local device identity is unavailable.".to_string())?;
        identity.enrollment_request_id = new_id()?;
        identity.enrollment_nonce = format!("{}{}", new_id()?, new_id()?);
        let identity = identity.clone();
        self.persist(&data)?;
        Ok(identity)
    }

    pub fn export_history_snapshot(
        &self,
        account_public_key: &str,
        source_device_id: &str,
        target_device_id: &str,
    ) -> Result<HistorySnapshot, String> {
        let (conversations, messages, mls_storage) = {
            let data = self
                .data
                .lock()
                .map_err(|_| "Local chat history is unavailable.".to_string())?;
            (
                data.conversations.clone(),
                data.messages.clone(),
                data.mls_storage.clone(),
            )
        };
        let attachment_ids = messages
            .iter()
            .flat_map(Message::attachments)
            .map(|attachment| attachment.attachment_id.clone())
            .collect::<HashSet<_>>();
        let mut attachments = Vec::new();
        let mut unavailable_attachment_ids = Vec::new();
        for attachment_id in attachment_ids {
            if self.has_attachment_ciphertext(&attachment_id)? {
                attachments.push(HistoryAttachment {
                    attachment_id: attachment_id.clone(),
                    ciphertext: self.read_attachment_ciphertext(&attachment_id)?,
                });
            } else {
                unavailable_attachment_ids.push(attachment_id);
            }
        }
        Ok(HistorySnapshot {
            version: 1,
            account_public_key: account_public_key.to_ascii_lowercase(),
            source_device_id: source_device_id.to_ascii_lowercase(),
            target_device_id: target_device_id.to_ascii_lowercase(),
            conversations,
            messages,
            mls_storage,
            attachments,
            unavailable_attachment_ids,
        })
    }

    pub fn import_history_snapshot(
        &self,
        snapshot: HistorySnapshot,
        account_public_key: &str,
        target_device_id: &str,
    ) -> Result<HistoryImportResult, String> {
        if snapshot.version != 1
            || snapshot.account_public_key != account_public_key.to_ascii_lowercase()
            || snapshot.target_device_id != target_device_id.to_ascii_lowercase()
        {
            return Err("The history snapshot belongs to another account or device.".into());
        }
        let conversation_ids = snapshot
            .conversations
            .iter()
            .map(|conversation| conversation.id.as_str())
            .collect::<HashSet<_>>();
        if conversation_ids.len() != snapshot.conversations.len()
            || snapshot
                .messages
                .iter()
                .any(|message| !conversation_ids.contains(message.conversation_id.as_str()))
        {
            return Err("The history snapshot contains invalid conversation references.".into());
        }
        let mut attachment_ids = HashSet::new();
        for attachment in &snapshot.attachments {
            if !crate::services::attachment::valid_id(&attachment.attachment_id)
                || !attachment_ids.insert(attachment.attachment_id.as_str())
            {
                return Err("The history snapshot contains invalid attachment references.".into());
            }
            if attachment.ciphertext.len() < 16
                || attachment.ciphertext.len() > MAX_ATTACHMENT_CIPHERTEXT_BYTES
            {
                return Err("A synchronized attachment has an invalid ciphertext size.".into());
            }
        }
        if snapshot.mls_storage.len() > crate::services::mls::MAX_STORED_MLS_BYTES {
            return Err("The synchronized MLS state exceeds the local storage limit.".into());
        }

        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        if !data.mls_storage.is_empty()
            && !snapshot.mls_storage.is_empty()
            && data.mls_storage != snapshot.mls_storage
        {
            return Err(
                "Local MLS groups already exist; history cannot safely replace their state.".into(),
            );
        }
        for conversation in &snapshot.conversations {
            if let Some(existing) = data
                .conversations
                .iter()
                .find(|existing| existing.id == conversation.id)
                && (existing.user_id != conversation.user_id
                    || existing.is_group() != conversation.is_group())
            {
                return Err(
                    "A synchronized conversation conflicts with local conversation identity."
                        .into(),
                );
            }
        }
        for message in &snapshot.messages {
            if let Some(existing) = data
                .messages
                .iter()
                .find(|existing| existing.id == message.id)
                && (existing.conversation_id != message.conversation_id
                    || existing.author_id != message.author_id
                    || existing.body != message.body
                    || existing.sent_at != message.sent_at
                    || existing.attachments != message.attachments
                    || existing.attachment != message.attachment)
            {
                return Err("A synchronized message conflicts with local message identity.".into());
            }
        }
        let mut imported_attachments = 0;
        for attachment in &snapshot.attachments {
            if self.has_attachment_ciphertext(&attachment.attachment_id)? {
                if self.read_attachment_ciphertext(&attachment.attachment_id)?
                    != attachment.ciphertext
                {
                    return Err("A synchronized attachment conflicts with local ciphertext.".into());
                }
            } else {
                self.save_attachment_ciphertext(&attachment.attachment_id, &attachment.ciphertext)?;
                imported_attachments += 1;
            }
        }
        let mut imported_conversations = 0;
        for conversation in snapshot.conversations {
            if !data
                .conversations
                .iter()
                .any(|existing| existing.id == conversation.id)
            {
                data.conversations.push(conversation);
                imported_conversations += 1;
            }
        }
        let mut imported_messages = 0;
        for message in snapshot.messages {
            if !data
                .messages
                .iter()
                .any(|existing| existing.id == message.id)
            {
                data.messages.push(message);
                imported_messages += 1;
            }
        }
        if data.mls_storage.is_empty() {
            data.mls_storage = snapshot.mls_storage;
        }
        self.persist(&data)?;
        Ok(HistoryImportResult {
            imported_conversations,
            imported_messages,
            imported_attachments,
            unavailable_attachment_ids: snapshot.unavailable_attachment_ids,
        })
    }

    pub fn pending_inbound_relays(&self) -> Result<Vec<PendingInboundRelay>, String> {
        let mut pending = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?
            .pending_inbound_relays
            .values()
            .cloned()
            .collect::<Vec<_>>();
        pending.sort_by(|left, right| {
            left.envelope
                .sent_at
                .cmp(&right.envelope.sent_at)
                .then_with(|| left.envelope.message_id.cmp(&right.envelope.message_id))
        });
        Ok(pending)
    }

    pub fn remove_pending_inbound_relay(&self, message_id: &str) -> Result<(), String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        if data.pending_inbound_relays.remove(message_id).is_some() {
            self.persist(&data)?;
        }
        Ok(())
    }

    pub(crate) fn mls_storage_snapshot(&self) -> Result<Vec<u8>, String> {
        self.data
            .lock()
            .map(|data| data.mls_storage.clone())
            .map_err(|_| "Local chat history is unavailable.".into())
    }

    pub(crate) fn save_mls_storage_snapshot(&self, snapshot: Vec<u8>) -> Result<(), String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        data.mls_storage = snapshot;
        self.persist(&data)
    }

    pub fn save_mls_welcome(
        &self,
        conversation_id: &str,
        welcome: &str,
    ) -> Result<MlsWelcomeRecord, String> {
        if conversation_id.len() != 32
            || !conversation_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || welcome.is_empty()
            || welcome.len() > 30_000
            || !welcome.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("The MLS Welcome record is invalid.".into());
        }
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        if let Some(record) = data.mls_welcomes.get(conversation_id) {
            if record.welcome != welcome {
                return Err("The conversation already has a different MLS Welcome.".into());
            }
            return Ok(record.clone());
        }
        let record = MlsWelcomeRecord {
            welcome: welcome.to_string(),
            message_id: new_id()?,
            sent_at: timestamp()?,
            queued_recipients: Vec::new(),
        };
        data.mls_welcomes
            .insert(conversation_id.to_string(), record.clone());
        self.persist(&data)?;
        Ok(record)
    }

    pub fn mls_welcome(&self, conversation_id: &str) -> Result<Option<MlsWelcomeRecord>, String> {
        self.data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())
            .map(|data| data.mls_welcomes.get(conversation_id).cloned())
    }

    pub fn mark_mls_welcome_queued(
        &self,
        conversation_id: &str,
        recipient_id: &str,
    ) -> Result<(), String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let record = data
            .mls_welcomes
            .get_mut(conversation_id)
            .ok_or_else(|| "The MLS Welcome record is unavailable.".to_string())?;
        if !record
            .queued_recipients
            .iter()
            .any(|recipient| recipient == recipient_id)
        {
            record.queued_recipients.push(recipient_id.to_string());
            self.persist(&data)?;
        }
        Ok(())
    }

    pub fn pending_group_change(
        &self,
        group_id: &str,
    ) -> Result<Option<PendingGroupMembershipChange>, String> {
        self.data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())
            .map(|data| data.pending_group_changes.get(group_id).cloned())
    }

    pub fn pending_group_changes(&self) -> Result<Vec<PendingGroupMembershipChange>, String> {
        self.data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())
            .map(|data| data.pending_group_changes.values().cloned().collect())
    }

    pub fn group_commit_delivery_pending(&self, group_id: &str) -> Result<bool, String> {
        self.data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())
            .map(|data| {
                data.pending_group_commit_deliveries
                    .get(group_id)
                    .is_some_and(|message_ids| !message_ids.is_empty())
            })
    }

    pub fn acknowledge_group_commit_deliveries(
        &self,
        delivered_message_ids: &[String],
    ) -> Result<(), String> {
        if delivered_message_ids.is_empty() {
            return Ok(());
        }
        let delivered = delivered_message_ids
            .iter()
            .map(String::as_str)
            .collect::<std::collections::HashSet<_>>();
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let mut changed = false;
        data.pending_group_commit_deliveries
            .retain(|_, message_ids| {
                let original_len = message_ids.len();
                message_ids.retain(|message_id| !delivered.contains(message_id.as_str()));
                changed |= original_len != message_ids.len();
                !message_ids.is_empty()
            });
        if changed {
            self.persist(&data)?;
        }
        Ok(())
    }

    pub fn save_pending_group_change(
        &self,
        change: PendingGroupMembershipChange,
        mls_snapshot: Vec<u8>,
    ) -> Result<(), String> {
        if !valid_group_id(&change.group_id)
            || !valid_group_id(&change.operation_id)
            || change.expected_version == u64::MAX
            || !(3..=12).contains(&change.members.len())
            || change.deliveries.len() < 2
            || change.deliveries.len() > 12
            || !valid_group_members(&change.members)
        {
            return Err("The pending group membership change is invalid.".into());
        }
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        if let Some(existing) = data.pending_group_changes.get(&change.group_id) {
            if existing == &change {
                return Ok(());
            }
            return Err("The group already has another pending membership change.".into());
        }
        let conversation = data
            .conversations
            .iter()
            .find(|conversation| conversation.id == change.group_id && conversation.is_group())
            .ok_or_else(|| "The local group conversation is unavailable.".to_string())?;
        if conversation.membership_version != change.expected_version
            || conversation
                .members
                .first()
                .map(|member| member.id.as_str())
                != change.members.first().map(|member| member.id.as_str())
        {
            return Err("The local group roster changed before the operation was saved.".into());
        }
        data.mls_storage = mls_snapshot;
        data.pending_group_changes
            .insert(change.group_id.clone(), change);
        self.persist(&data)
    }

    pub fn complete_pending_group_change(
        &self,
        group_id: &str,
        operation_id: &str,
        membership_version: u64,
        actual_member_ids: &[String],
        mls_snapshot: Vec<u8>,
    ) -> Result<(), String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let pending = data
            .pending_group_changes
            .get(group_id)
            .ok_or_else(|| "The pending group membership change is unavailable.".to_string())?;
        let expected_version = pending.expected_version;
        let expected_operation_id = pending.operation_id.clone();
        let expected_members = pending.members.clone();
        let current_member_ids = expected_members
            .iter()
            .map(|member| member.id.as_str())
            .collect::<std::collections::HashSet<_>>();
        let delivery_ids = pending
            .deliveries
            .iter()
            .filter(|delivery| current_member_ids.contains(delivery.recipient_public_key.as_str()))
            .map(|delivery| delivery.envelope.message_id.clone())
            .collect::<Vec<_>>();
        if expected_operation_id != operation_id
            || membership_version != expected_version + 1
            || !same_member_ids(&expected_members, actual_member_ids)
        {
            return Err("The committed MLS group does not match the pending roster change.".into());
        }
        let conversation = data
            .conversations
            .iter_mut()
            .find(|conversation| conversation.id == group_id && conversation.is_group())
            .ok_or_else(|| "The local group conversation is unavailable.".to_string())?;
        if conversation.membership_version != expected_version {
            return Err("The local group version changed before the commit completed.".into());
        }
        conversation.members = expected_members;
        conversation.membership_version = membership_version;
        data.mls_storage = mls_snapshot;
        data.pending_group_changes.remove(group_id);
        if !delivery_ids.is_empty() {
            data.pending_group_commit_deliveries
                .insert(group_id.to_string(), delivery_ids);
        }
        self.persist(&data)
    }

    pub fn apply_remote_group_change(
        &self,
        group_id: &str,
        membership_version: u64,
        members: Vec<User>,
        actual_member_ids: &[String],
        actual_owner_id: &str,
        credential_changes: &[String],
        mls_snapshot: Vec<u8>,
    ) -> Result<bool, String> {
        let changed_accounts = credential_changes
            .iter()
            .map(String::as_str)
            .collect::<std::collections::HashSet<_>>();
        if !valid_group_members(&members)
            || members.first().map(|member| member.id.as_str()) != Some(actual_owner_id)
            || !same_member_ids(&members, actual_member_ids)
            || changed_accounts.len() != credential_changes.len()
            || changed_accounts
                .iter()
                .any(|account_id| !actual_member_ids.iter().any(|id| id == account_id))
        {
            return Err(
                "The MLS commit does not match its authenticated roster or credential changes."
                    .into(),
            );
        }
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let conversation = data
            .conversations
            .iter_mut()
            .find(|conversation| conversation.id == group_id && conversation.is_group())
            .ok_or_else(|| "The local group conversation is unavailable.".to_string())?;
        if membership_version <= conversation.membership_version {
            return Ok(false);
        }
        if membership_version != conversation.membership_version + 1
            || conversation
                .members
                .first()
                .map(|member| member.id.as_str())
                != Some(actual_owner_id)
        {
            return Err("The group membership event is stale or out of sequence.".into());
        }
        conversation.members = members;
        conversation.membership_version = membership_version;
        data.mls_storage = mls_snapshot;
        if credential_changes.is_empty() {
            data.mls_credential_changes.remove(group_id);
        } else {
            let mut changed = credential_changes.to_vec();
            changed.sort();
            data.mls_credential_changes
                .insert(group_id.to_string(), changed);
        }
        self.persist(&data)?;
        Ok(true)
    }

    pub fn group_has_credential_change(
        &self,
        group_id: &str,
        account_id: &str,
    ) -> Result<bool, String> {
        self.data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())
            .map(|data| {
                data.mls_credential_changes
                    .get(group_id)
                    .is_some_and(|accounts| accounts.iter().any(|id| id == account_id))
            })
    }

    pub fn sync_friends(&self, friends: &[User]) -> Result<Vec<User>, String> {
        let mut usernames = std::collections::HashSet::new();
        let mut account_ids = std::collections::HashSet::new();
        for friend in friends {
            let username = friend.username.trim().to_ascii_lowercase();
            if username.is_empty()
                || !is_identity_public_key(&friend.id)
                || !usernames.insert(username)
                || !account_ids.insert(friend.id.to_ascii_lowercase())
            {
                return Err("The account API returned inconsistent friend identities.".into());
            }
        }
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let mut identities_changed = false;
        for friend in friends {
            let username = friend.username.trim().to_ascii_lowercase();
            let observed_public_key = friend.id.to_ascii_lowercase();
            match data.contact_identities.get_mut(&username) {
                Some(identity) if identity.observed_public_key != observed_public_key => {
                    if identity
                        .trusted_public_key
                        .as_ref()
                        .is_some_and(|trusted| trusted != &observed_public_key)
                    {
                        identity.status = IdentityTrustStatus::Changed;
                    } else if identity.status != IdentityTrustStatus::Changed
                        && identity.status != IdentityTrustStatus::Revoked
                    {
                        identity.status = IdentityTrustStatus::Unverified;
                    }
                    identity.observed_public_key = observed_public_key;
                    identities_changed = true;
                }
                Some(_) => {}
                None => {
                    data.contact_identities.insert(
                        username,
                        ContactIdentity {
                            observed_public_key,
                            trusted_public_key: None,
                            status: IdentityTrustStatus::Unverified,
                            previous_public_keys: Vec::new(),
                        },
                    );
                    identities_changed = true;
                }
            }
        }
        let new_friends = friends
            .iter()
            .filter(|friend| !data.known_friend_ids.contains(&friend.id))
            .cloned()
            .collect();
        let friend_ids: Vec<_> = friends.iter().map(|friend| friend.id.clone()).collect();
        if !data.friend_list_initialized || friend_ids != data.known_friend_ids {
            data.known_friend_ids = friend_ids;
            data.friend_list_initialized = true;
            identities_changed = true;
        }
        if identities_changed {
            self.persist(&data)?;
        }
        Ok(new_friends)
    }

    pub fn contact_identity(&self, friend: &User) -> Result<ContactIdentity, String> {
        self.data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())
            .map(|data| {
                let identity = data
                    .contact_identities
                    .get(&friend.username.trim().to_ascii_lowercase())
                    .or_else(|| {
                        data.contact_identities.values().find(|identity| {
                            identity.observed_public_key == friend.id
                                || identity.trusted_public_key.as_deref() == Some(&friend.id)
                        })
                    });
                identity.cloned().unwrap_or(ContactIdentity {
                    observed_public_key: friend.id.to_ascii_lowercase(),
                    trusted_public_key: None,
                    status: IdentityTrustStatus::Unverified,
                    previous_public_keys: Vec::new(),
                })
            })
    }

    pub fn verify_contact_identity(
        &self,
        friend: &User,
        presented_identity: &str,
    ) -> Result<(), String> {
        if !is_identity_public_key(&friend.id)
            || !crate::services::identity_verification::matches_presented_identity(
                presented_identity,
                &friend.id,
            )
        {
            return Err("The compared fingerprint or QR code does not match this contact.".into());
        }
        let username = friend.username.trim().to_ascii_lowercase();
        if username.is_empty() {
            return Err("The contact has no valid account name.".into());
        }
        let public_key = friend.id.to_ascii_lowercase();
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let identity = data
            .contact_identities
            .entry(username)
            .or_insert_with(|| ContactIdentity {
                observed_public_key: public_key.clone(),
                trusted_public_key: None,
                status: IdentityTrustStatus::Unverified,
                previous_public_keys: Vec::new(),
            });
        if identity.observed_public_key != public_key {
            return Err("The contact identity changed during verification. Refresh the friend list and compare again.".into());
        }
        if let Some(previous_key) = identity.trusted_public_key.replace(public_key.clone()) {
            if previous_key != public_key && !identity.previous_public_keys.contains(&previous_key)
            {
                identity.previous_public_keys.push(previous_key);
            }
        }
        identity.observed_public_key = public_key;
        identity.status = IdentityTrustStatus::Verified;
        self.persist(&data)
    }

    pub fn verify_contact_identity_after_live_acceptance(
        &self,
        friend: &User,
    ) -> Result<(), String> {
        if !is_identity_public_key(&friend.id) {
            return Err("The contact has an invalid public identity.".into());
        }
        let username = friend.username.trim().to_ascii_lowercase();
        if username.is_empty() {
            return Err("The contact has no valid account name.".into());
        }
        let public_key = friend.id.to_ascii_lowercase();
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let identity = data
            .contact_identities
            .entry(username)
            .or_insert_with(|| ContactIdentity {
                observed_public_key: public_key.clone(),
                trusted_public_key: None,
                status: IdentityTrustStatus::Unverified,
                previous_public_keys: Vec::new(),
            });
        if identity.observed_public_key != public_key {
            return Err("The contact identity changed during verification. Refresh the friend list and try again.".into());
        }
        if let Some(previous_key) = identity.trusted_public_key.replace(public_key.clone()) {
            if previous_key != public_key && !identity.previous_public_keys.contains(&previous_key)
            {
                identity.previous_public_keys.push(previous_key);
            }
        }
        identity.status = IdentityTrustStatus::Verified;
        self.persist(&data)
    }

    pub fn revoke_contact_verification(&self, friend: &User) -> Result<(), String> {
        let username = friend.username.trim().to_ascii_lowercase();
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let identity = data
            .contact_identities
            .get_mut(&username)
            .ok_or_else(|| "This contact has no verified identity to revoke.".to_string())?;
        if identity.status != IdentityTrustStatus::Verified {
            return Err("This contact is not currently verified.".into());
        }
        identity.status = IdentityTrustStatus::Revoked;
        self.persist(&data)
    }

    pub fn ensure_identity_not_changed(&self, friend: &User) -> Result<(), String> {
        let identity = self.contact_identity(friend)?;
        if identity.observed_public_key != friend.id.to_ascii_lowercase() {
            return Err(format!(
                "The key for @{} does not match the locally observed identity. Refresh and verify before communicating.",
                friend.username
            ));
        }
        match identity.status {
            IdentityTrustStatus::Changed => Err(format!(
                "The identity key for @{} changed. Compare and explicitly verify the new fingerprint before communicating.",
                friend.username
            )),
            IdentityTrustStatus::Revoked => Err(format!(
                "Identity verification for @{} was revoked. Verify the current fingerprint before communicating.",
                friend.username
            )),
            IdentityTrustStatus::Unverified | IdentityTrustStatus::Verified => Ok(()),
        }
    }

    pub fn ensure_identity_verified(&self, friend: &User) -> Result<(), String> {
        let identity = self.contact_identity(friend)?;
        let public_key = friend.id.to_ascii_lowercase();
        if identity.observed_public_key != public_key
            || identity.status != IdentityTrustStatus::Verified
            || identity.trusted_public_key.as_deref() != Some(public_key.as_str())
        {
            return Err(format!(
                "The identity for @{} must be independently verified before accepting its MLS credential change.",
                friend.username
            ));
        }
        Ok(())
    }

    pub fn conversation_for(&self, peer: &User) -> Result<Conversation, String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        if let Some(conversation) = data
            .conversations
            .iter()
            .find(|conversation| conversation.user_id == peer.id)
        {
            let mut conversation = conversation.clone();
            if conversation.is_closed {
                let stored = data
                    .conversations
                    .iter_mut()
                    .find(|stored| stored.id == conversation.id)
                    .ok_or_else(|| "The local conversation could not be found.".to_string())?;
                stored.is_closed = false;
                conversation.is_closed = false;
                self.persist(&data)?;
            }
            return Ok(conversation);
        }
        let now = timestamp()?;
        let conversation = Conversation::new(&new_id()?, &peer.id, &peer.name, "", &now);
        data.conversations.push(conversation.clone());
        self.persist(&data)?;
        Ok(conversation)
    }

    pub fn save_attachment_ciphertext(&self, id: &str, ciphertext: &[u8]) -> Result<(), String> {
        if !crate::services::attachment::valid_id(id)
            || ciphertext.len() < 16
            || ciphertext.len() > MAX_ATTACHMENT_CIPHERTEXT_BYTES
        {
            return Err("The encrypted attachment data is invalid.".into());
        }
        let directory = self.attachments_directory()?;
        fs::create_dir_all(&directory)
            .map_err(|error| format!("Could not create the local attachment directory: {error}"))?;
        let temporary_path = directory.join(format!("{id}.tmp"));
        let final_path = directory.join(id);
        let mut file = fs::File::create(&temporary_path)
            .map_err(|error| format!("Could not save the encrypted attachment: {error}"))?;
        file.write_all(ciphertext)
            .map_err(|error| format!("Could not save the encrypted attachment: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("Could not persist the encrypted attachment: {error}"))?;
        fs::rename(&temporary_path, &final_path)
            .map_err(|error| format!("Could not finalize the encrypted attachment: {error}"))
    }

    pub fn attachment_download_offset(&self, id: &str) -> Result<u64, String> {
        if !crate::services::attachment::valid_id(id) {
            return Err("The attachment identifier is invalid.".into());
        }
        let path = self.attachments_directory()?.join(format!("{id}.part"));
        match fs::metadata(path) {
            Ok(metadata) if metadata.len() <= MAX_ATTACHMENT_CIPHERTEXT_BYTES as u64 => {
                Ok(metadata.len())
            }
            Ok(_) => Err("The partial attachment exceeds the permitted size.".into()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(0),
            Err(error) => Err(format!("Could not inspect the partial attachment: {error}")),
        }
    }

    pub fn read_attachment_ciphertext(&self, id: &str) -> Result<Vec<u8>, String> {
        if !crate::services::attachment::valid_id(id) {
            return Err("The attachment identifier is invalid.".into());
        }
        let path = self.attachments_directory()?.join(id);
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("Could not inspect the encrypted attachment: {error}"))?;
        if metadata.len() < 16 || metadata.len() > MAX_ATTACHMENT_CIPHERTEXT_BYTES as u64 {
            return Err("The encrypted attachment has an invalid size.".into());
        }
        fs::read(path).map_err(|error| format!("Could not read the encrypted attachment: {error}"))
    }

    pub fn has_attachment_ciphertext(&self, id: &str) -> Result<bool, String> {
        if !crate::services::attachment::valid_id(id) {
            return Ok(false);
        }
        match fs::metadata(self.attachments_directory()?.join(id)) {
            Ok(metadata) => Ok(
                metadata.len() >= 16 && metadata.len() <= MAX_ATTACHMENT_CIPHERTEXT_BYTES as u64
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(format!(
                "Could not inspect the encrypted attachment: {error}"
            )),
        }
    }

    pub fn attachment_ciphertext_chunk(
        &self,
        id: &str,
        offset: u64,
        requested_size: usize,
    ) -> Result<(u64, Vec<u8>), String> {
        if !crate::services::attachment::valid_id(id)
            || requested_size == 0
            || requested_size > MAX_ATTACHMENT_CHUNK_BYTES
        {
            return Err("The encrypted attachment chunk request is invalid.".into());
        }
        let path = self.attachments_directory()?.join(id);
        let mut file = fs::File::open(&path)
            .map_err(|_| "The encrypted attachment is not available on this peer.".to_string())?;
        let total_size = file
            .metadata()
            .map_err(|error| format!("Could not inspect the encrypted attachment: {error}"))?
            .len();
        if total_size < 16
            || total_size > MAX_ATTACHMENT_CIPHERTEXT_BYTES as u64
            || offset > total_size
        {
            return Err("The encrypted attachment has an invalid size or offset.".into());
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| format!("Could not seek the encrypted attachment: {error}"))?;
        let remaining = (total_size - offset).min(requested_size as u64) as usize;
        let mut chunk = vec![0u8; remaining];
        file.read_exact(&mut chunk)
            .map_err(|error| format!("Could not read the encrypted attachment: {error}"))?;
        Ok((total_size, chunk))
    }

    pub fn record_attachment_chunk_served(
        &self,
        id: &str,
        peer_id: &str,
        offset: u64,
        total_size: u64,
        chunk_size: usize,
    ) -> Result<(), String> {
        if !crate::services::attachment::valid_id(id)
            || peer_id.len() != 64
            || !peer_id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || total_size < 16
            || total_size > MAX_ATTACHMENT_CIPHERTEXT_BYTES as u64
            || chunk_size == 0
            || chunk_size > MAX_ATTACHMENT_CHUNK_BYTES
            || offset.saturating_add(chunk_size as u64) > total_size
        {
            return Err("The served attachment chunk is invalid.".into());
        }
        let mut progress = self
            .attachment_serving_progress
            .lock()
            .map_err(|_| "Attachment transfer progress is unavailable.".to_string())?;
        let key = (id.to_string(), peer_id.to_string());
        if !progress.contains_key(&key) && progress.len() >= 512 {
            progress.clear();
        }
        let entry = progress
            .entry(key)
            .or_insert_with(|| AttachmentServingProgress {
                ranges: Vec::new(),
                total_size,
                started: Instant::now(),
            });
        if entry.total_size != total_size {
            return Err("The peer requested inconsistent attachment sizes.".into());
        }
        let mut ranges = std::mem::take(&mut entry.ranges);
        ranges.push((offset, offset.saturating_add(chunk_size as u64)));
        ranges.sort_unstable_by_key(|range| range.0);
        for (start, end) in ranges {
            if let Some((_, previous_end)) = entry.ranges.last_mut()
                && start <= *previous_end
            {
                *previous_end = (*previous_end).max(end);
            } else {
                entry.ranges.push((start, end));
            }
        }
        Ok(())
    }

    pub fn attachment_serving_progress(&self, id: &str) -> Result<Option<(u64, u64, u64)>, String> {
        let progress = self
            .attachment_serving_progress
            .lock()
            .map_err(|_| "Attachment transfer progress is unavailable.".to_string())?;
        let mut best = None;
        for ((attachment_id, _), entry) in progress.iter() {
            if attachment_id != id {
                continue;
            }
            let transferred = entry
                .ranges
                .iter()
                .map(|(start, end)| end.saturating_sub(*start))
                .sum::<u64>();
            let elapsed = entry.started.elapsed().as_secs_f64().max(0.001);
            let speed = (transferred as f64 / elapsed) as u64;
            let candidate = (transferred, entry.total_size, speed);
            if best.is_none_or(|current: (u64, u64, u64)| candidate.0 > current.0) {
                best = Some(candidate);
            }
        }
        Ok(best)
    }

    pub fn append_attachment_download_chunk(
        &self,
        id: &str,
        offset: u64,
        total_size: u64,
        chunk: &[u8],
    ) -> Result<(), String> {
        if !crate::services::attachment::valid_id(id)
            || total_size < 16
            || total_size > MAX_ATTACHMENT_CIPHERTEXT_BYTES as u64
            || chunk.is_empty()
            || chunk.len() > MAX_ATTACHMENT_CHUNK_BYTES
            || offset.saturating_add(chunk.len() as u64) > total_size
        {
            return Err("The received encrypted attachment chunk is invalid.".into());
        }
        let directory = self.attachments_directory()?;
        fs::create_dir_all(&directory)
            .map_err(|error| format!("Could not create the local attachment directory: {error}"))?;
        let path = directory.join(format!("{id}.part"));
        let mut file = fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| format!("Could not persist the attachment transfer: {error}"))?;
        let current_size = file
            .metadata()
            .map_err(|error| format!("Could not inspect the attachment transfer: {error}"))?
            .len();
        if offset > current_size {
            return Err("The attachment transfer cannot resume at the requested offset.".into());
        }
        if offset < current_size {
            let mut existing = vec![0u8; chunk.len()];
            file.seek(SeekFrom::Start(offset))
                .and_then(|_| file.read_exact(&mut existing))
                .map_err(|error| format!("Could not verify the attachment transfer: {error}"))?;
            if existing != chunk {
                return Err("The resumed attachment chunk conflicts with local data.".into());
            }
            return Ok(());
        }
        file.seek(SeekFrom::End(0))
            .and_then(|_| file.write_all(chunk))
            .and_then(|_| file.sync_all())
            .map_err(|error| format!("Could not persist the attachment transfer: {error}"))
    }

    pub fn finish_attachment_download(
        &self,
        metadata: &crate::services::attachment::AttachmentMetadata,
    ) -> Result<(), String> {
        crate::services::attachment::validate_metadata(metadata)?;
        let expected_size = metadata.size as u64 + 16;
        let id = &metadata.attachment_id;
        let directory = self.attachments_directory()?;
        let partial_path = directory.join(format!("{id}.part"));
        let final_path = directory.join(id);
        let actual_size = fs::metadata(&partial_path)
            .map_err(|error| format!("Could not inspect the completed attachment: {error}"))?
            .len();
        if actual_size != expected_size {
            return Err("The attachment transfer is incomplete.".into());
        }
        let ciphertext = fs::read(&partial_path)
            .map_err(|error| format!("Could not read the completed attachment: {error}"))?;
        if let Err(error) = crate::services::attachment::decrypt(metadata, &ciphertext) {
            fs::remove_file(&partial_path).map_err(|cleanup_error| {
                format!(
                    "{error} Could not remove the unauthenticated partial file: {cleanup_error}"
                )
            })?;
            return Err(error);
        }
        if final_path.exists() {
            let existing = self.read_attachment_ciphertext(id)?;
            crate::services::attachment::decrypt(metadata, &existing)?;
            fs::remove_file(&partial_path)
                .map_err(|error| format!("Could not clean up the duplicate transfer: {error}"))?;
            return Ok(());
        }
        fs::rename(&partial_path, &final_path)
            .map_err(|error| format!("Could not finalize the attachment transfer: {error}"))
    }

    pub fn can_serve_attachment(
        &self,
        id: &str,
        peer_id: &str,
        conversation_id: &str,
    ) -> Result<bool, String> {
        if !crate::services::attachment::valid_id(id)
            || !crate::services::attachment::valid_id(conversation_id)
        {
            return Ok(false);
        }
        let account_id = self.account_id.as_deref();
        let data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        Ok(data.messages.iter().any(|message| {
            message.is_mine
                && !message.deleted
                && message.attachments().any(|attachment| {
                    attachment.attachment_id == id
                        && attachment.binding.as_ref().is_none_or(|binding| {
                            binding.conversation_id == conversation_id
                                && Some(binding.sender_id.as_str()) == account_id
                                && binding
                                    .participants
                                    .iter()
                                    .any(|participant| participant == peer_id)
                        })
                })
                && data
                    .conversations
                    .iter()
                    .find(|conversation| {
                        if conversation.id != message.conversation_id {
                            false
                        } else if conversation.is_group() {
                            conversation.id == conversation_id
                        } else {
                            account_id.is_some_and(|account_id| {
                                crate::services::mls::MlsService::direct_group_id(
                                    account_id,
                                    &conversation.user_id,
                                )
                                .is_ok_and(|id| id == conversation_id)
                            })
                        }
                    })
                    .is_some_and(|conversation| {
                        if conversation.is_group() {
                            conversation
                                .members
                                .iter()
                                .any(|member| member.id == peer_id)
                        } else {
                            conversation.user_id == peer_id
                        }
                    })
        }))
    }

    pub fn record_attachment_transfer_receipt(
        &self,
        id: &str,
        peer_id: &str,
        conversation_id: &str,
    ) -> Result<(), String> {
        if !self.can_serve_attachment(id, peer_id, conversation_id)? {
            return Err("The peer cannot confirm this attachment transfer.".into());
        }
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let receipts = data
            .attachment_transfer_receipts
            .entry(id.to_string())
            .or_default();
        if !receipts.iter().any(|receipt| receipt == peer_id) {
            receipts.push(peer_id.to_string());
            self.persist(&data)?;
        }
        Ok(())
    }

    pub fn attachment_transfer_receipt_status(
        &self,
        id: &str,
    ) -> Result<Option<(usize, usize)>, String> {
        let data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let Some((message, metadata)) = data.messages.iter().find_map(|message| {
            message
                .is_mine
                .then(|| {
                    message
                        .attachments()
                        .find(|attachment| attachment.attachment_id == id)
                        .map(|metadata| (message, metadata))
                })
                .flatten()
        }) else {
            return Ok(None);
        };
        let Some(conversation) = data
            .conversations
            .iter()
            .find(|conversation| conversation.id == message.conversation_id)
        else {
            return Ok(None);
        };
        let expected_recipients = if let Some(binding) = metadata.binding.as_ref() {
            if binding.sender_id != message.author_id {
                return Ok(None);
            }
            binding
                .participants
                .iter()
                .filter(|participant| participant.as_str() != binding.sender_id)
                .filter(|participant| {
                    if conversation.is_group() {
                        conversation
                            .members
                            .iter()
                            .any(|member| member.id == participant.as_str())
                    } else {
                        conversation.user_id == participant.as_str()
                    }
                })
                .cloned()
                .collect::<Vec<_>>()
        } else {
            if conversation.is_group() {
                conversation
                    .members
                    .iter()
                    .filter(|member| member.id != message.author_id)
                    .map(|member| member.id.clone())
                    .collect()
            } else if conversation.user_id != message.author_id {
                vec![conversation.user_id.clone()]
            } else {
                Vec::new()
            }
        };
        if expected_recipients.is_empty() {
            return Ok(None);
        }
        let receipts = data
            .attachment_transfer_receipts
            .get(id)
            .cloned()
            .unwrap_or_default();
        let confirmed = expected_recipients
            .iter()
            .filter(|recipient| receipts.iter().any(|receipt| receipt == *recipient))
            .count();
        Ok(Some((confirmed, expected_recipients.len())))
    }

    fn attachments_directory(&self) -> Result<PathBuf, String> {
        let account_id = self
            .account_id
            .as_deref()
            .ok_or_else(|| "Local attachment storage is unavailable.".to_string())?;
        let path = self
            .path
            .as_ref()
            .ok_or_else(|| "Local attachment storage is unavailable.".to_string())?;
        let file_name = format!("{account_id}.attachments");
        Ok(path.with_file_name(file_name))
    }

    fn remove_attachment_files(&self, attachment_ids: &[String]) -> Result<(), String> {
        if self.account_id.is_none() || attachment_ids.is_empty() {
            return Ok(());
        }
        let directory = self.attachments_directory()?;
        if !directory.exists() {
            return Ok(());
        }
        for id in attachment_ids {
            if !crate::services::attachment::valid_id(id) {
                return Err("The attachment identifier is invalid during cleanup.".into());
            }
            for path in [directory.join(id), directory.join(format!("{id}.part"))] {
                match fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(format!("Could not clean up attachment data: {error}"));
                    }
                }
            }
        }
        let mut progress = self
            .attachment_serving_progress
            .lock()
            .map_err(|_| "Attachment transfer progress is unavailable.".to_string())?;
        progress.retain(|(attachment_id, _), _| {
            !attachment_ids
                .iter()
                .any(|removed_id| removed_id == attachment_id)
        });
        Ok(())
    }

    pub fn create_group(
        &self,
        id: &str,
        name: &str,
        members: &[User],
    ) -> Result<Conversation, String> {
        self.create_group_at_version(id, name, members, 1)
    }

    pub fn create_group_at_version(
        &self,
        id: &str,
        name: &str,
        members: &[User],
        membership_version: u64,
    ) -> Result<Conversation, String> {
        if id.len() != 32
            || !id.bytes().all(|byte| byte.is_ascii_hexdigit())
            || membership_version == 0
        {
            return Err("The group identifier is invalid.".into());
        }
        if name.trim().is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control)
        {
            return Err("Group names must contain 1-64 visible characters.".into());
        }
        if !(3..=12).contains(&members.len()) {
            return Err("A group must have 3-12 members, including you.".into());
        }
        let mut member_ids = std::collections::HashSet::new();
        if members.iter().any(|member| {
            member.id.is_empty()
                || member.name.trim().is_empty()
                || !member_ids.insert(member.id.as_str())
        }) {
            return Err("The group members are invalid or duplicated.".into());
        }

        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        if let Some(existing) = data
            .conversations
            .iter()
            .find(|conversation| conversation.id == id)
            .cloned()
        {
            let existing_ids: std::collections::HashSet<_> = existing
                .members
                .iter()
                .map(|member| member.id.as_str())
                .collect();
            if !existing.is_group() || existing.name != name || existing_ids != member_ids {
                return Err("The group identifier conflicts with another conversation.".into());
            }
            if existing.is_closed {
                let stored = data
                    .conversations
                    .iter_mut()
                    .find(|conversation| conversation.id == id)
                    .ok_or_else(|| {
                        "The local group conversation could not be found.".to_string()
                    })?;
                stored.is_closed = false;
                let reopened = stored.clone();
                self.persist(&data)?;
                return Ok(reopened);
            }
            return Ok(existing);
        }

        let now = timestamp()?;
        let mut conversation = Conversation::new(id, "", name, "", &now);
        conversation.members = members.to_vec();
        conversation.membership_version = membership_version;
        data.conversations.push(conversation.clone());
        self.persist(&data)?;
        Ok(conversation)
    }

    pub fn update_group_from_welcome(
        &self,
        group_id: &str,
        membership_version: u64,
        members: Vec<User>,
    ) -> Result<bool, String> {
        if membership_version == 0 || !valid_group_members(&members) {
            return Err("The MLS Welcome roster is invalid.".into());
        }
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let conversation = data
            .conversations
            .iter_mut()
            .find(|conversation| conversation.id == group_id && conversation.is_group())
            .ok_or_else(|| "The local group conversation is unavailable.".to_string())?;
        if conversation
            .members
            .first()
            .map(|member| member.id.as_str())
            != members.first().map(|member| member.id.as_str())
        {
            return Err("The MLS Welcome has a different group owner.".into());
        }
        if membership_version < conversation.membership_version {
            return Ok(false);
        }
        if membership_version == conversation.membership_version {
            if !same_member_ids(
                &conversation.members,
                &members.iter().map(|m| m.id.clone()).collect::<Vec<_>>(),
            ) {
                return Err("The MLS Welcome conflicts with the current group version.".into());
            }
            return Ok(false);
        }
        conversation.members = members;
        conversation.membership_version = membership_version;
        self.persist(&data)?;
        Ok(true)
    }

    pub fn add_group_message(
        &self,
        message: Message,
        group_id: &str,
        group_name: &str,
        members: &[User],
    ) -> Result<(), String> {
        self.add_group_message_with_mls_snapshot(message, group_id, group_name, members, None)
    }

    pub fn add_group_message_with_mls_snapshot(
        &self,
        mut message: Message,
        group_id: &str,
        group_name: &str,
        members: &[User],
        mls_snapshot: Option<Vec<u8>>,
    ) -> Result<(), String> {
        let conversation = self.create_group(group_id, group_name, members)?;
        message.conversation_id = conversation.id.clone();
        if message.is_mine && message.group_deliveries.is_empty() {
            message.group_deliveries = members
                .iter()
                .filter(|member| member.id != message.author_id)
                .map(|member| GroupRecipientDelivery {
                    recipient_id: member.id.clone(),
                    state: RecipientDeliveryState::Pending,
                    api_accepted: false,
                    api_failure_permanent: false,
                    attempts: 0,
                    next_attempt_at: None,
                    delivered_at: None,
                    read_at: None,
                    last_error: None,
                    device_available: false,
                    active_devices: 0,
                    pending_devices: 0,
                    delivered_devices: 0,
                    failed_devices: 0,
                    rejected_devices: 0,
                    expired_devices: 0,
                })
                .collect();
        }
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        if data
            .messages
            .iter()
            .any(|existing| existing.id == message.id)
        {
            return Ok(());
        }
        let conversation = data
            .conversations
            .iter_mut()
            .find(|existing| existing.id == group_id)
            .ok_or_else(|| "The local group conversation could not be found.".to_string())?;
        conversation.preview = message.preview();
        conversation.updated_at = message.sent_at.clone();
        data.messages.push(message);
        if let Some(snapshot) = mls_snapshot {
            data.mls_storage = snapshot;
        }
        sort_conversations_newest_first(&mut data.conversations);
        self.persist(&data)
    }

    pub fn add_message(&self, message: Message, peer: &User) -> Result<(), String> {
        self.add_message_with_mls_snapshot(message, peer, None)
    }

    pub fn add_message_with_mls_snapshot(
        &self,
        message: Message,
        peer: &User,
        mls_snapshot: Option<Vec<u8>>,
    ) -> Result<(), String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        if data
            .messages
            .iter()
            .any(|existing| existing.id == message.id)
        {
            return Ok(());
        }
        let conversation = if let Some(conversation) = data
            .conversations
            .iter_mut()
            .find(|conversation| conversation.user_id == peer.id)
        {
            conversation
        } else {
            let now = timestamp()?;
            data.conversations.push(Conversation::new(
                &new_id()?,
                &peer.id,
                &peer.name,
                "",
                &now,
            ));
            data.conversations
                .last_mut()
                .ok_or_else(|| "Could not create a local conversation.".to_string())?
        };
        let conversation_id = conversation.id.clone();
        conversation.preview = message.preview();
        conversation.updated_at = message.sent_at.clone();
        let mut message = message;
        message.conversation_id = conversation_id;
        data.messages.push(message);
        if let Some(snapshot) = mls_snapshot {
            data.mls_storage = snapshot;
        }
        sort_conversations_newest_first(&mut data.conversations);
        self.persist(&data)
    }

    pub fn set_delivery_state(&self, message_id: &str, state: &str) -> Result<(), String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let message = data
            .messages
            .iter_mut()
            .find(|message| message.id == message_id)
            .ok_or_else(|| "The local message could not be found.".to_string())?;
        message.delivery_state = state.to_string();
        if state == "relayed" {
            message.last_queued_at = Some(timestamp()?);
        }
        self.persist(&data)
    }

    pub fn update_group_delivery(
        &self,
        message_id: &str,
        delivery: GroupRecipientDelivery,
    ) -> Result<(), String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let message = data
            .messages
            .iter_mut()
            .find(|message| message.id == message_id)
            .ok_or_else(|| "The local group message could not be found.".to_string())?;
        if !message.is_mine || message.group_deliveries.is_empty() {
            return Err("The local message has no group delivery recipients.".into());
        }
        let existing_index = message
            .group_deliveries
            .iter()
            .position(|existing| existing.recipient_id == delivery.recipient_id);
        let mut changed = if let Some(index) = existing_index {
            let existing = &mut message.group_deliveries[index];
            let mut updated = delivery;
            if existing.state == RecipientDeliveryState::Delivered
                && updated.state != RecipientDeliveryState::Delivered
            {
                updated.state = RecipientDeliveryState::Delivered;
                updated.delivered_at = existing.delivered_at.clone().or(updated.delivered_at);
                updated.read_at = existing.read_at.clone().or(updated.read_at);
            }
            if *existing == updated {
                false
            } else {
                *existing = updated;
                true
            }
        } else {
            message.group_deliveries.push(delivery);
            true
        };
        let previous_message_state = message.delivery_state.clone();
        let previous_delivered_at = message.delivered_at.clone();
        let previous_read_at = message.read_at.clone();
        refresh_group_message_state(message);
        changed |= message.delivery_state != previous_message_state
            || message.delivered_at != previous_delivered_at
            || message.read_at != previous_read_at;
        if changed {
            self.persist(&data)
        } else {
            Ok(())
        }
    }

    pub fn retry_group_delivery(&self, message_id: &str) -> Result<(), String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let message = data
            .messages
            .iter_mut()
            .find(|message| message.id == message_id)
            .ok_or_else(|| "The local group message could not be found.".to_string())?;
        let mut retryable = false;
        for recipient in &mut message.group_deliveries {
            if recipient.state == RecipientDeliveryState::Failed
                || (recipient.state == RecipientDeliveryState::Delivered
                    && !recipient.api_accepted
                    && !recipient.api_failure_permanent
                    && recipient.attempts >= 5)
            {
                if recipient.state == RecipientDeliveryState::Failed {
                    recipient.state = RecipientDeliveryState::Pending;
                }
                recipient.api_accepted = false;
                recipient.api_failure_permanent = false;
                recipient.attempts = 0;
                recipient.next_attempt_at = None;
                recipient.last_error = None;
                retryable = true;
            }
        }
        if !retryable {
            return Err("The group message has no retryable recipients.".into());
        }
        refresh_group_message_state(message);
        message.last_queued_at = None;
        self.persist(&data)
    }

    pub fn update_receipt(
        &self,
        message_id: &str,
        delivered_at: Option<String>,
        read_at: Option<String>,
    ) -> Result<(), String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let message = data
            .messages
            .iter_mut()
            .find(|message| message.id == message_id)
            .ok_or_else(|| "The local message could not be found.".to_string())?;
        let mut changed = false;
        if let Some(delivered_at) = delivered_at
            && message.delivered_at.is_none()
        {
            message.delivered_at = Some(delivered_at);
            message.delivery_state = "delivered".into();
            changed = true;
        }
        if let Some(read_at) = read_at
            && message.read_at.is_none()
        {
            message.read_at = Some(read_at);
            changed = true;
        }
        if changed {
            self.persist(&data)
        } else {
            Ok(())
        }
    }

    pub fn mark_delivered(&self, message_id: &str) -> Result<String, String> {
        let messages = self.messages()?;
        if let Some(delivered_at) = messages
            .iter()
            .find(|message| message.id == message_id)
            .and_then(|message| message.delivered_at.clone())
        {
            return Ok(delivered_at);
        }
        let delivered_at = timestamp()?;
        self.update_receipt(message_id, Some(delivered_at.clone()), None)?;
        Ok(delivered_at)
    }

    pub fn mark_conversation_read(&self, conversation_id: &str) -> Result<Vec<String>, String> {
        let read_at = timestamp()?;
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let mut message_ids = Vec::new();
        let mut updated = false;
        for message in &mut data.messages {
            if message.conversation_id == conversation_id
                && !message.is_mine
                && !message.deleted
                && message.read_at.is_none()
            {
                message.read_at = Some(read_at.clone());
                updated = true;
                if message.delivery_state != "received_direct" {
                    message_ids.push(message.id.clone());
                }
            }
        }
        if updated {
            self.persist(&data)?;
        }
        Ok(message_ids)
    }

    pub fn close_conversation(&self, conversation_id: &str) -> Result<(), String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let conversation = data
            .conversations
            .iter_mut()
            .find(|conversation| conversation.id == conversation_id)
            .ok_or_else(|| "The local conversation could not be found.".to_string())?;
        conversation.is_closed = true;
        self.persist(&data)
    }

    pub fn delete_message(&self, message_id: &str) -> Result<(), String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let index = data
            .messages
            .iter()
            .position(|message| message.id == message_id)
            .ok_or_else(|| "The local message could not be found.".to_string())?;
        if !data.messages[index].is_mine {
            return Err("Only your own messages can be deleted.".into());
        }
        let attachment_ids = data.messages[index]
            .attachments()
            .map(|attachment| attachment.attachment_id.clone())
            .collect::<Vec<_>>();
        let conversation_id = data.messages[index].conversation_id.clone();
        let message = &mut data.messages[index];
        message.body.clear();
        message.attachment = None;
        message.attachments.clear();
        message.deleted = true;
        if let Some(conversation) = data
            .conversations
            .iter_mut()
            .find(|conversation| conversation.id == conversation_id)
        {
            conversation.preview = "Message deleted".into();
        }
        let unreferenced_ids = attachment_ids
            .into_iter()
            .filter(|id| {
                !data
                    .messages
                    .iter()
                    .flat_map(Message::attachments)
                    .any(|attachment| attachment.attachment_id == *id)
            })
            .collect::<Vec<_>>();
        for id in &unreferenced_ids {
            data.attachment_transfer_receipts.remove(id);
        }
        self.persist(&data)?;
        drop(data);
        self.remove_attachment_files(&unreferenced_ids)
    }

    pub fn edit_message(&self, message_id: &str, body: &str) -> Result<(), String> {
        if body.trim().is_empty() {
            return Err("A message cannot be empty.".into());
        }
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let message = data
            .messages
            .iter_mut()
            .find(|message| message.id == message_id)
            .ok_or_else(|| "The local message could not be found.".to_string())?;
        if !message.is_mine || message.deleted {
            return Err("Only your own active messages can be edited.".into());
        }
        let conversation_id = message.conversation_id.clone();
        message.body = body.to_string();
        let latest_preview = data
            .messages
            .iter()
            .filter(|message| message.conversation_id == conversation_id)
            .max_by(|left, right| left.sent_at.cmp(&right.sent_at))
            .map(|latest_message| {
                if latest_message.deleted {
                    "Message deleted".into()
                } else {
                    latest_message.body.clone()
                }
            });
        if let Some(conversation) = data
            .conversations
            .iter_mut()
            .find(|conversation| conversation.id == conversation_id)
            && let Some(latest_preview) = latest_preview
        {
            conversation.preview = latest_preview;
        }
        self.persist(&data)
    }

    pub fn delete_conversation(&self, conversation_id: &str) -> Result<(), String> {
        let mut data = self
            .data
            .lock()
            .map_err(|_| "Local chat history is unavailable.".to_string())?;
        let removed_attachment_ids = data
            .messages
            .iter()
            .filter(|message| message.conversation_id == conversation_id)
            .flat_map(Message::attachments)
            .map(|attachment| attachment.attachment_id.clone())
            .collect::<Vec<_>>();
        data.conversations
            .retain(|conversation| conversation.id != conversation_id);
        data.messages
            .retain(|message| message.conversation_id != conversation_id);
        data.mls_welcomes.remove(conversation_id);
        let unreferenced_ids = removed_attachment_ids
            .into_iter()
            .filter(|id| {
                !data
                    .messages
                    .iter()
                    .flat_map(Message::attachments)
                    .any(|attachment| attachment.attachment_id == *id)
            })
            .collect::<Vec<_>>();
        for id in &unreferenced_ids {
            data.attachment_transfer_receipts.remove(id);
        }
        self.persist(&data)?;
        drop(data);
        self.remove_attachment_files(&unreferenced_ids)
    }

    fn persist(&self, data: &ChatData) -> Result<(), String> {
        let Some(path) = self.path.as_ref() else {
            return Ok(());
        };
        let plaintext = serde_json::to_vec(data)
            .map_err(|error| format!("Could not encode local chat history: {error}"))?;
        let account_id = self
            .account_id
            .as_deref()
            .ok_or_else(|| "The local chat account is unavailable.".to_string())?;
        let encryption_key = self
            .encryption_key
            .as_deref()
            .ok_or_else(|| "The local chat encryption key is unavailable.".to_string())?;
        let bytes = encrypt_history(&plaintext, account_id, encryption_key)?;
        let suffix = new_id()?;
        let temporary = path.with_extension(format!("{suffix}.tmp"));
        let backup = path.with_extension("bak");
        fs::write(&temporary, bytes)
            .map_err(|error| format!("Could not write local chat history: {error}"))?;
        if path.exists() {
            if backup.exists() {
                fs::remove_file(&backup)
                    .map_err(|error| format!("Could not replace the local chat backup: {error}"))?;
            }
            fs::rename(path, &backup)
                .map_err(|error| format!("Could not back up local chat history: {error}"))?;
        }
        if let Err(error) = fs::rename(&temporary, path) {
            if backup.exists() {
                match fs::rename(&backup, path) {
                    Ok(()) => {}
                    Err(restore_error) => {
                        let _ = fs::remove_file(&temporary);
                        return Err(format!(
                            "Could not save local chat history ({error}) or restore its backup ({restore_error})."
                        ));
                    }
                }
            }
            let _ = fs::remove_file(&temporary);
            return Err(format!("Could not save local chat history: {error}"));
        }
        if backup.exists() {
            fs::remove_file(&backup)
                .map_err(|error| format!("Could not remove the local chat backup: {error}"))?;
        }
        Ok(())
    }
}

fn derive_storage_key(identity_key: &[u8]) -> Result<Zeroizing<[u8; 32]>, String> {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, b"Resonance local chat storage v1");
    let pseudorandom_key = salt.extract(identity_key);
    let mut key = Zeroizing::new([0; 32]);
    pseudorandom_key
        .expand(&[b"account chat history"], &aead::AES_256_GCM)
        .map_err(|_| "Could not derive the local chat encryption key.".to_string())?
        .fill(&mut key[..])
        .map_err(|_| "Could not derive the local chat encryption key.".to_string())?;
    Ok(key)
}

fn encrypt_history(
    plaintext: &[u8],
    account_id: &str,
    encryption_key: &[u8; 32],
) -> Result<Vec<u8>, String> {
    let mut nonce_bytes = [0; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| "Could not create a local chat encryption nonce.".to_string())?;
    let key = LessSafeKey::new(
        UnboundKey::new(&aead::AES_256_GCM, encryption_key)
            .map_err(|_| "Could not initialize local chat encryption.".to_string())?,
    );
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);
    let mut ciphertext = plaintext.to_vec();
    key.seal_in_place_append_tag(nonce, Aad::from(account_id.as_bytes()), &mut ciphertext)
        .map_err(|_| "Could not encrypt local chat history.".to_string())?;
    let mut output = Vec::with_capacity(FILE_MAGIC.len() + NONCE_LEN + ciphertext.len());
    output.extend_from_slice(FILE_MAGIC);
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);
    Ok(output)
}

fn decrypt_history(
    encrypted: &[u8],
    account_id: &str,
    encryption_key: &[u8; 32],
) -> Result<Vec<u8>, String> {
    let payload_start = FILE_MAGIC.len() + NONCE_LEN;
    if encrypted.len() < payload_start + aead::AES_256_GCM.tag_len()
        || !encrypted.starts_with(FILE_MAGIC)
    {
        return Err("Local chat history is corrupted or uses an unsupported format.".into());
    }
    let nonce = Nonce::try_assume_unique_for_key(
        encrypted[FILE_MAGIC.len()..payload_start]
            .try_into()
            .map_err(|_| "Local chat history is corrupted.".to_string())?,
    )
    .map_err(|_| "Local chat history is corrupted.".to_string())?;
    let mut ciphertext = encrypted[payload_start..].to_vec();
    let key = LessSafeKey::new(
        UnboundKey::new(&aead::AES_256_GCM, encryption_key)
            .map_err(|_| "Could not initialize local chat decryption.".to_string())?,
    );
    let plaintext = key
        .open_in_place(nonce, Aad::from(account_id.as_bytes()), &mut ciphertext)
        .map_err(|_| "Local chat history authentication failed.".to_string())?;
    Ok(plaintext.to_vec())
}

fn valid_group_id(group_id: &str) -> bool {
    group_id.len() == 32 && group_id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_identity_public_key(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_group_members(members: &[User]) -> bool {
    if !(3..=12).contains(&members.len()) {
        return false;
    }
    let mut ids = std::collections::HashSet::new();
    members.iter().all(|member| {
        member.id.len() == 64
            && member.id.bytes().all(|byte| byte.is_ascii_hexdigit())
            && !member.name.trim().is_empty()
            && member.name.chars().count() <= 64
            && member.username.len() <= 32
            && ids.insert(member.id.as_str())
    })
}

fn same_member_ids(members: &[User], member_ids: &[String]) -> bool {
    let expected = members
        .iter()
        .map(|member| member.id.as_str())
        .collect::<std::collections::HashSet<_>>();
    let actual = member_ids
        .iter()
        .map(String::as_str)
        .collect::<std::collections::HashSet<_>>();
    expected.len() == members.len() && actual.len() == member_ids.len() && expected == actual
}

pub fn timestamp() -> Result<String, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs().to_string())
        .map_err(|_| "The device clock is before the Unix epoch.".into())
}

pub fn new_id() -> Result<String, String> {
    use ring::rand::{SecureRandom, SystemRandom};
    let mut bytes = [0; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "Could not create a local conversation identifier.".to_string())?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversation_history_is_saved_and_reloaded_from_the_device() {
        let directory =
            std::env::temp_dir().join(format!("resonance-chat-test-{}", new_id().unwrap()));
        let path = directory.join("local.json");
        let peer = User::new("peer-id", "peer", "Peer", "P", 0x123456, "Online");
        let identity_key = b"test identity private key";
        let store = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        let conversation = store.conversation_for(&peer).unwrap();
        store
            .add_message_with_mls_snapshot(
                Message::new(
                    "message-id",
                    &conversation.id,
                    "me",
                    "Me",
                    "hello",
                    "1",
                    true,
                ),
                &peer,
                Some(b"committed mls state".to_vec()),
            )
            .unwrap();
        drop(store);

        let on_disk = fs::read(&path).unwrap();
        assert!(on_disk.starts_with(FILE_MAGIC));
        assert!(!on_disk.windows(5).any(|window| window == b"hello"));
        let reloaded = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        assert_eq!(reloaded.messages().unwrap()[0].body, "hello");
        assert_eq!(reloaded.conversations().unwrap()[0].preview, "hello");
        assert_eq!(
            reloaded.mls_storage_snapshot().unwrap(),
            b"committed mls state"
        );
        drop(reloaded);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unchanged_group_delivery_does_not_rewrite_chat_history() {
        let directory = std::env::temp_dir()
            .join(format!("resonance-group-delivery-noop-{}", new_id().unwrap()));
        let path = directory.join("local.json");
        let identity_key = b"group delivery identity key";
        let owner = User::new(&"1".repeat(64), "owner", "Owner", "O", 0x123456, "Online");
        let first = User::new(&"2".repeat(64), "first", "First", "F", 0x123456, "Online");
        let second = User::new(&"3".repeat(64), "second", "Second", "S", 0x123456, "Online");
        let group_id = "0123456789abcdef0123456789abcdef";
        let store = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        let mut message = Message::new(
            "group-message",
            group_id,
            &owner.id,
            &owner.name,
            "hello",
            "1",
            true,
        );
        message.delivery_state = "relayed".into();
        store
            .add_group_message(
                message,
                group_id,
                "Group",
                &[owner, first, second],
            )
            .unwrap();
        let delivery = store.messages().unwrap()[0].group_deliveries[0].clone();
        store
            .update_group_delivery("group-message", delivery.clone())
            .unwrap();
        let before = fs::read(&path).unwrap();

        store
            .update_group_delivery("group-message", delivery)
            .unwrap();

        assert_eq!(fs::read(&path).unwrap(), before);
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn group_delivery_failures_remain_per_recipient_and_retry_after_restart() {
        let directory =
            std::env::temp_dir().join(format!("resonance-group-delivery-{}", new_id().unwrap()));
        let path = directory.join("local.json");
        let identity_key = b"group delivery identity key";
        let owner = User::new(&"1".repeat(64), "owner", "Owner", "O", 0x123456, "Online");
        let first = User::new(&"2".repeat(64), "first", "First", "F", 0x123456, "Online");
        let second = User::new(&"3".repeat(64), "second", "Second", "S", 0x123456, "Online");
        let group_id = "0123456789abcdef0123456789abcdef";
        let store = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        let mut message = Message::new(
            "group-message",
            group_id,
            &owner.id,
            &owner.name,
            "hello",
            "1",
            true,
        );
        message.delivery_state = "relayed".into();
        store
            .add_group_message(
                message,
                group_id,
                "Group",
                &[owner, first.clone(), second.clone()],
            )
            .unwrap();
        let mut deliveries = store.messages().unwrap()[0].group_deliveries.clone();
        deliveries[1].attempts = 2;
        deliveries[1].next_attempt_at = Some("2000000000".into());
        store
            .update_group_delivery("group-message", deliveries[1].clone())
            .unwrap();
        let mut first_delivery = deliveries[0].clone();
        first_delivery.state = RecipientDeliveryState::Failed;
        first_delivery.attempts = 5;
        first_delivery.last_error = Some("temporary network failure".into());
        store
            .update_group_delivery("group-message", first_delivery)
            .unwrap();

        let message = store.messages().unwrap().remove(0);
        assert_eq!(message.delivery_state, "relayed");
        assert_eq!(
            message.group_deliveries[0].state,
            RecipientDeliveryState::Failed
        );
        assert_eq!(
            message.group_deliveries[1].state,
            RecipientDeliveryState::Pending
        );
        assert_eq!(message.group_deliveries[1].attempts, 2);
        assert_eq!(
            message.group_deliveries[1].next_attempt_at.as_deref(),
            Some("2000000000")
        );
        drop(store);

        let reloaded = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        reloaded.retry_group_delivery("group-message").unwrap();
        let retried = reloaded.messages().unwrap().remove(0);
        assert_eq!(retried.delivery_state, "relayed");
        assert!(
            retried
                .group_deliveries
                .iter()
                .all(|delivery| delivery.state == RecipientDeliveryState::Pending)
        );
        assert_eq!(retried.group_deliveries[0].attempts, 0);
        assert_eq!(retried.group_deliveries[1].attempts, 2);
        assert_eq!(
            retried.group_deliveries[1].next_attempt_at.as_deref(),
            Some("2000000000")
        );

        drop(reloaded);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn pending_inbound_relays_survive_restart_and_are_removed_idempotently() {
        let directory =
            std::env::temp_dir().join(format!("resonance-inbound-test-{}", new_id().unwrap()));
        let path = directory.join("local.json");
        let identity_key = b"inbound test identity key";
        let store = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        let envelope = RelayEnvelope {
            message_id: "1".repeat(32),
            sender_id: "a".repeat(64),
            recipient_id: "b".repeat(64),
            group_id: None,
            sent_at: "123".into(),
            ephemeral_public_key: "c".repeat(64),
            nonce: "d".repeat(24),
            ciphertext: "e".repeat(64),
            signature: "f".repeat(128),
        };
        store
            .save_pending_inbound_relay(&envelope.sender_id, &envelope, None, None)
            .unwrap();
        store
            .save_pending_inbound_relay(&envelope.sender_id, &envelope, None, None)
            .unwrap();
        drop(store);

        let restored = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        assert_eq!(
            restored.pending_inbound_relays().unwrap(),
            vec![PendingInboundRelay {
                sender_public_key: envelope.sender_id.clone(),
                envelope: envelope.clone(),
                lease_token: None,
                queue_sequence: None,
            }]
        );
        restored
            .remove_pending_inbound_relay(&envelope.message_id)
            .unwrap();
        restored
            .remove_pending_inbound_relay(&envelope.message_id)
            .unwrap();
        assert!(restored.pending_inbound_relays().unwrap().is_empty());
        drop(restored);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn device_identity_is_stable_and_stored_only_in_encrypted_chat_data() {
        let directory =
            std::env::temp_dir().join(format!("resonance-device-id-{}", new_id().unwrap()));
        let path = directory.join("local.json");
        let identity_key = b"device identity encryption key";
        let store = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        let first = store.device_identity().unwrap();
        assert_eq!(first.id.len(), 32);
        assert_eq!(first.secret.len(), 64);
        assert_ne!(first.id, first.secret[..32]);
        drop(store);

        let on_disk = fs::read(&path).unwrap();
        assert!(
            !on_disk
                .windows(first.id.len())
                .any(|window| window == first.id.as_bytes())
        );
        assert!(
            !on_disk
                .windows(first.secret.len())
                .any(|window| window == first.secret.as_bytes())
        );
        let restored = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        assert!(restored.device_identity().unwrap() == first);
        drop(restored);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn enrollment_restart_rotates_only_the_persisted_request_nonce() {
        let directory =
            std::env::temp_dir().join(format!("resonance-enrollment-retry-{}", new_id().unwrap()));
        let path = directory.join("local.json");
        let identity_key = b"enrollment restart identity key";
        let store = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        let original = store.device_identity().unwrap();
        let restored = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        assert!(restored.device_identity().unwrap() == original);

        let restarted = restored.restart_device_enrollment().unwrap();
        assert_eq!(restarted.id, original.id);
        assert_eq!(restarted.secret, original.secret);
        assert_eq!(restarted.signing_private_key, original.signing_private_key);
        assert_eq!(
            restarted.encryption_private_key,
            original.encryption_private_key
        );
        assert_ne!(
            restarted.enrollment_request_id,
            original.enrollment_request_id
        );
        assert_ne!(restarted.enrollment_nonce, original.enrollment_nonce);
        drop(restored);
        drop(store);

        let after_restart = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        assert!(after_restart.device_identity().unwrap() == restarted);
        drop(after_restart);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn history_import_never_replaces_existing_mls_state() {
        let store = LocalChatStore::in_memory();
        store.save_mls_storage_snapshot(vec![1, 2, 3]).unwrap();
        let snapshot = HistorySnapshot {
            version: 1,
            account_public_key: "a".repeat(64),
            source_device_id: "1".repeat(32),
            target_device_id: "2".repeat(32),
            conversations: Vec::new(),
            messages: Vec::new(),
            mls_storage: vec![4, 5, 6],
            attachments: Vec::new(),
            unavailable_attachment_ids: Vec::new(),
        };
        assert!(
            store
                .import_history_snapshot(snapshot, &"a".repeat(64), &"2".repeat(32))
                .is_err()
        );
        assert_eq!(store.mls_storage_snapshot().unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn encrypted_attachment_chunks_resume_and_finalize_without_server_storage() {
        let directory =
            std::env::temp_dir().join(format!("resonance-attachment-test-{}", new_id().unwrap()));
        let source = LocalChatStore::open_at(directory.join("source.json"), b"source key").unwrap();
        let destination =
            LocalChatStore::open_at(directory.join("destination.json"), b"destination key")
                .unwrap();
        let attachment_id = "0123456789abcdef0123456789abcdef";
        let plaintext = vec![42; 80];
        let (key, nonce, ciphertext) =
            crate::services::attachment::encrypt(attachment_id, &plaintext).unwrap();
        let metadata = crate::services::attachment::AttachmentMetadata {
            attachment_id: attachment_id.into(),
            file_name: "test.bin".into(),
            content_type: "application/octet-stream".into(),
            size: plaintext.len(),
            kind: crate::services::attachment::AttachmentKind::File,
            key,
            nonce,
            binding: None,
        };
        source
            .save_attachment_ciphertext(attachment_id, &ciphertext)
            .unwrap();

        let (size, first_chunk) = source
            .attachment_ciphertext_chunk(attachment_id, 0, 32)
            .unwrap();
        destination
            .append_attachment_download_chunk(attachment_id, 0, size, &first_chunk)
            .unwrap();
        destination
            .append_attachment_download_chunk(attachment_id, 0, size, &first_chunk)
            .unwrap();
        let mut conflicting_chunk = first_chunk.clone();
        conflicting_chunk[0] ^= 0xff;
        assert!(
            destination
                .append_attachment_download_chunk(attachment_id, 0, size, &conflicting_chunk)
                .is_err()
        );
        assert_eq!(
            destination
                .attachment_download_offset(attachment_id)
                .unwrap(),
            32
        );
        let (resumed_size, second_chunk) = source
            .attachment_ciphertext_chunk(attachment_id, 32, 64)
            .unwrap();
        assert_eq!(size, resumed_size);
        destination
            .append_attachment_download_chunk(attachment_id, 32, size, &second_chunk)
            .unwrap();
        destination.finish_attachment_download(&metadata).unwrap();
        assert_eq!(
            destination
                .read_attachment_ciphertext(attachment_id)
                .unwrap(),
            ciphertext
        );

        drop(source);
        drop(destination);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn unauthenticated_complete_attachment_is_removed_not_finalized() {
        let directory =
            std::env::temp_dir().join(format!("resonance-attachment-tamper-{}", new_id().unwrap()));
        let store =
            LocalChatStore::open_at(directory.join("local.json"), b"tamper test identity key")
                .unwrap();
        let attachment_id = "0123456789abcdef0123456789abcdef";
        let plaintext = b"authenticated contents";
        let (key, nonce, mut ciphertext) =
            crate::services::attachment::encrypt(attachment_id, plaintext).unwrap();
        let metadata = crate::services::attachment::AttachmentMetadata {
            attachment_id: attachment_id.into(),
            file_name: "test.bin".into(),
            content_type: "application/octet-stream".into(),
            size: plaintext.len(),
            kind: crate::services::attachment::AttachmentKind::File,
            key,
            nonce,
            binding: None,
        };
        ciphertext[0] ^= 0x80;
        store
            .append_attachment_download_chunk(
                attachment_id,
                0,
                ciphertext.len() as u64,
                &ciphertext,
            )
            .unwrap();

        assert!(store.finish_attachment_download(&metadata).is_err());
        assert_eq!(store.attachment_download_offset(attachment_id).unwrap(), 0);
        assert!(!store.has_attachment_ciphertext(attachment_id).unwrap());

        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn deleting_an_attachment_message_removes_its_local_transfer_data() {
        let directory =
            std::env::temp_dir().join(format!("resonance-attachment-delete-{}", new_id().unwrap()));
        let store =
            LocalChatStore::open_at(directory.join("local.json"), b"delete test identity key")
                .unwrap();
        let owner = User::new(&"1".repeat(64), "owner", "Owner", "O", 1, "Online");
        let first = User::new(&"2".repeat(64), "first", "First", "F", 2, "Online");
        let second = User::new(&"3".repeat(64), "second", "Second", "S", 3, "Online");
        let group_id = "0123456789abcdef0123456789abcdef";
        let members = vec![owner.clone(), first, second];
        store.create_group(group_id, "Files", &members).unwrap();
        let attachment_id = "fedcba9876543210fedcba9876543210";
        let (key, nonce, ciphertext) =
            crate::services::attachment::encrypt(attachment_id, b"file").unwrap();
        let mut message = Message::new(
            "message-id",
            group_id,
            &owner.id,
            &owner.name,
            "",
            "1",
            true,
        );
        message.attachments = vec![crate::services::attachment::AttachmentMetadata {
            attachment_id: attachment_id.into(),
            file_name: "file.txt".into(),
            content_type: "text/plain".into(),
            size: 4,
            kind: crate::services::attachment::AttachmentKind::File,
            key,
            nonce,
            binding: None,
        }];
        store
            .add_group_message(message, group_id, "Files", &members)
            .unwrap();
        store
            .save_attachment_ciphertext(attachment_id, &ciphertext)
            .unwrap();

        store.delete_message("message-id").unwrap();
        assert!(!store.has_attachment_ciphertext(attachment_id).unwrap());
        let deleted = store.messages().unwrap().remove(0);
        assert!(deleted.deleted);
        assert_eq!(deleted.attachments().count(), 0);

        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn mls_welcome_retry_state_is_persisted_per_recipient() {
        let directory =
            std::env::temp_dir().join(format!("resonance-mls-welcome-{}", new_id().unwrap()));
        let path = directory.join("local.json");
        let welcome = "a1b2c3";
        let store = LocalChatStore::open_at(path.clone(), b"welcome state identity key").unwrap();
        let record = store
            .save_mls_welcome("0123456789abcdef0123456789abcdef", welcome)
            .unwrap();
        assert_eq!(
            store
                .save_mls_welcome("0123456789abcdef0123456789abcdef", welcome)
                .unwrap()
                .message_id,
            record.message_id
        );
        store
            .mark_mls_welcome_queued("0123456789abcdef0123456789abcdef", "recipient-public-key")
            .unwrap();
        drop(store);

        let restored =
            LocalChatStore::open_at(path.clone(), b"welcome state identity key").unwrap();
        let restored_record = restored
            .mls_welcome("0123456789abcdef0123456789abcdef")
            .unwrap()
            .unwrap();
        assert_eq!(restored_record.message_id, record.message_id);
        assert_eq!(restored_record.welcome, welcome);
        assert_eq!(
            restored_record.queued_recipients,
            vec!["recipient-public-key"]
        );
        drop(restored);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn pending_membership_change_survives_restart_and_duplicate_events_are_idempotent() {
        let directory =
            std::env::temp_dir().join(format!("resonance-group-change-{}", new_id().unwrap()));
        let path = directory.join("local.json");
        let identity_key = b"group membership persistence key";
        let owner = User::new(&"1".repeat(64), "owner", "Owner", "O", 1, "Online");
        let member = User::new(&"2".repeat(64), "member", "Member", "M", 2, "Online");
        let other = User::new(&"3".repeat(64), "other", "Other", "T", 3, "Online");
        let added = User::new(&"4".repeat(64), "added", "Added", "A", 4, "Online");
        let group_id = "0123456789abcdef0123456789abcdef";
        let store = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        store
            .create_group(
                group_id,
                "Project",
                &[owner.clone(), member.clone(), other.clone()],
            )
            .unwrap();
        let deliveries = [member.id.as_str(), other.id.as_str(), added.id.as_str()]
            .into_iter()
            .map(|recipient| RelayDelivery {
                recipient_public_key: recipient.to_string(),
                envelope: crate::services::relay::RelayEnvelope {
                    message_id: new_id().unwrap(),
                    sender_id: owner.id.clone(),
                    recipient_id: recipient.to_string(),
                    group_id: Some(group_id.to_string()),
                    sent_at: "1".into(),
                    ephemeral_public_key: String::new(),
                    nonce: String::new(),
                    ciphertext: String::new(),
                    signature: String::new(),
                },
            })
            .collect();
        let change = PendingGroupMembershipChange {
            group_id: group_id.to_string(),
            operation_id: new_id().unwrap(),
            expected_version: 1,
            members: vec![owner.clone(), member.clone(), other.clone(), added.clone()],
            deliveries,
        };
        let operation_id = change.operation_id.clone();
        store
            .save_pending_group_change(change, vec![1, 2, 3])
            .unwrap();
        drop(store);

        let restored = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        let pending = restored.pending_group_change(group_id).unwrap().unwrap();
        assert_eq!(pending.operation_id, operation_id);
        assert_eq!(pending.deliveries.len(), 3);
        assert_eq!(restored.mls_storage_snapshot().unwrap(), vec![1, 2, 3]);
        let event_store = LocalChatStore::in_memory();
        event_store
            .create_group(
                group_id,
                "Project",
                &[owner.clone(), member.clone(), other.clone()],
            )
            .unwrap();
        let actual_member_ids = vec![
            owner.id.clone(),
            member.id.clone(),
            other.id.clone(),
            added.id.clone(),
        ];
        assert!(
            event_store
                .apply_remote_group_change(
                    group_id,
                    3,
                    vec![owner.clone(), member.clone(), other.clone(), added.clone()],
                    &actual_member_ids,
                    &owner.id,
                    &[],
                    vec![4],
                )
                .is_err()
        );
        assert!(
            event_store
                .apply_remote_group_change(
                    group_id,
                    2,
                    vec![owner.clone(), member.clone(), other.clone(), added.clone()],
                    &actual_member_ids,
                    &owner.id,
                    std::slice::from_ref(&member.id),
                    vec![4],
                )
                .unwrap()
        );
        assert!(
            event_store
                .group_has_credential_change(group_id, &member.id)
                .unwrap()
        );
        assert!(
            event_store
                .apply_remote_group_change(
                    group_id,
                    3,
                    vec![owner.clone(), member.clone(), other.clone(), added.clone()],
                    &actual_member_ids,
                    &owner.id,
                    &["f".repeat(64)],
                    vec![5],
                )
                .is_err()
        );
        assert!(
            !event_store
                .apply_remote_group_change(
                    group_id,
                    2,
                    vec![owner, member, other, added],
                    &actual_member_ids,
                    &"1".repeat(64),
                    &[],
                    vec![5],
                )
                .unwrap()
        );
        assert_eq!(
            event_store
                .conversations()
                .unwrap()
                .first()
                .unwrap()
                .membership_version,
            2
        );
        drop(restored);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn group_epoch_delivery_barrier_waits_for_every_recipient_acknowledgement() {
        let store = LocalChatStore::in_memory();
        let owner = User::new(&"1".repeat(64), "owner", "Owner", "O", 1, "Online");
        let first = User::new(&"2".repeat(64), "first", "First", "F", 2, "Online");
        let second = User::new(&"3".repeat(64), "second", "Second", "S", 3, "Online");
        let added = User::new(&"4".repeat(64), "added", "Added", "A", 4, "Online");
        let group_id = "0123456789abcdef0123456789abcdef";
        store
            .create_group(
                group_id,
                "Project",
                &[owner.clone(), first.clone(), second.clone()],
            )
            .unwrap();
        let deliveries = [&first, &second, &added]
            .iter()
            .map(|recipient| RelayDelivery {
                recipient_public_key: recipient.id.clone(),
                envelope: crate::services::relay::RelayEnvelope {
                    message_id: new_id().unwrap(),
                    sender_id: owner.id.clone(),
                    recipient_id: recipient.id.clone(),
                    group_id: Some(group_id.to_string()),
                    sent_at: "1".into(),
                    ephemeral_public_key: String::new(),
                    nonce: String::new(),
                    ciphertext: String::new(),
                    signature: String::new(),
                },
            })
            .collect::<Vec<_>>();
        let delivery_ids = deliveries
            .iter()
            .map(|delivery| delivery.envelope.message_id.clone())
            .collect::<Vec<_>>();
        store
            .save_pending_group_change(
                PendingGroupMembershipChange {
                    group_id: group_id.to_string(),
                    operation_id: new_id().unwrap(),
                    expected_version: 1,
                    members: vec![owner, first, second, added],
                    deliveries,
                },
                vec![1],
            )
            .unwrap();
        store
            .complete_pending_group_change(
                group_id,
                &store
                    .pending_group_change(group_id)
                    .unwrap()
                    .unwrap()
                    .operation_id,
                2,
                &[
                    "1".repeat(64),
                    "2".repeat(64),
                    "3".repeat(64),
                    "4".repeat(64),
                ],
                vec![2],
            )
            .unwrap();
        assert!(store.group_commit_delivery_pending(group_id).unwrap());
        store
            .acknowledge_group_commit_deliveries(&delivery_ids[..2])
            .unwrap();
        assert!(store.group_commit_delivery_pending(group_id).unwrap());
        store
            .acknowledge_group_commit_deliveries(&delivery_ids[2..])
            .unwrap();
        assert!(!store.group_commit_delivery_pending(group_id).unwrap());
    }

    #[test]
    fn group_conversations_and_messages_are_saved_and_reloaded() {
        let directory =
            std::env::temp_dir().join(format!("resonance-group-test-{}", new_id().unwrap()));
        let path = directory.join("local.json");
        let members = vec![
            User::new("creator", "creator", "Creator", "C", 0x123456, "Offline"),
            User::new("alice", "alice", "Alice", "A", 0x123456, "Offline"),
            User::new("bob", "bob", "Bob", "B", 0x123456, "Offline"),
        ];
        let store = LocalChatStore::open_at(path.clone(), b"group identity key").unwrap();
        let conversation = store
            .create_group("0123456789abcdef0123456789abcdef", "Project", &members)
            .unwrap();
        store
            .add_group_message(
                Message::new(
                    "group-message",
                    &conversation.id,
                    "creator",
                    "Creator",
                    "hello group",
                    "10",
                    true,
                ),
                &conversation.id,
                "Project",
                &members,
            )
            .unwrap();
        drop(store);

        let restored = LocalChatStore::open_at(path.clone(), b"group identity key").unwrap();
        let conversation = restored.conversations().unwrap().remove(0);
        assert!(conversation.is_group());
        assert_eq!(conversation.members.len(), 3);
        assert_eq!(conversation.name, "Project");
        assert_eq!(
            restored.messages().unwrap()[0].conversation_id,
            conversation.id
        );
        restored.close_conversation(&conversation.id).unwrap();
        let reopened = restored
            .create_group(&conversation.id, &conversation.name, &conversation.members)
            .unwrap();
        assert!(!reopened.is_closed);
        drop(restored);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn friend_sync_reports_new_accounts_once_and_persists_the_baseline() {
        let directory =
            std::env::temp_dir().join(format!("resonance-friend-sync-{}", new_id().unwrap()));
        let path = directory.join("local.json");
        let store = LocalChatStore::open_at(path.clone(), b"friend sync identity key").unwrap();
        let alice = User::new(&"a".repeat(64), "alice", "Alice", "A", 0x123456, "Offline");
        let bob = User::new(&"b".repeat(64), "bob", "Bob", "B", 0x123456, "Offline");

        assert_eq!(
            store.sync_friends(std::slice::from_ref(&alice)).unwrap(),
            vec![alice.clone()]
        );
        drop(store);

        let restored = LocalChatStore::open_at(path.clone(), b"friend sync identity key").unwrap();
        assert!(
            restored
                .sync_friends(std::slice::from_ref(&alice))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            restored.sync_friends(&[alice, bob.clone()]).unwrap(),
            vec![bob]
        );
        drop(restored);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn identity_verification_persists_and_is_bound_to_the_public_key() {
        let directory =
            std::env::temp_dir().join(format!("resonance-identity-trust-{}", new_id().unwrap()));
        let path = directory.join("local.json");
        let identity_key = b"identity verification persistence key";
        let alice = User::new(&"a".repeat(64), "alice", "Alice", "A", 0x123456, "Offline");
        let store = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        store.sync_friends(std::slice::from_ref(&alice)).unwrap();
        assert_eq!(
            store.contact_identity(&alice).unwrap().status,
            IdentityTrustStatus::Unverified
        );
        let fingerprint = crate::services::identity_verification::fingerprint(&alice.id).unwrap();
        assert!(store.verify_contact_identity(&alice, &fingerprint).is_ok());
        drop(store);

        let restored = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        let trust = restored.contact_identity(&alice).unwrap();
        assert_eq!(trust.status, IdentityTrustStatus::Verified);
        assert_eq!(trust.trusted_public_key.as_deref(), Some(alice.id.as_str()));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn live_acceptance_verifies_only_the_current_friend_key() {
        let store = LocalChatStore::in_memory();
        let alice = User::new(&"a".repeat(64), "alice", "Alice", "A", 0x123456, "Offline");
        store.sync_friends(std::slice::from_ref(&alice)).unwrap();

        assert!(
            store
                .verify_contact_identity_after_live_acceptance(&alice)
                .is_ok()
        );
        let trust = store.contact_identity(&alice).unwrap();
        assert_eq!(trust.status, IdentityTrustStatus::Verified);
        assert_eq!(trust.trusted_public_key.as_deref(), Some(alice.id.as_str()));

        let changed_key = User::new(&"b".repeat(64), "alice", "Alice", "A", 0x123456, "Offline");
        assert!(
            store
                .verify_contact_identity_after_live_acceptance(&changed_key)
                .is_err()
        );
        assert_eq!(
            store
                .contact_identity(&alice)
                .unwrap()
                .trusted_public_key
                .as_deref(),
            Some(alice.id.as_str())
        );
    }

    #[test]
    fn both_people_can_independently_verify_each_others_exact_public_key() {
        let directory =
            std::env::temp_dir().join(format!("resonance-mutual-identity-{}", new_id().unwrap()));
        let alice_store =
            LocalChatStore::open_at(directory.join("alice.json"), b"alice local identity").unwrap();
        let bob_store =
            LocalChatStore::open_at(directory.join("bob.json"), b"bob local identity").unwrap();
        let alice = User::new(&"a".repeat(64), "alice", "Alice", "A", 0x123456, "Offline");
        let bob = User::new(&"b".repeat(64), "bob", "Bob", "B", 0x123456, "Offline");
        alice_store
            .sync_friends(std::slice::from_ref(&bob))
            .unwrap();
        bob_store
            .sync_friends(std::slice::from_ref(&alice))
            .unwrap();

        alice_store
            .verify_contact_identity(
                &bob,
                &crate::services::identity_verification::fingerprint(&bob.id).unwrap(),
            )
            .unwrap();
        bob_store
            .verify_contact_identity(
                &alice,
                &crate::services::identity_verification::qr_payload(&alice.id).unwrap(),
            )
            .unwrap();

        assert_eq!(
            alice_store.contact_identity(&bob).unwrap().status,
            IdentityTrustStatus::Verified
        );
        assert_eq!(
            bob_store.contact_identity(&alice).unwrap().status,
            IdentityTrustStatus::Verified
        );
        drop(alice_store);
        drop(bob_store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn changed_keys_are_blocked_until_explicitly_reverified_and_revocation_blocks_again() {
        let directory =
            std::env::temp_dir().join(format!("resonance-key-change-{}", new_id().unwrap()));
        let store = LocalChatStore::open_at(directory.join("local.json"), b"key change protection")
            .unwrap();
        let original = User::new(&"a".repeat(64), "alice", "Alice", "A", 0x123456, "Offline");
        store.sync_friends(std::slice::from_ref(&original)).unwrap();
        store
            .verify_contact_identity(
                &original,
                &crate::services::identity_verification::fingerprint(&original.id).unwrap(),
            )
            .unwrap();

        let replacement = User::new(&"b".repeat(64), "alice", "Alice", "A", 0x123456, "Offline");
        store
            .sync_friends(std::slice::from_ref(&replacement))
            .unwrap();
        let changed = store.contact_identity(&replacement).unwrap();
        assert_eq!(changed.status, IdentityTrustStatus::Changed);
        assert_eq!(
            changed.trusted_public_key.as_deref(),
            Some(original.id.as_str())
        );
        assert!(store.ensure_identity_not_changed(&replacement).is_err());
        assert!(store.ensure_identity_verified(&replacement).is_err());
        assert!(
            store
                .verify_contact_identity(&replacement, &original.id)
                .is_err()
        );

        store
            .verify_contact_identity(
                &replacement,
                &crate::services::identity_verification::fingerprint(&replacement.id).unwrap(),
            )
            .unwrap();
        let verified = store.contact_identity(&replacement).unwrap();
        assert_eq!(verified.status, IdentityTrustStatus::Verified);
        assert_eq!(verified.previous_public_keys, vec![original.id.clone()]);
        store.ensure_identity_not_changed(&replacement).unwrap();
        store.ensure_identity_verified(&replacement).unwrap();
        store.revoke_contact_verification(&replacement).unwrap();
        assert_eq!(
            store.contact_identity(&replacement).unwrap().status,
            IdentityTrustStatus::Revoked
        );
        assert!(store.ensure_identity_not_changed(&replacement).is_err());
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn new_accounts_remain_unverified_without_replacing_existing_trust() {
        let directory =
            std::env::temp_dir().join(format!("resonance-new-device-{}", new_id().unwrap()));
        let store = LocalChatStore::open_at(directory.join("local.json"), b"account identity list")
            .unwrap();
        let alice = User::new(&"a".repeat(64), "alice", "Alice", "A", 0x123456, "Offline");
        let device_account = User::new(
            &"c".repeat(64),
            "alice-device",
            "Alice's device",
            "AD",
            0x123456,
            "Offline",
        );
        store.sync_friends(std::slice::from_ref(&alice)).unwrap();
        store
            .verify_contact_identity(
                &alice,
                &crate::services::identity_verification::fingerprint(&alice.id).unwrap(),
            )
            .unwrap();
        store
            .sync_friends(&[alice.clone(), device_account.clone()])
            .unwrap();
        assert_eq!(
            store.contact_identity(&alice).unwrap().status,
            IdentityTrustStatus::Verified
        );
        assert_eq!(
            store.contact_identity(&device_account).unwrap().status,
            IdentityTrustStatus::Unverified
        );
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn inconsistent_friend_identity_responses_are_rejected_without_overwriting_trust() {
        let directory = std::env::temp_dir().join(format!(
            "resonance-inconsistent-friends-{}",
            new_id().unwrap()
        ));
        let store =
            LocalChatStore::open_at(directory.join("local.json"), b"inconsistent identity list")
                .unwrap();
        let alice = User::new(&"a".repeat(64), "alice", "Alice", "A", 0x123456, "Offline");
        let spoofed = User::new(
            &"b".repeat(64),
            "ALICE",
            "Different",
            "D",
            0x123456,
            "Offline",
        );
        store.sync_friends(std::slice::from_ref(&alice)).unwrap();
        assert!(store.sync_friends(&[alice.clone(), spoofed]).is_err());
        assert_eq!(
            store.contact_identity(&alice).unwrap().observed_public_key,
            alice.id
        );
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn encrypted_chat_history_requires_its_identity_key() {
        let directory =
            std::env::temp_dir().join(format!("resonance-chat-key-test-{}", new_id().unwrap()));
        let path = directory.join("local.json");
        let store = LocalChatStore::open_at(path.clone(), b"first account key").unwrap();
        let peer = User::new("peer-id", "peer", "Peer", "P", 0x123456, "Online");
        store.conversation_for(&peer).unwrap();
        drop(store);

        assert!(LocalChatStore::open_at(path.clone(), b"another account key").is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn legacy_plaintext_history_is_migrated_to_encrypted_storage() {
        let directory =
            std::env::temp_dir().join(format!("resonance-chat-migration-{}", new_id().unwrap()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local.json");
        fs::write(&path, serde_json::to_vec(&ChatData::default()).unwrap()).unwrap();

        let _store = LocalChatStore::open_at(path.clone(), b"legacy account key").unwrap();

        let on_disk = fs::read(&path).unwrap();
        assert!(on_disk.starts_with(FILE_MAGIC));
        assert!(!path.with_extension("bak").exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn interrupted_chat_replacement_recovers_the_previous_device_copy() {
        let directory =
            std::env::temp_dir().join(format!("resonance-chat-recovery-{}", new_id().unwrap()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("local.json");
        let backup = path.with_extension("bak");
        fs::write(&backup, serde_json::to_vec(&ChatData::default()).unwrap()).unwrap();

        let store = LocalChatStore::open_at(path.clone(), b"recovery account key").unwrap();

        assert!(path.exists());
        assert!(store.messages().unwrap().is_empty());
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn recent_conversations_are_descending_and_closed_chats_reopen_from_a_friend() {
        let store = LocalChatStore::in_memory();
        let older = User::new("older", "older", "Older", "O", 0x123456, "Offline");
        let newer = User::new("newer", "newer", "Newer", "N", 0x123456, "Offline");
        let older_conversation = store.conversation_for(&older).unwrap();
        let newer_conversation = store.conversation_for(&newer).unwrap();
        store
            .add_message(
                Message::new(
                    "old-message",
                    &older_conversation.id,
                    "me",
                    "Me",
                    "old",
                    "100",
                    true,
                ),
                &older,
            )
            .unwrap();
        store
            .add_message(
                Message::new(
                    "new-message",
                    &newer_conversation.id,
                    "me",
                    "Me",
                    "new",
                    "200",
                    true,
                ),
                &newer,
            )
            .unwrap();

        assert_eq!(store.conversations().unwrap()[0].id, newer_conversation.id);
        store.close_conversation(&newer_conversation.id).unwrap();
        assert!(store.conversations().unwrap()[0].is_closed);
        assert!(!store.conversation_for(&newer).unwrap().is_closed);
    }

    #[test]
    fn message_deletion_is_indicated_and_receipt_times_are_retained() {
        let store = LocalChatStore::in_memory();
        let peer = User::new("peer-id", "peer", "Peer", "P", 0x123456, "Online");
        let conversation = store.conversation_for(&peer).unwrap();
        store
            .add_message(
                Message::new(
                    "message-id",
                    &conversation.id,
                    "me",
                    "Me",
                    "hello",
                    "100",
                    true,
                ),
                &peer,
            )
            .unwrap();
        store
            .update_receipt("message-id", Some("200".into()), Some("300".into()))
            .unwrap();
        store.delete_message("message-id").unwrap();

        let message = store.messages().unwrap().remove(0);
        assert!(message.deleted);
        assert!(message.body.is_empty());
        assert_eq!(message.sent_at, "100");
        assert_eq!(message.delivered_at.as_deref(), Some("200"));
        assert_eq!(message.read_at.as_deref(), Some("300"));
    }

    #[test]
    fn unchanged_receipt_does_not_rewrite_chat_history() {
        let directory =
            std::env::temp_dir().join(format!("resonance-receipt-noop-{}", new_id().unwrap()));
        let path = directory.join("local.json");
        let identity_key = b"receipt identity key";
        let store = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        let peer = User::new("peer-id", "peer", "Peer", "P", 0x123456, "Online");
        let conversation = store.conversation_for(&peer).unwrap();
        store
            .add_message(
                Message::new(
                    "message-id",
                    &conversation.id,
                    "me",
                    "Me",
                    "hello",
                    "100",
                    true,
                ),
                &peer,
            )
            .unwrap();
        store
            .update_receipt("message-id", Some("200".into()), Some("300".into()))
            .unwrap();
        let before = fs::read(&path).unwrap();

        store
            .update_receipt("message-id", Some("200".into()), Some("300".into()))
            .unwrap();

        assert_eq!(fs::read(&path).unwrap(), before);
        drop(store);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn queued_message_state_survives_restart_and_duplicate_delivery_is_idempotent() {
        let directory =
            std::env::temp_dir().join(format!("resonance-queued-message-{}", new_id().unwrap()));
        let path = directory.join("local.json");
        let identity_key = b"queued message identity key";
        let peer = User::new("peer-id", "peer", "Peer", "P", 0x123456, "Online");
        let store = LocalChatStore::open_at(path.clone(), identity_key).unwrap();
        let conversation = store.conversation_for(&peer).unwrap();
        let message = Message::new(
            "message-id",
            &conversation.id,
            "me",
            "Me",
            "queued",
            "100",
            true,
        );
        store.add_message(message.clone(), &peer).unwrap();
        store.set_delivery_state(&message.id, "relayed").unwrap();
        let queued_at = store.messages().unwrap()[0].last_queued_at.clone();
        drop(store);

        let restored = LocalChatStore::open_at(path, identity_key).unwrap();
        restored.add_message(message, &peer).unwrap();
        let messages = restored.messages().unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].delivery_state, "relayed");
        assert_eq!(messages[0].last_queued_at, queued_at);
        drop(restored);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn editing_a_message_updates_its_local_conversation_preview() {
        let store = LocalChatStore::in_memory();
        let peer = User::new("peer-id", "peer", "Peer", "P", 0x123456, "Online");
        let conversation = store.conversation_for(&peer).unwrap();
        store
            .add_message(
                Message::new(
                    "message-id",
                    &conversation.id,
                    "me",
                    "Me",
                    "before edit",
                    "100",
                    true,
                ),
                &peer,
            )
            .unwrap();

        store.edit_message("message-id", "after edit").unwrap();

        assert_eq!(store.messages().unwrap()[0].body, "after edit");
        assert_eq!(store.conversations().unwrap()[0].preview, "after edit");
    }
}
