use std::{
    collections::HashMap,
    net::UdpSocket,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};

use ring::signature::{self, KeyPair};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::{
    models::{message::Message, user::User},
    services::{
        attachment::{self, AttachmentMetadata, MAX_ATTACHMENTS_PER_MESSAGE},
        identity::UnlockedIdentity,
        local_chat_store::LocalChatStore,
        p2p_transport::P2pTransport,
        voice_call::VoiceCall,
    },
};

#[derive(Clone)]
pub(crate) struct PeerContext {
    pub(crate) account_id: String,
    pub(crate) display_name: String,
    pub(crate) private_key: Arc<Zeroizing<Vec<u8>>>,
    pub(crate) friends: HashMap<String, User>,
    pub(crate) store: LocalChatStore,
}

#[derive(Clone)]
pub struct PeerTransport {
    context: Arc<RwLock<Option<PeerContext>>>,
    voice_calls: Arc<RwLock<HashMap<String, Arc<VoiceCall>>>>,
    p2p: P2pTransport,
    port: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PeerEnvelope {
    pub(crate) id: String,
    pub(crate) conversation_id: String,
    pub(crate) author_id: String,
    pub(crate) author_name: String,
    pub(crate) body: String,
    pub(crate) time: String,
    #[serde(default)]
    pub(crate) is_group: bool,
    #[serde(default)]
    pub(crate) attachments: Vec<AttachmentMetadata>,
}

impl PeerTransport {
    pub fn bind() -> Result<Self, String> {
        let port = std::env::var("RESONANCE_P2P_PORT")
            .ok()
            .map(|value| {
                value
                    .parse::<u16>()
                    .map_err(|_| "RESONANCE_P2P_PORT must be a valid UDP port.".to_string())
            })
            .transpose()?
            .unwrap_or(0);
        Self::bind_on(port)
    }

    fn bind_on(port: u16) -> Result<Self, String> {
        let port = if port == 0 {
            let socket = UdpSocket::bind(("0.0.0.0", 0))
                .map_err(|error| format!("Could not allocate a P2P UDP port: {error}"))?;
            socket
                .local_addr()
                .map_err(|error| format!("Could not determine the P2P UDP port: {error}"))?
                .port()
        } else {
            port
        };
        Ok(Self {
            context: Arc::new(RwLock::new(None)),
            voice_calls: Arc::new(RwLock::new(HashMap::new())),
            p2p: P2pTransport::new(port),
            port,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn configure(
        &self,
        identity: &UnlockedIdentity,
        friends: &[User],
        store: LocalChatStore,
    ) -> Result<(), String> {
        let key = signature::Ed25519KeyPair::from_pkcs8(identity.private_key())
            .map_err(|_| "The unlocked identity key cannot sign peer messages.".to_string())?;
        if encode_hex(key.public_key().as_ref()) != identity.user.id {
            return Err("The unlocked identity does not match the account public key.".into());
        }
        let context = PeerContext {
            account_id: identity.user.id.clone(),
            display_name: identity.user.name.clone(),
            private_key: Arc::new(Zeroizing::new(identity.private_key().to_vec())),
            friends: friends
                .iter()
                .cloned()
                .map(|friend| (friend.id.clone(), friend))
                .collect(),
            store,
        };
        self.p2p.configure(&context)?;
        *self
            .context
            .write()
            .map_err(|_| "Peer networking is unavailable.".to_string())? = Some(context);
        Ok(())
    }

    pub fn clear(&self) {
        self.p2p.clear();
        if let Ok(mut context) = self.context.write() {
            *context = None;
        }
        self.end_all_voice_calls();
    }

    pub fn start_voice_call(
        &self,
        call_id: &str,
        conversation_id: &str,
        peer_ids: Vec<String>,
    ) -> Result<(), String> {
        let context = self
            .context
            .read()
            .map_err(|_| "Peer networking is unavailable.".to_string())?
            .clone()
            .ok_or_else(|| "Sign in before starting a voice call.".to_string())?;
        if self
            .voice_calls
            .read()
            .map_err(|_| "Voice calls are unavailable.".to_string())?
            .contains_key(call_id)
        {
            return Err("This voice call is already active.".into());
        }
        let call = VoiceCall::start(call_id, conversation_id, &context.account_id, peer_ids)?;
        self.voice_calls
            .write()
            .map_err(|_| "Voice calls are unavailable.".to_string())?
            .insert(call_id.to_string(), call);
        Ok(())
    }

    pub fn create_voice_offer(&self, peer_id: &str, call_id: &str) -> Result<String, String> {
        let context = self.signed_in_context()?;
        if !context.friends.contains_key(peer_id) {
            return Err("The participant is no longer in the local friend list.".into());
        }
        let call = self.voice_call(call_id)?;
        call.authorize_peer(peer_id)?;
        call.create_offer(peer_id)
    }

    pub fn create_voice_ice_restart_offer(
        &self,
        peer_id: &str,
        call_id: &str,
    ) -> Result<String, String> {
        let context = self.signed_in_context()?;
        if !context.friends.contains_key(peer_id) {
            return Err("The participant is no longer in the local friend list.".into());
        }
        self.voice_call(call_id)?.create_ice_restart_offer(peer_id)
    }

    pub fn accept_voice_offer(
        &self,
        peer_id: &str,
        call_id: &str,
        offer: &str,
    ) -> Result<String, String> {
        let context = self.signed_in_context()?;
        if !context.friends.contains_key(peer_id) {
            return Err("The participant is no longer in the local friend list.".into());
        }
        let call = self.voice_call(call_id)?;
        call.authorize_peer(peer_id)?;
        call.accept_offer(peer_id, offer)
    }

    pub fn apply_voice_answer(
        &self,
        peer_id: &str,
        call_id: &str,
        answer: &str,
    ) -> Result<(), String> {
        self.voice_call(call_id)?.apply_answer(peer_id, answer)
    }

    pub fn take_voice_ice_candidates(
        &self,
        peer_id: &str,
        call_id: &str,
    ) -> Result<tokio::sync::mpsc::UnboundedReceiver<String>, String> {
        self.voice_call(call_id)?.take_ice_candidates(peer_id)
    }

    pub fn add_voice_ice_candidate(
        &self,
        peer_id: &str,
        call_id: &str,
        candidate: &str,
    ) -> Result<(), String> {
        self.voice_call(call_id)?
            .add_ice_candidate(peer_id, candidate)
    }

    pub fn authorize_voice_peer(&self, call_id: &str, peer_id: &str) -> Result<(), String> {
        self.voice_call(call_id)?.authorize_peer(peer_id)
    }

    pub fn voice_peer_state(
        &self,
        peer_id: &str,
        call_id: &str,
    ) -> Result<tokio::sync::watch::Receiver<webrtc::peer_connection::RTCPeerConnectionState>, String>
    {
        self.voice_call(call_id)?
            .peer_state(peer_id)
            .ok_or_else(|| "The WebRTC peer session is not available yet.".to_string())
    }

    pub fn set_voice_call_muted(&self, call_id: &str, muted: bool) -> Result<(), String> {
        self.voice_call(call_id)?.set_muted(muted);
        Ok(())
    }

    pub fn remove_voice_peer(&self, call_id: &str, peer_id: &str) -> Result<(), String> {
        self.voice_call(call_id)?.remove_peer(peer_id);
        Ok(())
    }

    pub fn end_voice_call(&self, call_id: &str) {
        if let Ok(mut calls) = self.voice_calls.write()
            && let Some(call) = calls.remove(call_id)
        {
            call.stop();
        }
    }

    fn end_all_voice_calls(&self) {
        if let Ok(mut calls) = self.voice_calls.write() {
            for call in calls.values() {
                call.stop();
            }
            calls.clear();
        }
    }

    pub fn send(&self, peer: &User, message: &Message) -> Result<(), String> {
        let context = self.signed_in_context()?;
        self.p2p.send(peer, message, &context)
    }

    pub fn request_identity_verification(&self, peer: &User) -> Result<String, String> {
        let context = self.signed_in_context()?;
        if !context.friends.contains_key(&peer.id) {
            return Err("The participant is no longer in the local friend list.".into());
        }
        self.p2p.request_identity_verification(peer)
    }

    pub fn respond_to_identity_verification(
        &self,
        peer: &User,
        request_id: &str,
        accepted: bool,
    ) -> Result<(), String> {
        let context = self.signed_in_context()?;
        if !context.friends.contains_key(&peer.id) {
            return Err("The participant is no longer in the local friend list.".into());
        }
        self.p2p
            .respond_to_identity_verification(peer, request_id, accepted)
    }

    pub fn take_identity_verification_requests(
        &self,
    ) -> Result<Vec<crate::services::p2p_transport::PendingIdentityVerification>, String> {
        self.p2p.take_identity_verification_requests()
    }

    pub fn download_attachment<F>(
        &self,
        peer: &User,
        conversation_id: &str,
        metadata: &AttachmentMetadata,
        cancelled: &AtomicBool,
        mut progress: F,
    ) -> Result<Vec<u8>, String>
    where
        F: FnMut(u64, u64),
    {
        attachment::validate_metadata(metadata)?;
        let context = self.signed_in_context()?;
        if let Some(binding) = metadata.binding.as_ref()
            && (binding.conversation_id != conversation_id
                || binding.sender_id != peer.id
                || !binding
                    .participants
                    .iter()
                    .any(|id| id == &context.account_id)
                || !binding.participants.iter().any(|id| id == &peer.id))
        {
            return Err("The attachment is not bound to this peer and conversation.".into());
        }
        let expected_size = metadata.size as u64 + 16;
        let mut offset = context
            .store
            .attachment_download_offset(&metadata.attachment_id)?;
        if offset > expected_size {
            return Err("The partial attachment exceeds the expected file size.".into());
        }
        progress(offset, expected_size);
        while offset < expected_size {
            if cancelled.load(Ordering::Relaxed) {
                return Err("Attachment transfer was cancelled.".into());
            }
            let (response_offset, total_size, bytes) = self.p2p.get_attachment_chunk(
                peer,
                conversation_id,
                &metadata.attachment_id,
                offset,
                false,
            )?;
            if response_offset != offset
                || total_size != expected_size
                || bytes.is_empty()
                || bytes.len() > 256 * 1024
                || offset.saturating_add(bytes.len() as u64) > expected_size
            {
                return Err("The peer returned an invalid attachment chunk.".into());
            }
            if cancelled.load(Ordering::Relaxed) {
                return Err("Attachment transfer was cancelled.".into());
            }
            context.store.append_attachment_download_chunk(
                &metadata.attachment_id,
                offset,
                total_size,
                &bytes,
            )?;
            offset += bytes.len() as u64;
            progress(offset, expected_size);
        }
        if cancelled.load(Ordering::Relaxed) {
            return Err("Attachment transfer was cancelled.".into());
        }
        context.store.finish_attachment_download(metadata)?;
        context
            .store
            .read_attachment_ciphertext(&metadata.attachment_id)
    }

    pub fn acknowledge_attachment(
        &self,
        peer: &User,
        conversation_id: &str,
        metadata: &AttachmentMetadata,
    ) -> Result<(), String> {
        attachment::validate_metadata(metadata)?;
        let context = self.signed_in_context()?;
        if let Some(binding) = metadata.binding.as_ref()
            && (binding.conversation_id != conversation_id
                || binding.sender_id != peer.id
                || !binding
                    .participants
                    .iter()
                    .any(|id| id == &context.account_id)
                || !binding.participants.iter().any(|id| id == &peer.id))
        {
            return Err(
                "The attachment confirmation is not bound to this peer and conversation.".into(),
            );
        }
        self.p2p
            .acknowledge_attachment(peer, conversation_id, &metadata.attachment_id)
    }

    fn signed_in_context(&self) -> Result<PeerContext, String> {
        self.context
            .read()
            .map_err(|_| "Peer networking is unavailable.".to_string())?
            .clone()
            .ok_or_else(|| "Sign in before using peer networking.".to_string())
    }

    fn voice_call(&self, call_id: &str) -> Result<Arc<VoiceCall>, String> {
        self.voice_calls
            .read()
            .map_err(|_| "Voice calls are unavailable.".to_string())?
            .get(call_id)
            .cloned()
            .ok_or_else(|| "This voice call is no longer active.".to_string())
    }
}

fn validate_body(body: &str) -> Result<(), String> {
    if body.trim().is_empty()
        || body.len() > 4000
        || body
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r'))
    {
        return Err("Message must contain 1-4000 visible bytes.".into());
    }
    Ok(())
}

pub(crate) fn validate_peer_message(envelope: &PeerEnvelope) -> Result<(), String> {
    if envelope.body.trim().is_empty() && envelope.attachments.is_empty() {
        return Err("A peer message must contain text or an attachment.".into());
    }
    if !envelope.body.trim().is_empty() {
        validate_body(&envelope.body)?;
    }
    if envelope.time.is_empty()
        || envelope.time.len() > 20
        || !envelope.time.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("The peer message timestamp is invalid.".into());
    }
    if envelope.id.is_empty()
        || envelope.id.len() > 128
        || envelope.id.chars().any(char::is_control)
    {
        return Err("The peer message identifier is invalid.".into());
    }
    if envelope.attachments.len() > MAX_ATTACHMENTS_PER_MESSAGE {
        return Err("A peer message can contain no more than 5 attachments.".into());
    }
    let mut attachment_ids = std::collections::HashSet::with_capacity(envelope.attachments.len());
    for metadata in &envelope.attachments {
        attachment::validate_metadata(metadata)?;
        if !attachment_ids.insert(metadata.attachment_id.to_ascii_lowercase()) {
            return Err("The peer message contains a duplicate attachment.".into());
        }
    }
    Ok(())
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 0x0f) as usize] as char);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{
        attachment::{self, AttachmentKind},
        identity::IdentityService,
        local_chat_store::LocalChatStore,
    };

    #[test]
    fn cancelled_transfer_keeps_partial_ciphertext_available_for_resume() {
        let directory = std::env::temp_dir().join(format!(
            "resonance-transfer-cancel-{}",
            crate::services::local_chat_store::new_id().unwrap()
        ));
        let store =
            LocalChatStore::open_at(directory.join("local.json"), b"cancel test identity").unwrap();
        let attachment_id = "0123456789abcdef0123456789abcdef";
        let plaintext = b"resumable transfer";
        let (key, nonce, ciphertext) = attachment::encrypt(attachment_id, plaintext).unwrap();
        let metadata = AttachmentMetadata {
            attachment_id: attachment_id.into(),
            file_name: "resumable.txt".into(),
            content_type: "text/plain".into(),
            size: plaintext.len(),
            kind: AttachmentKind::File,
            key,
            nonce,
            binding: None,
        };
        store
            .append_attachment_download_chunk(
                attachment_id,
                0,
                ciphertext.len() as u64,
                &ciphertext[..16],
            )
            .unwrap();
        let (identity, _) = IdentityService::new()
            .create("cancel-user", "Cancel User", "a sufficiently long password")
            .unwrap();
        let transport = PeerTransport::bind_on(0).unwrap();
        *transport.context.write().unwrap() = Some(PeerContext {
            account_id: identity.user.id.clone(),
            display_name: identity.user.name.clone(),
            private_key: Arc::new(Zeroizing::new(identity.private_key().to_vec())),
            friends: HashMap::new(),
            store: store.clone(),
        });
        let cancelled = AtomicBool::new(true);
        let result = transport.download_attachment(
            &identity.user,
            "0123456789abcdef0123456789abcdef",
            &metadata,
            &cancelled,
            |_, _| {},
        );

        assert_eq!(result.unwrap_err(), "Attachment transfer was cancelled.");
        assert_eq!(store.attachment_download_offset(attachment_id).unwrap(), 16);
        assert!(!store.has_attachment_ciphertext(attachment_id).unwrap());

        drop(transport);
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
