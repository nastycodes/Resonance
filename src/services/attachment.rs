use std::collections::HashSet;
use std::fmt;

use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

pub const MAX_ATTACHMENT_BYTES: usize = 25 * 1024 * 1024;
const TAG_BYTES: usize = 16;
const MAX_PLAINTEXT_BYTES: usize = MAX_ATTACHMENT_BYTES - TAG_BYTES;
const ATTACHMENT_AAD_PREFIX: &str = "resonance-attachment-v1:";
const BOUND_ATTACHMENT_AAD_PREFIX: &str = "resonance-attachment-v2:";
const RELAY_PREFIX: &str = "resonance-attachment-message-v1:";
const MESSAGE_CONTENT_PREFIX: &str = "resonance-message-content-v2:";
pub const MAX_ATTACHMENTS_PER_MESSAGE: usize = 5;

#[derive(Deserialize, Serialize)]
struct MessageContentPayload {
    body: String,
    attachments: Vec<AttachmentMetadata>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    File,
    Voice,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentBinding {
    pub conversation_id: String,
    pub sender_id: String,
    pub participants: Vec<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentMetadata {
    pub attachment_id: String,
    pub file_name: String,
    pub content_type: String,
    pub size: usize,
    pub kind: AttachmentKind,
    pub key: String,
    pub nonce: String,
    #[serde(default)]
    pub binding: Option<AttachmentBinding>,
}

impl fmt::Debug for AttachmentMetadata {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AttachmentMetadata")
            .field("attachment_id", &self.attachment_id)
            .field("file_name", &self.file_name)
            .field("content_type", &self.content_type)
            .field("size", &self.size)
            .field("kind", &self.kind)
            .field("key", &"[REDACTED]")
            .field("nonce", &"[REDACTED]")
            .field("binding", &self.binding)
            .finish()
    }
}

pub fn encrypt(attachment_id: &str, plaintext: &[u8]) -> Result<(String, String, Vec<u8>), String> {
    encrypt_inner(attachment_id, plaintext, None)
}

pub fn encrypt_bound(
    attachment_id: &str,
    plaintext: &[u8],
    binding: &AttachmentBinding,
) -> Result<(String, String, Vec<u8>), String> {
    validate_binding(binding)?;
    encrypt_inner(attachment_id, plaintext, Some(binding))
}

fn encrypt_inner(
    attachment_id: &str,
    plaintext: &[u8],
    binding: Option<&AttachmentBinding>,
) -> Result<(String, String, Vec<u8>), String> {
    if !valid_id(attachment_id) {
        return Err("The attachment identifier is invalid.".into());
    }
    if plaintext.is_empty() || plaintext.len() > max_attachment_bytes()? {
        return Err("The file is empty or exceeds the configured attachment size limit.".into());
    }
    let mut key_bytes = Zeroizing::new([0u8; 32]);
    let mut nonce_bytes = [0u8; 12];
    let random = SystemRandom::new();
    random
        .fill(&mut key_bytes[..])
        .and_then(|_| random.fill(&mut nonce_bytes))
        .map_err(|_| "Could not generate an attachment encryption key.".to_string())?;
    let key = LessSafeKey::new(
        UnboundKey::new(&aead::AES_256_GCM, &key_bytes[..])
            .map_err(|_| "Could not initialize attachment encryption.".to_string())?,
    );
    let mut ciphertext = plaintext.to_vec();
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce_bytes),
        Aad::from(associated_data(attachment_id, binding)),
        &mut ciphertext,
    )
    .map_err(|_| "Could not encrypt the attachment.".to_string())?;
    Ok((
        encode_hex(&key_bytes[..]),
        encode_hex(&nonce_bytes),
        ciphertext,
    ))
}

pub fn max_attachment_bytes() -> Result<usize, String> {
    let configured = match std::env::var("RESONANCE_MAX_ATTACHMENT_BYTES") {
        Ok(value) => value.parse::<usize>().map_err(|_| {
            "RESONANCE_MAX_ATTACHMENT_BYTES must be a positive integer no greater than the 25 MiB protocol maximum.".to_string()
        })?,
        Err(std::env::VarError::NotPresent) => MAX_PLAINTEXT_BYTES,
        Err(std::env::VarError::NotUnicode(_)) => {
            return Err("RESONANCE_MAX_ATTACHMENT_BYTES is not valid Unicode.".into());
        }
    };
    if configured == 0 || configured > MAX_PLAINTEXT_BYTES {
        return Err(
            "RESONANCE_MAX_ATTACHMENT_BYTES must be between 1 and the 25 MiB protocol maximum."
                .into(),
        );
    }
    Ok(configured)
}

pub fn attachment_binding(
    conversation_id: &str,
    sender_id: &str,
    participants: impl IntoIterator<Item = String>,
) -> Result<AttachmentBinding, String> {
    let mut participants = participants.into_iter().collect::<Vec<_>>();
    participants.sort();
    participants.dedup();
    let binding = AttachmentBinding {
        conversation_id: conversation_id.to_string(),
        sender_id: sender_id.to_string(),
        participants,
    };
    validate_binding(&binding)?;
    Ok(binding)
}

pub fn validate_binding(binding: &AttachmentBinding) -> Result<(), String> {
    let participants_are_canonical = binding
        .participants
        .windows(2)
        .all(|pair| pair[0] < pair[1]);
    if !valid_id(&binding.conversation_id)
        || !valid_account_id(&binding.sender_id)
        || !(2..=256).contains(&binding.participants.len())
        || !participants_are_canonical
        || binding
            .participants
            .iter()
            .any(|participant| !valid_account_id(participant))
        || !binding
            .participants
            .iter()
            .any(|participant| participant == &binding.sender_id)
    {
        return Err("The attachment conversation binding is invalid.".into());
    }
    Ok(())
}

fn associated_data(attachment_id: &str, binding: Option<&AttachmentBinding>) -> Vec<u8> {
    match binding {
        Some(binding) => {
            let mut aad = format!(
                "{BOUND_ATTACHMENT_AAD_PREFIX}{attachment_id}:{}:{}",
                binding.conversation_id, binding.sender_id
            );
            for participant in &binding.participants {
                aad.push(':');
                aad.push_str(participant);
            }
            aad.into_bytes()
        }
        None => format!("{ATTACHMENT_AAD_PREFIX}{attachment_id}").into_bytes(),
    }
}

pub fn decrypt(
    metadata: &AttachmentMetadata,
    ciphertext: &[u8],
) -> Result<Zeroizing<Vec<u8>>, String> {
    validate_metadata(metadata)?;
    if ciphertext.len() > MAX_ATTACHMENT_BYTES || ciphertext.len() < TAG_BYTES {
        return Err("The downloaded attachment has an invalid size.".into());
    }
    let decoded_key = Zeroizing::new(
        decode_hex(&metadata.key)
            .filter(|bytes| bytes.len() == 32)
            .ok_or_else(|| "The attachment encryption key is invalid.".to_string())?,
    );
    let mut key_bytes = Zeroizing::new([0u8; 32]);
    key_bytes.copy_from_slice(&decoded_key);
    let nonce_bytes = decode_hex(&metadata.nonce)
        .filter(|bytes| bytes.len() == 12)
        .ok_or_else(|| "The attachment nonce is invalid.".to_string())?;
    let nonce: [u8; 12] = nonce_bytes
        .try_into()
        .map_err(|_| "The attachment nonce is invalid.".to_string())?;
    let key = LessSafeKey::new(
        UnboundKey::new(&aead::AES_256_GCM, &key_bytes[..])
            .map_err(|_| "The attachment encryption key is invalid.".to_string())?,
    );
    let mut plaintext = Zeroizing::new(ciphertext.to_vec());
    let plaintext_len = {
        let result = key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(associated_data(
                    &metadata.attachment_id,
                    metadata.binding.as_ref(),
                )),
                &mut plaintext[..],
            )
            .map_err(|_| "The attachment could not be authenticated or decrypted.".to_string())?;
        result.len()
    };
    if plaintext_len != metadata.size {
        return Err("The decrypted attachment size does not match its message.".into());
    }
    plaintext.truncate(plaintext_len);
    Ok(plaintext)
}

pub fn validate_metadata(metadata: &AttachmentMetadata) -> Result<(), String> {
    if !valid_id(&metadata.attachment_id)
        || metadata.file_name.trim().is_empty()
        || metadata.file_name.chars().count() > 255
        || metadata.file_name.chars().any(char::is_control)
        || metadata
            .file_name
            .chars()
            .any(|character| matches!(character, '/' | '\\'))
        || metadata.content_type.is_empty()
        || metadata.content_type.len() > 128
        || metadata.content_type.chars().any(char::is_control)
        || metadata.size == 0
        || metadata.size > max_attachment_bytes()?
        || decode_hex(&metadata.key).map(|key| key.len()) != Some(32)
        || decode_hex(&metadata.nonce).map(|nonce| nonce.len()) != Some(12)
    {
        return Err("The received attachment details are invalid.".into());
    }
    if let Some(binding) = metadata.binding.as_ref() {
        validate_binding(binding)?;
    }
    Ok(())
}

fn valid_account_id(id: &str) -> bool {
    id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn valid_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn is_image_previewable(metadata: &AttachmentMetadata) -> bool {
    metadata.kind == AttachmentKind::File
        && matches!(
            metadata.content_type.as_str(),
            "image/png"
                | "image/jpeg"
                | "image/jpg"
                | "image/gif"
                | "image/webp"
                | "image/bmp"
                | "image/tiff"
        )
}

pub fn is_audio_playable(metadata: &AttachmentMetadata) -> bool {
    metadata.kind == AttachmentKind::Voice
        || matches!(
            metadata.content_type.as_str(),
            "audio/wav" | "audio/x-wav" | "audio/mpeg" | "audio/ogg" | "audio/flac"
        )
}

pub fn encode_relay_payload(metadata: &AttachmentMetadata) -> Result<String, String> {
    validate_metadata(metadata)?;
    serde_json::to_string(metadata)
        .map(|payload| format!("{RELAY_PREFIX}{payload}"))
        .map_err(|error| format!("Could not encode the attachment message: {error}"))
}

pub fn decode_relay_payload(body: &str) -> Result<Option<AttachmentMetadata>, String> {
    let Some(payload) = body.strip_prefix(RELAY_PREFIX) else {
        return Ok(None);
    };
    let metadata: AttachmentMetadata = serde_json::from_str(payload)
        .map_err(|error| format!("The received attachment message is invalid: {error}"))?;
    validate_metadata(&metadata)?;
    Ok(Some(metadata))
}

pub fn encode_message_payload(
    body: &str,
    attachments: &[AttachmentMetadata],
) -> Result<String, String> {
    if attachments.len() > MAX_ATTACHMENTS_PER_MESSAGE {
        return Err("A message can contain no more than 5 attachments.".into());
    }
    if attachments.is_empty()
        && !body.starts_with(MESSAGE_CONTENT_PREFIX)
        && !body.starts_with(RELAY_PREFIX)
    {
        return Ok(body.to_string());
    }
    if !attachments.is_empty() {
        validate_attachments(attachments)?;
    }
    serde_json::to_string(&MessageContentPayload {
        body: body.to_string(),
        attachments: attachments.to_vec(),
    })
    .map(|payload| format!("{MESSAGE_CONTENT_PREFIX}{payload}"))
    .map_err(|error| format!("Could not encode the attachment message: {error}"))
}

pub fn decode_message_payload(
    body: &str,
) -> Result<Option<(String, Vec<AttachmentMetadata>)>, String> {
    if let Some(payload) = body.strip_prefix(MESSAGE_CONTENT_PREFIX) {
        let payload: MessageContentPayload = serde_json::from_str(payload)
            .map_err(|error| format!("The received attachment message is invalid: {error}"))?;
        if payload.attachments.len() > MAX_ATTACHMENTS_PER_MESSAGE {
            return Err("The received message has too many attachments.".into());
        }
        if !payload.attachments.is_empty() {
            validate_attachments(&payload.attachments)?;
        } else if payload.body.trim().is_empty() {
            return Err("The received message is invalid.".into());
        }
        return Ok(Some((payload.body, payload.attachments)));
    }
    if let Some(metadata) = decode_relay_payload(body)? {
        return Ok(Some((String::new(), vec![metadata])));
    }
    Ok(None)
}

fn validate_attachments(attachments: &[AttachmentMetadata]) -> Result<(), String> {
    if attachments.is_empty() || attachments.len() > MAX_ATTACHMENTS_PER_MESSAGE {
        return Err("A message must contain between 1 and 5 attachments.".into());
    }
    let mut ids = HashSet::with_capacity(attachments.len());
    for metadata in attachments {
        validate_metadata(metadata)?;
        if !ids.insert(metadata.attachment_id.to_ascii_lowercase()) {
            return Err("The attachment message contains a duplicate file.".into());
        }
    }
    Ok(())
}

pub fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            Some(((high << 4) | low) as u8)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(id: String, key: String, nonce: String, size: usize) -> AttachmentMetadata {
        AttachmentMetadata {
            attachment_id: id,
            file_name: "test.txt".into(),
            content_type: "text/plain".into(),
            size,
            kind: AttachmentKind::File,
            key,
            nonce,
            binding: None,
        }
    }

    #[test]
    fn encrypted_attachment_round_trips_and_is_bound_to_its_identifier() {
        let id = "0123456789abcdef0123456789abcdef";
        let plaintext = b"private attachment contents";
        let (key, nonce, ciphertext) = encrypt(id, plaintext).unwrap();
        let metadata = metadata(id.into(), key, nonce, plaintext.len());
        assert_eq!(
            decrypt(&metadata, &ciphertext).unwrap().as_slice(),
            plaintext
        );

        let mut changed_id = metadata;
        changed_id.attachment_id = "fedcba9876543210fedcba9876543210".into();
        assert!(decrypt(&changed_id, &ciphertext).is_err());
    }

    #[test]
    fn encrypted_attachment_rejects_tampering_and_unsafe_names() {
        let id = "0123456789abcdef0123456789abcdef";
        let (_, _, mut ciphertext) = encrypt(id, b"private attachment contents").unwrap();
        let (key, nonce, _) = encrypt(id, b"private attachment contents").unwrap();
        let mut metadata = metadata(id.into(), key, nonce, b"private attachment contents".len());
        ciphertext[0] ^= 0x80;
        assert!(decrypt(&metadata, &ciphertext).is_err());

        metadata.file_name = "../outside.txt".into();
        assert!(validate_metadata(&metadata).is_err());
    }

    #[test]
    fn encrypted_attachment_rejects_empty_oversized_and_invalid_identifier_inputs() {
        let id = "0123456789abcdef0123456789abcdef";
        assert!(encrypt(id, &[]).is_err());
        assert!(encrypt("not-an-id", b"private").is_err());
        assert!(encrypt(id, &vec![0; MAX_PLAINTEXT_BYTES + 1]).is_err());
    }

    #[test]
    fn bound_attachment_authenticates_conversation_sender_and_participants() {
        let id = "0123456789abcdef0123456789abcdef";
        let sender = "1".repeat(64);
        let recipient = "2".repeat(64);
        let binding = attachment_binding(
            "fedcba9876543210fedcba9876543210",
            &sender,
            [sender.clone(), recipient],
        )
        .unwrap();
        let plaintext = b"conversation-specific contents";
        let (key, nonce, ciphertext) = encrypt_bound(id, plaintext, &binding).unwrap();
        let mut metadata = metadata(id.into(), key, nonce, plaintext.len());
        metadata.binding = Some(binding.clone());
        assert_eq!(
            decrypt(&metadata, &ciphertext).unwrap().as_slice(),
            plaintext
        );

        metadata.binding.as_mut().unwrap().conversation_id =
            "0123456789abcdef0123456789abcdef".into();
        assert!(decrypt(&metadata, &ciphertext).is_err());
    }

    #[test]
    fn attachment_binding_rejects_duplicate_or_missing_participants() {
        let sender = "1".repeat(64);
        let duplicate = AttachmentBinding {
            conversation_id: "0123456789abcdef0123456789abcdef".into(),
            sender_id: sender.clone(),
            participants: vec![sender.clone(), sender.clone()],
        };
        assert!(validate_binding(&duplicate).is_err());
        let missing_sender = AttachmentBinding {
            conversation_id: "0123456789abcdef0123456789abcdef".into(),
            sender_id: sender,
            participants: vec!["2".repeat(64), "3".repeat(64)],
        };
        assert!(validate_binding(&missing_sender).is_err());
    }

    #[test]
    fn received_attachment_metadata_rejects_invalid_size_and_identity_binding() {
        let id = "0123456789abcdef0123456789abcdef";
        let (key, nonce, _) = encrypt(id, b"file").unwrap();
        let mut metadata = metadata(id.into(), key, nonce, 4);
        metadata.size = MAX_PLAINTEXT_BYTES + 1;
        assert!(validate_metadata(&metadata).is_err());

        metadata.size = 4;
        metadata.binding = Some(AttachmentBinding {
            conversation_id: id.into(),
            sender_id: "1".repeat(64),
            participants: vec!["2".repeat(64), "1".repeat(64)],
        });
        assert!(validate_metadata(&metadata).is_err());
    }

    #[test]
    fn message_payload_round_trips_text_and_multiple_attachments() {
        let first_id = "0123456789abcdef0123456789abcdef";
        let (first_key, first_nonce, _) = encrypt(first_id, b"first").unwrap();
        let first = metadata(first_id.into(), first_key, first_nonce, 5);
        let second_id = "fedcba9876543210fedcba9876543210";
        let (second_key, second_nonce, _) = encrypt(second_id, b"second").unwrap();
        let second = metadata(second_id.into(), second_key, second_nonce, 6);

        let payload =
            encode_message_payload("Here are the files", &[first.clone(), second.clone()]).unwrap();
        assert_eq!(
            decode_message_payload(&payload).unwrap(),
            Some(("Here are the files".into(), vec![first.clone(), second]))
        );
        assert_eq!(
            encode_message_payload("Plain text", &[]).unwrap(),
            "Plain text"
        );
        let reserved_prefix_text = format!("{MESSAGE_CONTENT_PREFIX}literal text");
        let escaped = encode_message_payload(&reserved_prefix_text, &[]).unwrap();
        assert_eq!(
            decode_message_payload(&escaped).unwrap(),
            Some((reserved_prefix_text, Vec::new()))
        );

        let legacy = encode_relay_payload(&first).unwrap();
        assert_eq!(
            decode_message_payload(&legacy).unwrap(),
            Some((String::new(), vec![first]))
        );
    }

    #[test]
    fn message_payload_rejects_duplicate_and_excess_attachments() {
        let id = "0123456789abcdef0123456789abcdef";
        let (key, nonce, _) = encrypt(id, b"file").unwrap();
        let file = metadata(id.into(), key, nonce, 4);
        assert!(encode_message_payload("", &[file.clone(), file.clone()]).is_err());
        let too_many = (0..=MAX_ATTACHMENTS_PER_MESSAGE)
            .map(|index| {
                let mut file = file.clone();
                file.attachment_id = format!("{index:032x}");
                file
            })
            .collect::<Vec<_>>();
        assert!(encode_message_payload("", &too_many).is_err());
    }

    #[test]
    fn image_and_audio_types_are_recognized_for_inline_controls() {
        let mut file = metadata(
            "0123456789abcdef0123456789abcdef".into(),
            "00".repeat(32),
            "00".repeat(12),
            1,
        );
        file.content_type = "image/webp".into();
        assert!(is_image_previewable(&file));
        assert!(!is_audio_playable(&file));
        file.content_type = "audio/mpeg".into();
        assert!(!is_image_previewable(&file));
        assert!(is_audio_playable(&file));
        file.kind = AttachmentKind::Voice;
        file.content_type = "audio/wav".into();
        assert!(is_audio_playable(&file));
    }
}
