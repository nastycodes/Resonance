use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::Duration,
};

use ed25519_dalek::{SigningKey, pkcs8::DecodePrivateKey};
use futures::StreamExt;
use libp2p::{
    Multiaddr, PeerId, SwarmBuilder, autonat,
    core::multiaddr::Protocol,
    dcutr, identify, identity, noise, relay, request_response,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, tls, yamux,
};
use serde::{Deserialize, Serialize};
use tokio::{runtime::Builder as RuntimeBuilder, sync::mpsc as tokio_mpsc};

use crate::{
    models::{message::Message, user::User},
    services::{
        attachment,
        local_chat_store::new_id,
        mls::{MLS_MESSAGE_PREFIX, MlsService},
        peer_transport::{PeerContext, PeerEnvelope, validate_peer_message},
    },
};

const MESSAGE_PROTOCOL: &str = "/resonance/message/1.0.0";
const IDENTITY_VERIFICATION_PROTOCOL: &str = "/resonance/identity-verification/1.0.0";
const FILE_PROTOCOL: &str = "/resonance/attachment/2.0.0";
const MAX_PENDING_REQUESTS: usize = 64;
const FILE_CHUNK_SIZE: usize = 256 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MessageRequest {
    envelope: PeerEnvelope,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct IdentityVerificationRequest {
    request_id: String,
    action: IdentityVerificationAction,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
enum IdentityVerificationAction {
    Request,
    Accept,
    Reject,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MessageResponse {
    accepted: bool,
    error: Option<String>,
}

type IdentityVerificationResponse = MessageResponse;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FileChunkRequest {
    attachment_id: String,
    conversation_id: String,
    offset: u64,
    requested_size: u32,
    #[serde(default)]
    verified: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct FileChunkResponse {
    offset: u64,
    total_size: u64,
    bytes: Vec<u8>,
    error: Option<String>,
}

#[derive(NetworkBehaviour)]
struct P2pBehaviour {
    requests: request_response::cbor::Behaviour<MessageRequest, MessageResponse>,
    identity_verifications: request_response::cbor::Behaviour<
        IdentityVerificationRequest,
        IdentityVerificationResponse,
    >,
    files: request_response::cbor::Behaviour<FileChunkRequest, FileChunkResponse>,
    autonat: autonat::Behaviour,
    dcutr: dcutr::Behaviour,
    identify: identify::Behaviour,
    relay: relay::client::Behaviour,
}

enum Command {
    Send {
        peer: User,
        envelope: PeerEnvelope,
        result: mpsc::SyncSender<Result<(), String>>,
    },
    IdentityVerification {
        peer: User,
        request_id: String,
        action: IdentityVerificationAction,
        result: mpsc::SyncSender<Result<(), String>>,
    },
    DownloadChunk {
        peer: User,
        attachment_id: String,
        conversation_id: String,
        offset: u64,
        verified: bool,
        result: mpsc::SyncSender<Result<(u64, u64, Vec<u8>), String>>,
    },
    UpdateContext {
        context: PeerContext,
        friends: Vec<User>,
        peer_id: PeerId,
        result: mpsc::SyncSender<Result<(), String>>,
    },
    Shutdown,
}

#[derive(Clone)]
pub struct P2pTransport {
    port: u16,
    commands: Arc<Mutex<Option<tokio_mpsc::Sender<Command>>>>,
    identity_verification_requests: Arc<Mutex<Vec<PendingIdentityVerification>>>,
}

#[derive(Clone, Debug)]
pub struct PendingIdentityVerification {
    pub(crate) friend: User,
    pub(crate) request_id: String,
}

impl P2pTransport {
    pub fn new(port: u16) -> Self {
        Self {
            port,
            commands: Arc::new(Mutex::new(None)),
            identity_verification_requests: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub(crate) fn configure(&self, context: &PeerContext) -> Result<(), String> {
        let keypair = libp2p_identity(context)?;
        let local_peer_id = keypair.public().to_peer_id();
        let friends = configured_peers(context)?;
        let existing_commands = self
            .commands
            .lock()
            .map_err(|_| "libp2p networking is unavailable.".to_string())?
            .clone();
        if let Some(commands) = existing_commands {
            let (result_sender, result_receiver) = mpsc::sync_channel(1);
            match commands.blocking_send(Command::UpdateContext {
                context: context.clone(),
                friends: friends.clone(),
                peer_id: local_peer_id,
                result: result_sender,
            }) {
                Ok(()) => {
                    return result_receiver
                        .recv_timeout(Duration::from_secs(10))
                        .map_err(|error| {
                            format!("libp2p configuration update timed out: {error}")
                        })?;
                }
                Err(_) => {
                    *self
                        .commands
                        .lock()
                        .map_err(|_| "libp2p networking is unavailable.".to_string())? = None;
                }
            }
        }
        let context = context.clone();
        let relay_addresses = configured_relays()?;
        let (commands, receiver) = tokio_mpsc::channel(64);
        let (ready_sender, ready_receiver) = mpsc::sync_channel(1);
        let port = self.port;
        let identity_verification_requests = self.identity_verification_requests.clone();
        thread::Builder::new()
            .name("resonance-libp2p".into())
            .spawn(move || {
                let runtime = RuntimeBuilder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| format!("Could not start the libp2p runtime: {error}"));
                match runtime {
                    Ok(runtime) => runtime.block_on(async move {
                        let setup = (|| {
                            let mut swarm = build_swarm(keypair, port)?;
                            let listen_address: Multiaddr =
                                format!("/ip4/0.0.0.0/udp/{port}/quic-v1").parse().map_err(
                                    |error| format!("Invalid QUIC listen address: {error}"),
                                )?;
                            swarm.listen_on(listen_address).map_err(|error| {
                                format!("Could not listen for libp2p QUIC peers: {error:?}")
                            })?;
                            Ok::<_, String>(swarm)
                        })();
                        match setup {
                            Ok(mut swarm) => {
                                let _ = ready_sender.send(Ok(()));
                                run_swarm(
                                    &mut swarm,
                                    receiver,
                                    context,
                                    friends,
                                    relay_addresses,
                                    local_peer_id,
                                    identity_verification_requests,
                                )
                                .await;
                            }
                            Err(error) => {
                                let _ = ready_sender.send(Err(error));
                            }
                        }
                    }),
                    Err(error) => {
                        let _ = ready_sender.send(Err(error));
                    }
                }
            })
            .map_err(|error| format!("Could not start libp2p networking: {error}"))?;

        match ready_receiver.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => {
                *self
                    .commands
                    .lock()
                    .map_err(|_| "libp2p networking is unavailable.".to_string())? = Some(commands);
                Ok(())
            }
            Ok(Err(error)) => Err(error),
            Err(error) => Err(format!("libp2p did not start: {error}")),
        }
    }

    pub(crate) fn clear(&self) {
        if let Ok(mut commands) = self.commands.lock() {
            if let Some(commands) = commands.take() {
                let _ = commands.blocking_send(Command::Shutdown);
            }
        }
    }

    pub(crate) fn send(
        &self,
        peer: &User,
        message: &Message,
        context: &PeerContext,
    ) -> Result<(), String> {
        let conversation_is_group = context.store.conversations()?.iter().any(|conversation| {
            conversation.id == message.conversation_id && conversation.is_group()
        });
        let envelope = PeerEnvelope {
            id: message.id.clone(),
            conversation_id: message.conversation_id.clone(),
            author_id: context.account_id.clone(),
            author_name: context.display_name.clone(),
            body: message.encrypted_payload.clone().ok_or_else(|| {
                "The message has not been encrypted for peer delivery.".to_string()
            })?,
            time: message.sent_at.clone(),
            is_group: conversation_is_group,
            attachments: Vec::new(),
        };
        validate_peer_message(&envelope)?;
        let commands = self
            .commands
            .lock()
            .map_err(|_| "libp2p networking is unavailable.".to_string())?
            .clone()
            .ok_or_else(|| "Sign in before sending a peer message.".to_string())?;
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        commands
            .blocking_send(Command::Send {
                peer: peer.clone(),
                envelope,
                result: result_sender,
            })
            .map_err(|_| "libp2p networking is unavailable.".to_string())?;
        result_receiver
            .recv_timeout(Duration::from_secs(25))
            .map_err(|error| format!("Peer message delivery timed out: {error}"))?
    }

    pub(crate) fn get_attachment_chunk(
        &self,
        peer: &User,
        conversation_id: &str,
        attachment_id: &str,
        offset: u64,
        verified: bool,
    ) -> Result<(u64, u64, Vec<u8>), String> {
        let commands = self
            .commands
            .lock()
            .map_err(|_| "libp2p networking is unavailable.".to_string())?
            .clone()
            .ok_or_else(|| "Sign in before downloading a peer attachment.".to_string())?;
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        commands
            .blocking_send(Command::DownloadChunk {
                peer: peer.clone(),
                attachment_id: attachment_id.to_string(),
                conversation_id: conversation_id.to_string(),
                offset,
                verified,
                result: result_sender,
            })
            .map_err(|_| "libp2p networking is unavailable.".to_string())?;
        result_receiver
            .recv_timeout(Duration::from_secs(35))
            .map_err(|error| format!("Peer attachment transfer timed out: {error}"))?
    }

    pub(crate) fn request_identity_verification(&self, peer: &User) -> Result<String, String> {
        self.send_identity_verification_action(peer, new_id()?, IdentityVerificationAction::Request)
            .map(|request_id| request_id)
    }

    pub(crate) fn respond_to_identity_verification(
        &self,
        peer: &User,
        request_id: &str,
        accepted: bool,
    ) -> Result<(), String> {
        let action = if accepted {
            IdentityVerificationAction::Accept
        } else {
            IdentityVerificationAction::Reject
        };
        self.send_identity_verification_action(peer, request_id.to_string(), action)
            .map(|_| ())
    }

    pub(crate) fn take_identity_verification_requests(
        &self,
    ) -> Result<Vec<PendingIdentityVerification>, String> {
        let mut requests = self
            .identity_verification_requests
            .lock()
            .map_err(|_| "Peer verification requests are unavailable.".to_string())?;
        Ok(std::mem::take(&mut *requests))
    }

    fn send_identity_verification_action(
        &self,
        peer: &User,
        request_id: String,
        action: IdentityVerificationAction,
    ) -> Result<String, String> {
        if request_id.is_empty()
            || request_id.len() > 128
            || request_id.chars().any(char::is_control)
        {
            return Err("The identity verification request is invalid.".into());
        }
        let commands = self
            .commands
            .lock()
            .map_err(|_| "libp2p networking is unavailable.".to_string())?
            .clone()
            .ok_or_else(|| "Sign in before verifying a contact identity.".to_string())?;
        let (result_sender, result_receiver) = mpsc::sync_channel(1);
        commands
            .blocking_send(Command::IdentityVerification {
                peer: peer.clone(),
                request_id: request_id.clone(),
                action,
                result: result_sender,
            })
            .map_err(|_| "libp2p networking is unavailable.".to_string())?;
        result_receiver
            .recv_timeout(Duration::from_secs(25))
            .map_err(|error| format!("Identity verification request timed out: {error}"))??;
        Ok(request_id)
    }

    pub(crate) fn acknowledge_attachment(
        &self,
        peer: &User,
        conversation_id: &str,
        attachment_id: &str,
    ) -> Result<(), String> {
        let (offset, total_size, bytes) =
            self.get_attachment_chunk(peer, conversation_id, attachment_id, 0, true)?;
        if offset != 0 || total_size != 0 || !bytes.is_empty() {
            return Err("The peer returned an invalid attachment confirmation.".into());
        }
        Ok(())
    }
}

impl Drop for P2pTransport {
    fn drop(&mut self) {
        if Arc::strong_count(&self.commands) == 1 {
            self.clear();
        }
    }
}

fn build_swarm(
    keypair: identity::Keypair,
    _port: u16,
) -> Result<libp2p::Swarm<P2pBehaviour>, String> {
    let peer_id = keypair.public().to_peer_id();
    SwarmBuilder::with_existing_identity(keypair.clone())
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            (tls::Config::new, noise::Config::new),
            yamux::Config::default,
        )
        .map_err(|error| format!("Could not initialize libp2p TCP transport: {error}"))?
        .with_quic()
        .with_relay_client(
            (tls::Config::new, noise::Config::new),
            yamux::Config::default,
        )
        .map_err(|error| format!("Could not initialize the libp2p relay client: {error}"))?
        .with_behaviour(|key, relay| P2pBehaviour {
            requests: request_response::cbor::Behaviour::new(
                [(
                    libp2p::StreamProtocol::new(MESSAGE_PROTOCOL),
                    request_response::ProtocolSupport::Full,
                )],
                request_response::Config::default().with_request_timeout(Duration::from_secs(20)),
            ),
            identity_verifications: request_response::cbor::Behaviour::new(
                [(
                    libp2p::StreamProtocol::new(IDENTITY_VERIFICATION_PROTOCOL),
                    request_response::ProtocolSupport::Full,
                )],
                request_response::Config::default().with_request_timeout(Duration::from_secs(20)),
            ),
            files: request_response::cbor::Behaviour::new(
                [(
                    libp2p::StreamProtocol::new(FILE_PROTOCOL),
                    request_response::ProtocolSupport::Full,
                )],
                request_response::Config::default().with_request_timeout(Duration::from_secs(30)),
            ),
            autonat: autonat::Behaviour::new(peer_id, autonat::Config::default()),
            dcutr: dcutr::Behaviour::new(peer_id),
            identify: identify::Behaviour::new(
                identify::Config::new("/resonance/1.0.0".into(), key.public())
                    .with_push_listen_addr_updates(true),
            ),
            relay,
        })
        .map_err(|error| format!("Could not initialize libp2p behaviours: {error}"))
        .map(|builder| builder.build())
}

async fn run_swarm(
    swarm: &mut libp2p::Swarm<P2pBehaviour>,
    mut commands: tokio_mpsc::Receiver<Command>,
    mut context: PeerContext,
    friends: Vec<User>,
    relay_addresses: Vec<Multiaddr>,
    local_peer_id: PeerId,
    identity_verification_requests: Arc<Mutex<Vec<PendingIdentityVerification>>>,
) {
    let mut peers = HashMap::new();
    update_peer_addresses(swarm, &mut peers, friends);

    for relay_address in &relay_addresses {
        if let Err(error) = swarm.dial(relay_address.clone()) {
            eprintln!("Could not connect to configured libp2p relay {relay_address}: {error}");
            continue;
        }
        let reservation = relay_address.clone().with(Protocol::P2pCircuit);
        if let Err(error) = swarm.listen_on(reservation) {
            eprintln!("Could not reserve a libp2p relay circuit: {error}");
        }
    }

    let mut pending = HashMap::new();
    let mut pending_verification_commands = HashMap::new();
    let mut pending_outgoing_verifications = HashMap::new();
    let mut pending_incoming_verifications = HashMap::new();
    let mut pending_files = HashMap::new();
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Some(Command::Send { peer, envelope, result }) => {
                    if pending.len() >= MAX_PENDING_REQUESTS {
                        let _ = result.send(Err("Too many peer messages are awaiting acknowledgement.".into()));
                        continue;
                    }
                    let peer_id = match peer_id_for_user(&peer) {
                        Ok(peer_id) => peer_id,
                        Err(error) => {
                            let _ = result.send(Err(error));
                            continue;
                        }
                    };
                    let mut addresses = Vec::new();
                    if let Some(endpoint) = peer.endpoint.as_deref() {
                        match direct_address(endpoint) {
                            Ok(address) => addresses.push(address),
                            Err(error) => {
                                let _ = result.send(Err(error));
                                continue;
                            }
                        }
                    }
                    for relay_address in &relay_addresses {
                        addresses.push(relay_address.clone().with(Protocol::P2pCircuit).with(Protocol::P2p(peer_id)));
                    }
                    if addresses.is_empty() {
                        let _ = result.send(Err(format!("@{} has no published P2P endpoint or configured relay.", peer.username)));
                        continue;
                    }
                    let request_id = swarm.behaviour_mut().requests.send_request_with_addresses(
                        &peer_id,
                        MessageRequest { envelope },
                        addresses,
                    );
                    pending.insert(request_id, result);
                }
                Some(Command::IdentityVerification { peer, request_id, action, result }) => {
                    if pending_verification_commands.len() >= MAX_PENDING_REQUESTS {
                        let _ = result.send(Err("Too many identity verification requests are awaiting responses.".into()));
                        continue;
                    }
                    let peer_id = match peer_id_for_user(&peer) {
                        Ok(peer_id) => peer_id,
                        Err(error) => {
                            let _ = result.send(Err(error));
                            continue;
                        }
                    };
                    if !context.friends.contains_key(&peer.id) {
                        let _ = result.send(Err("The participant is no longer in the local friend list.".into()));
                        continue;
                    }
                    let key = (peer_id, request_id.clone());
                    match action {
                        IdentityVerificationAction::Request => {
                            if pending_outgoing_verifications.len() >= MAX_PENDING_REQUESTS {
                                let _ = result.send(Err("Too many identity verification requests are pending.".into()));
                                continue;
                            }
                            if pending_outgoing_verifications.contains_key(&key) {
                                let _ = result.send(Err("A verification request is already pending for this friend.".into()));
                                continue;
                            }
                            pending_outgoing_verifications.insert(key.clone(), peer.clone());
                        }
                        IdentityVerificationAction::Accept | IdentityVerificationAction::Reject => {
                            if !pending_incoming_verifications.contains_key(&key) {
                                let _ = result.send(Err("This identity verification request is no longer pending.".into()));
                                continue;
                            }
                        }
                    }
                    let mut addresses = Vec::new();
                    if let Some(endpoint) = peer.endpoint.as_deref() {
                        match direct_address(endpoint) {
                            Ok(address) => addresses.push(address),
                            Err(error) => {
                                pending_outgoing_verifications.remove(&key);
                                let _ = result.send(Err(error));
                                continue;
                            }
                        }
                    }
                    for relay_address in &relay_addresses {
                        addresses.push(relay_address.clone().with(Protocol::P2pCircuit).with(Protocol::P2p(peer_id)));
                    }
                    if addresses.is_empty() {
                        pending_outgoing_verifications.remove(&key);
                        let _ = result.send(Err(format!("@{} has no published P2P endpoint or configured relay.", peer.username)));
                        continue;
                    }
                    let request = IdentityVerificationRequest { request_id: request_id.clone(), action };
                    let outbound_id = swarm.behaviour_mut().identity_verifications.send_request_with_addresses(
                        &peer_id,
                        request,
                        addresses,
                    );
                    pending_verification_commands.insert(
                        outbound_id,
                        (result, peer, request_id, action, peer_id),
                    );
                }
                Some(Command::DownloadChunk { peer, attachment_id, conversation_id, offset, verified, result }) => {
                    if pending_files.len() >= MAX_PENDING_REQUESTS {
                        let _ = result.send(Err("Too many peer attachment requests are awaiting responses.".into()));
                        continue;
                    }
                    let peer_id = match peer_id_for_user(&peer) {
                        Ok(peer_id) => peer_id,
                        Err(error) => {
                            let _ = result.send(Err(error));
                            continue;
                        }
                    };
                    let mut addresses = Vec::new();
                    if let Some(endpoint) = peer.endpoint.as_deref() {
                        match direct_address(endpoint) {
                            Ok(address) => addresses.push(address),
                            Err(error) => {
                                let _ = result.send(Err(error));
                                continue;
                            }
                        }
                    }
                    for relay_address in &relay_addresses {
                        addresses.push(relay_address.clone().with(Protocol::P2pCircuit).with(Protocol::P2p(peer_id)));
                    }
                    if addresses.is_empty() {
                        let _ = result.send(Err(format!("@{} has no published P2P endpoint or configured relay.", peer.username)));
                        continue;
                    }
                    let request_id = swarm.behaviour_mut().files.send_request_with_addresses(
                        &peer_id,
                        FileChunkRequest {
                            attachment_id,
                            conversation_id,
                            offset,
                            requested_size: if verified { 0 } else { FILE_CHUNK_SIZE as u32 },
                            verified,
                        },
                        addresses,
                    );
                    pending_files.insert(request_id, result);
                }
                Some(Command::UpdateContext { context: updated_context, friends, peer_id, result }) => {
                    if updated_context.account_id != context.account_id || peer_id != local_peer_id {
                        let _ = result.send(Err("Clear peer networking before switching accounts.".into()));
                        continue;
                    }
                    update_peer_addresses(swarm, &mut peers, friends);
                    context = updated_context;
                    let _ = result.send(Ok(()));
                }
                Some(Command::Shutdown) | None => break,
            },
            event = swarm.select_next_some() => match event {
                SwarmEvent::Behaviour(P2pBehaviourEvent::Requests(event)) => match event {
                    request_response::Event::Message { peer, message, .. } => match message {
                        request_response::Message::Request { request, channel, .. } => {
                            let response = receive_message(&context, &peers, peer, request);
                            if let Err(error) = swarm.behaviour_mut().requests.send_response(channel, response) {
                                eprintln!("Could not respond to a libp2p message request: {error:?}");
                            }
                        }
                        request_response::Message::Response { request_id, response } => {
                            if let Some(result) = pending.remove(&request_id) {
                                let response = if response.accepted {
                                    Ok(())
                                } else {
                                    Err(response.error.unwrap_or_else(|| "The peer rejected the message.".into()))
                                };
                                let _ = result.send(response);
                            }
                        }
                    },
                    request_response::Event::OutboundFailure { request_id, error, .. } => {
                        if let Some(result) = pending.remove(&request_id) {
                            let _ = result.send(Err(format!("libp2p message delivery failed: {error}")));
                        }
                    }
                    request_response::Event::InboundFailure { peer, error, .. } => {
                        eprintln!("libp2p request from {peer} failed: {error}");
                    }
                    request_response::Event::ResponseSent { .. } => {}
                },
                SwarmEvent::Behaviour(P2pBehaviourEvent::IdentityVerifications(event)) => match event {
                    request_response::Event::Message { peer, message, .. } => match message {
                        request_response::Message::Request { request, channel, .. } => {
                            let response = receive_identity_verification(
                                &context,
                                &peers,
                                peer,
                                request,
                                &mut pending_outgoing_verifications,
                                &mut pending_incoming_verifications,
                                &identity_verification_requests,
                            );
                            if let Err(error) = swarm
                                .behaviour_mut()
                                .identity_verifications
                                .send_response(channel, response)
                            {
                                eprintln!("Could not respond to an identity verification request: {error:?}");
                            }
                        }
                        request_response::Message::Response { request_id, response } => {
                            if let Some((result, friend, verification_id, action, peer_id)) =
                                pending_verification_commands.remove(&request_id)
                            {
                                let response_result = complete_identity_verification_response(
                                    &context,
                                    &friend,
                                    &verification_id,
                                    action,
                                    peer_id,
                                    response,
                                    &mut pending_outgoing_verifications,
                                    &mut pending_incoming_verifications,
                                    &identity_verification_requests,
                                );
                                let _ = result.send(response_result);
                            }
                        }
                    },
                    request_response::Event::OutboundFailure { request_id, error, .. } => {
                        if let Some((result, _, verification_id, action, peer_id)) =
                            pending_verification_commands.remove(&request_id)
                        {
                            if action == IdentityVerificationAction::Request {
                                pending_outgoing_verifications
                                    .remove(&(peer_id, verification_id));
                            }
                            let _ = result.send(Err(format!(
                                "Identity verification delivery failed: {error}"
                            )));
                        }
                    }
                    request_response::Event::InboundFailure { peer, error, .. } => {
                        eprintln!("Identity verification request from {peer} failed: {error}");
                    }
                    request_response::Event::ResponseSent { .. } => {}
                },
                SwarmEvent::Behaviour(P2pBehaviourEvent::Files(event)) => match event {
                    request_response::Event::Message { peer, message, .. } => match message {
                        request_response::Message::Request { request, channel, .. } => {
                            let response = receive_file_chunk(&context, &peers, peer, request);
                            if let Err(error) = swarm.behaviour_mut().files.send_response(channel, response) {
                                eprintln!("Could not respond to a libp2p attachment request: {error:?}");
                            }
                        }
                        request_response::Message::Response { request_id, response } => {
                            if let Some(result) = pending_files.remove(&request_id) {
                                let response = match response.error {
                                    Some(error) => Err(error),
                                    None => Ok((response.offset, response.total_size, response.bytes)),
                                };
                                let _ = result.send(response);
                            }
                        }
                    },
                    request_response::Event::OutboundFailure { request_id, error, .. } => {
                        if let Some(result) = pending_files.remove(&request_id) {
                            let _ = result.send(Err(format!("libp2p attachment transfer failed: {error}")));
                        }
                    }
                    request_response::Event::InboundFailure { peer, error, .. } => {
                        eprintln!("libp2p attachment request from {peer} failed: {error}");
                    }
                    request_response::Event::ResponseSent { .. } => {}
                },
                SwarmEvent::Behaviour(P2pBehaviourEvent::Identify(event)) => {
                    if let identify::Event::Received { peer_id, info, .. } = event {
                        if peers.contains_key(&peer_id) {
                            for address in info.listen_addrs {
                                swarm.add_peer_address(peer_id, address);
                            }
                        }
                    }
                }
                SwarmEvent::Behaviour(P2pBehaviourEvent::Dcutr(event)) => {
                    if let dcutr::Event { result: Err(error), remote_peer_id, .. } = event {
                        eprintln!("DCUtR failed for {remote_peer_id}: {error}");
                    }
                }
                SwarmEvent::Behaviour(P2pBehaviourEvent::Autonat(event)) => {
                    match event {
                        autonat::Event::StatusChanged { new, .. } => {
                            eprintln!("libp2p AutoNAT reachability changed: {new:?}");
                        }
                        event => eprintln!("libp2p AutoNAT event: {event:?}"),
                    }
                }
                SwarmEvent::Behaviour(P2pBehaviourEvent::Relay(event)) => {
                    match event {
                        relay::client::Event::ReservationReqAccepted { relay_peer_id, .. } => {
                            eprintln!("Circuit Relay v2 reservation accepted by {relay_peer_id}");
                        }
                        relay::client::Event::OutboundCircuitEstablished { relay_peer_id, .. } => {
                            eprintln!("Established a relayed P2P connection via {relay_peer_id}");
                        }
                        relay::client::Event::InboundCircuitEstablished { src_peer_id, .. } => {
                            eprintln!("Accepted a relayed P2P connection from {src_peer_id}");
                        }
                    }
                }
                SwarmEvent::NewListenAddr { address, .. } => {
                    eprintln!("libp2p is listening on {address}");
                }
                SwarmEvent::ListenerError { error, .. } => {
                    eprintln!("libp2p listener error: {error}");
                }
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    if peer_id != local_peer_id && !peers.contains_key(&peer_id) {
                        eprintln!("Ignoring connection from an unknown peer: {peer_id}");
                    }
                }
                SwarmEvent::OutgoingConnectionError { peer_id, error, .. } => {
                    eprintln!("libp2p connection to {peer_id:?} failed: {error}");
                }
                SwarmEvent::IncomingConnectionError { error, .. } => {
                    eprintln!("Incoming libp2p connection failed: {error}");
                }
                _ => {}
            }
        }
    }

    for (_, result) in pending {
        let _ = result.send(Err(
            "libp2p networking stopped before delivery completed.".into()
        ));
    }
    for (_, (result, _, verification_id, action, peer_id)) in pending_verification_commands {
        if action == IdentityVerificationAction::Request {
            pending_outgoing_verifications.remove(&(peer_id, verification_id));
        }
        let _ = result.send(Err(
            "libp2p networking stopped before identity verification completed.".into(),
        ));
    }
    for (_, result) in pending_files {
        let _ = result.send(Err(
            "libp2p networking stopped during attachment transfer.".into()
        ));
    }
}

fn configured_peers(context: &PeerContext) -> Result<Vec<User>, String> {
    let mut peers = context.friends.clone();
    for conversation in context.store.conversations()? {
        if conversation.is_group() {
            for member in conversation.members {
                peers.entry(member.id.clone()).or_insert(member);
            }
        }
    }
    Ok(peers.into_values().collect())
}

fn update_peer_addresses(
    swarm: &mut libp2p::Swarm<P2pBehaviour>,
    peers: &mut HashMap<PeerId, User>,
    friends: Vec<User>,
) {
    peers.clear();
    for friend in friends {
        match peer_id_for_user(&friend) {
            Ok(peer_id) => {
                peers.insert(peer_id, friend.clone());
                if let Some(endpoint) = friend.endpoint.as_deref() {
                    if let Ok(address) = direct_address(endpoint) {
                        swarm.add_peer_address(peer_id, address);
                    }
                }
            }
            Err(error) => eprintln!("Skipping friend with invalid P2P identity: {error}"),
        }
    }
}

fn receive_message(
    context: &PeerContext,
    peers: &HashMap<PeerId, User>,
    peer_id: PeerId,
    request: MessageRequest,
) -> MessageResponse {
    let result = peers
        .get(&peer_id)
        .ok_or_else(|| "The sender is not in the local friend list.".to_string())
        .and_then(|friend| receive_chat_message(context, friend, request.envelope));
    match result {
        Ok(()) => MessageResponse {
            accepted: true,
            error: None,
        },
        Err(error) => MessageResponse {
            accepted: false,
            error: Some(error),
        },
    }
}

fn receive_identity_verification(
    context: &PeerContext,
    peers: &HashMap<PeerId, User>,
    peer_id: PeerId,
    request: IdentityVerificationRequest,
    pending_outgoing_verifications: &mut HashMap<(PeerId, String), User>,
    pending_incoming_verifications: &mut HashMap<(PeerId, String), User>,
    identity_verification_requests: &Arc<Mutex<Vec<PendingIdentityVerification>>>,
) -> MessageResponse {
    let result = (|| {
        let friend = peers
            .get(&peer_id)
            .ok_or_else(|| "The sender is not in the local friend list.".to_string())?;
        let request_id = request.request_id;
        if request_id.is_empty()
            || request_id.len() > 128
            || request_id.chars().any(char::is_control)
            || !context.friends.contains_key(&friend.id)
        {
            return Err("The identity verification request is invalid.".into());
        }
        let key = (peer_id, request_id.clone());
        match request.action {
            IdentityVerificationAction::Request => {
                if !pending_incoming_verifications.contains_key(&key) {
                    if pending_incoming_verifications.len() >= MAX_PENDING_REQUESTS {
                        return Err("Too many identity verification requests are pending.".into());
                    }
                    let mut requests = identity_verification_requests
                        .lock()
                        .map_err(|_| "Peer verification requests are unavailable.".to_string())?;
                    if !requests.iter().any(|request| {
                        request.request_id == request_id && request.friend.id == friend.id
                    }) {
                        requests.push(PendingIdentityVerification {
                            friend: friend.clone(),
                            request_id,
                        });
                    }
                    pending_incoming_verifications.insert(key, friend.clone());
                }
                Ok(())
            }
            IdentityVerificationAction::Accept => {
                if !pending_outgoing_verifications.contains_key(&key) {
                    return Err("This identity verification request is no longer pending.".into());
                }
                context
                    .store
                    .verify_contact_identity_after_live_acceptance(friend)?;
                pending_outgoing_verifications.remove(&key);
                Ok(())
            }
            IdentityVerificationAction::Reject => {
                if pending_outgoing_verifications.remove(&key).is_none() {
                    return Err("This identity verification request is no longer pending.".into());
                }
                Ok(())
            }
        }
    })();

    match result {
        Ok(()) => MessageResponse {
            accepted: true,
            error: None,
        },
        Err(error) => MessageResponse {
            accepted: false,
            error: Some(error),
        },
    }
}

fn complete_identity_verification_response(
    context: &PeerContext,
    friend: &User,
    request_id: &str,
    action: IdentityVerificationAction,
    peer_id: PeerId,
    response: MessageResponse,
    pending_outgoing_verifications: &mut HashMap<(PeerId, String), User>,
    pending_incoming_verifications: &mut HashMap<(PeerId, String), User>,
    identity_verification_requests: &Arc<Mutex<Vec<PendingIdentityVerification>>>,
) -> Result<(), String> {
    let key = (peer_id, request_id.to_string());
    if !response.accepted {
        if action == IdentityVerificationAction::Request {
            pending_outgoing_verifications.remove(&key);
        }
        return Err(response
            .error
            .unwrap_or_else(|| "The peer rejected the identity verification request.".into()));
    }

    match action {
        IdentityVerificationAction::Request => Ok(()),
        IdentityVerificationAction::Accept => {
            let mut requests = identity_verification_requests
                .lock()
                .map_err(|_| "Peer verification requests are unavailable.".to_string())?;
            context
                .store
                .verify_contact_identity_after_live_acceptance(friend)?;
            pending_incoming_verifications.remove(&key);
            requests.retain(|request| {
                request.request_id != request_id || request.friend.id != friend.id
            });
            Ok(())
        }
        IdentityVerificationAction::Reject => {
            let mut requests = identity_verification_requests
                .lock()
                .map_err(|_| "Peer verification requests are unavailable.".to_string())?;
            pending_incoming_verifications.remove(&key);
            requests.retain(|request| {
                request.request_id != request_id || request.friend.id != friend.id
            });
            Ok(())
        }
    }
}

fn receive_chat_message(
    context: &PeerContext,
    friend: &User,
    envelope: PeerEnvelope,
) -> Result<(), String> {
    if !envelope.is_group && !context.friends.contains_key(&friend.id) {
        return Err("Direct messages are only accepted from local friends.".into());
    }
    if envelope.author_id != friend.id || envelope.author_name.is_empty() {
        return Err("The message identity does not match the authenticated libp2p peer.".into());
    }
    validate_peer_message(&envelope)?;
    let group = context
        .store
        .conversations()?
        .into_iter()
        .find(|conversation| {
            conversation.id == envelope.conversation_id && conversation.is_group()
        });
    if envelope.is_group != group.is_some() {
        return Err("The message group does not match local membership.".into());
    }
    if let Some(group) = group.as_ref() {
        if !group
            .members
            .iter()
            .any(|member| member.id == context.account_id)
            || !group.members.iter().any(|member| member.id == friend.id)
        {
            return Err("The sender is not a member of this group.".into());
        }
    }
    if context
        .store
        .messages()?
        .iter()
        .any(|message| message.id == envelope.id)
    {
        return Ok(());
    }
    let mls_group_id = match group.as_ref() {
        Some(group) => group.id.clone(),
        None => MlsService::direct_group_id(&context.account_id, &friend.id)?,
    };
    let encoded_message = envelope
        .body
        .strip_prefix(MLS_MESSAGE_PREFIX)
        .ok_or_else(|| "The peer sent an unencrypted application message.".to_string())?;
    let Some((sender_id, plaintext)) = MlsService::decrypt(
        &context.store,
        &context.account_id,
        context.private_key.as_slice(),
        &mls_group_id,
        encoded_message,
    )?
    else {
        return Ok(());
    };
    if sender_id != friend.id {
        return Err("The MLS message identity does not match the authenticated peer.".into());
    }
    let plaintext = String::from_utf8(plaintext)
        .map_err(|_| "The encrypted message body is not valid UTF-8.".to_string())?;
    let (body, attachments) = match attachment::decode_message_payload(&plaintext)? {
        Some(content) => content,
        None => (plaintext, Vec::new()),
    };
    let mut message = Message::new(
        &envelope.id,
        &envelope.conversation_id,
        &envelope.author_id,
        &friend.name,
        &body,
        &envelope.time,
        false,
    );
    message.delivery_state = "received_direct".into();
    message.attachments = attachments;
    if let Some(group) = group {
        context
            .store
            .add_group_message(message, &group.id, &group.name, &group.members)?;
    } else {
        message.conversation_id = context.store.conversation_for(friend)?.id;
        context.store.add_message(message, friend)?;
    }
    Ok(())
}

fn receive_file_chunk(
    context: &PeerContext,
    peers: &HashMap<PeerId, User>,
    peer_id: PeerId,
    request: FileChunkRequest,
) -> FileChunkResponse {
    let result = (|| {
        let requester_id = if let Some(friend) = peers.get(&peer_id) {
            friend.id.clone()
        } else {
            context
                .store
                .conversations()?
                .into_iter()
                .find(|conversation| {
                    conversation.id == request.conversation_id && conversation.is_group()
                })
                .and_then(|conversation| {
                    conversation.members.into_iter().find_map(|member| {
                        (peer_id_for_user(&member).ok() == Some(peer_id)).then_some(member.id)
                    })
                })
                .ok_or_else(|| {
                    "The requester is not a participant in this local conversation.".to_string()
                })?
        };
        if !context.store.can_serve_attachment(
            &request.attachment_id,
            &requester_id,
            &request.conversation_id,
        )? {
            return Err(
                "The attachment was not shared in this conversation with this peer.".into(),
            );
        }
        if request.verified {
            if request.offset != 0 || request.requested_size != 0 {
                return Err("The attachment confirmation request is invalid.".into());
            }
            context.store.record_attachment_transfer_receipt(
                &request.attachment_id,
                &requester_id,
                &request.conversation_id,
            )?;
            return Ok((0, Vec::new()));
        }
        let (total_size, bytes) = context.store.attachment_ciphertext_chunk(
            &request.attachment_id,
            request.offset,
            request.requested_size as usize,
        )?;
        context.store.record_attachment_chunk_served(
            &request.attachment_id,
            &requester_id,
            request.offset,
            total_size,
            bytes.len(),
        )?;
        Ok((total_size, bytes))
    })();
    match result {
        Ok((total_size, bytes)) => FileChunkResponse {
            offset: request.offset,
            total_size,
            bytes,
            error: None,
        },
        Err(error) => FileChunkResponse {
            offset: request.offset,
            total_size: 0,
            bytes: Vec::new(),
            error: Some(error),
        },
    }
}

fn libp2p_identity(context: &PeerContext) -> Result<identity::Keypair, String> {
    identity_from_pkcs8(&context.private_key, &context.account_id)
}

fn identity_from_pkcs8(private_key: &[u8], account_id: &str) -> Result<identity::Keypair, String> {
    let signing_key = SigningKey::from_pkcs8_der(private_key).map_err(|_| {
        "The unlocked identity key cannot authenticate libp2p connections.".to_string()
    })?;
    let public_bytes = signing_key.verifying_key().to_bytes();
    let expected_public = decode_hex(account_id)
        .ok_or_else(|| "The account identity public key is invalid.".to_string())?;
    if public_bytes.as_slice() != expected_public {
        return Err("The libp2p key does not match the account identity.".into());
    }
    let mut keypair_bytes = signing_key.to_keypair_bytes();
    let keypair = identity::ed25519::Keypair::try_from_bytes(&mut keypair_bytes)
        .map_err(|_| "Could not import the persistent identity into libp2p.".to_string())?;
    Ok(identity::Keypair::from(keypair))
}

fn peer_id_for_user(user: &User) -> Result<PeerId, String> {
    let public_key = decode_hex(&user.id)
        .ok_or_else(|| format!("@{} has an invalid public identity.", user.username))?;
    let public_key = identity::ed25519::PublicKey::try_from_bytes(&public_key)
        .map_err(|_| format!("@{} has an invalid Ed25519 public key.", user.username))?;
    Ok(identity::PublicKey::from(public_key).to_peer_id())
}

fn direct_address(endpoint: &str) -> Result<Multiaddr, String> {
    let socket = endpoint
        .parse::<SocketAddr>()
        .map_err(|_| "The published peer endpoint is invalid.".to_string())?;
    match socket.ip() {
        IpAddr::V4(ip) => format!("/ip4/{ip}/udp/{}/quic-v1", socket.port()),
        IpAddr::V6(ip) => format!("/ip6/{ip}/udp/{}/quic-v1", socket.port()),
    }
    .parse()
    .map_err(|_| "Could not construct a QUIC peer address.".to_string())
}

fn configured_relays() -> Result<Vec<Multiaddr>, String> {
    let Some(value) = std::env::var_os("RESONANCE_RELAY_ADDRESSES") else {
        return Ok(Vec::new());
    };
    value
        .to_string_lossy()
        .split(',')
        .map(|address| {
            address
                .trim()
                .parse::<Multiaddr>()
                .map_err(|_| format!("Invalid libp2p relay multiaddress: {address}"))
        })
        .collect()
}

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    hex.as_bytes()
        .chunks(2)
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
    use crate::{
        models::message::Message,
        services::{
            attachment::{self, AttachmentKind, AttachmentMetadata},
            identity::IdentityService,
            local_chat_store::{LocalChatStore, new_id},
        },
    };
    use std::sync::Arc;
    use zeroize::Zeroizing;

    #[test]
    fn accepted_live_verification_trusts_the_authenticated_keys_on_both_devices() {
        let (alice, _) = IdentityService::new()
            .create("live-verify-alice", "Alice", "a sufficiently long password")
            .unwrap();
        let (bob, _) = IdentityService::new()
            .create("live-verify-bob", "Bob", "a sufficiently long password")
            .unwrap();
        let alice_store = LocalChatStore::in_memory();
        let bob_store = LocalChatStore::in_memory();
        alice_store
            .sync_friends(std::slice::from_ref(&bob.user))
            .unwrap();
        bob_store
            .sync_friends(std::slice::from_ref(&alice.user))
            .unwrap();
        let alice_context = PeerContext {
            account_id: alice.user.id.clone(),
            display_name: alice.user.name.clone(),
            private_key: Arc::new(Zeroizing::new(alice.private_key().to_vec())),
            friends: HashMap::from([(bob.user.id.clone(), bob.user.clone())]),
            store: alice_store.clone(),
        };
        let bob_context = PeerContext {
            account_id: bob.user.id.clone(),
            display_name: bob.user.name.clone(),
            private_key: Arc::new(Zeroizing::new(bob.private_key().to_vec())),
            friends: HashMap::from([(alice.user.id.clone(), alice.user.clone())]),
            store: bob_store.clone(),
        };
        let alice_peer_id = peer_id_for_user(&alice.user).unwrap();
        let bob_peer_id = peer_id_for_user(&bob.user).unwrap();
        let alice_peers = HashMap::from([(bob_peer_id, bob.user.clone())]);
        let bob_peers = HashMap::from([(alice_peer_id, alice.user.clone())]);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let request_id = "live-verification-1".to_string();
        let mut alice_pending_outgoing =
            HashMap::from([((bob_peer_id, request_id.clone()), bob.user.clone())]);
        let mut bob_pending_incoming = HashMap::new();

        let received = receive_identity_verification(
            &bob_context,
            &bob_peers,
            alice_peer_id,
            IdentityVerificationRequest {
                request_id: request_id.clone(),
                action: IdentityVerificationAction::Request,
            },
            &mut HashMap::new(),
            &mut bob_pending_incoming,
            &requests,
        );
        assert!(received.accepted);
        assert_eq!(requests.lock().unwrap().len(), 1);

        let accepted = receive_identity_verification(
            &alice_context,
            &alice_peers,
            bob_peer_id,
            IdentityVerificationRequest {
                request_id: request_id.clone(),
                action: IdentityVerificationAction::Accept,
            },
            &mut alice_pending_outgoing,
            &mut HashMap::new(),
            &requests,
        );
        assert!(accepted.accepted);
        assert_eq!(
            alice_store.contact_identity(&bob.user).unwrap().status,
            crate::services::local_chat_store::IdentityTrustStatus::Verified
        );

        complete_identity_verification_response(
            &bob_context,
            &alice.user,
            &request_id,
            IdentityVerificationAction::Accept,
            alice_peer_id,
            MessageResponse {
                accepted: true,
                error: None,
            },
            &mut HashMap::new(),
            &mut bob_pending_incoming,
            &requests,
        )
        .unwrap();
        assert_eq!(
            bob_store.contact_identity(&alice.user).unwrap().status,
            crate::services::local_chat_store::IdentityTrustStatus::Verified
        );
        assert!(requests.lock().unwrap().is_empty());
    }

    #[test]
    fn rejected_live_verification_does_not_trust_either_contact() {
        let (alice, _) = IdentityService::new()
            .create("live-reject-alice", "Alice", "a sufficiently long password")
            .unwrap();
        let (bob, _) = IdentityService::new()
            .create("live-reject-bob", "Bob", "a sufficiently long password")
            .unwrap();
        let alice_store = LocalChatStore::in_memory();
        let bob_store = LocalChatStore::in_memory();
        alice_store
            .sync_friends(std::slice::from_ref(&bob.user))
            .unwrap();
        bob_store
            .sync_friends(std::slice::from_ref(&alice.user))
            .unwrap();
        let alice_context = PeerContext {
            account_id: alice.user.id.clone(),
            display_name: alice.user.name.clone(),
            private_key: Arc::new(Zeroizing::new(alice.private_key().to_vec())),
            friends: HashMap::from([(bob.user.id.clone(), bob.user.clone())]),
            store: alice_store.clone(),
        };
        let bob_context = PeerContext {
            account_id: bob.user.id.clone(),
            display_name: bob.user.name.clone(),
            private_key: Arc::new(Zeroizing::new(bob.private_key().to_vec())),
            friends: HashMap::from([(alice.user.id.clone(), alice.user.clone())]),
            store: bob_store.clone(),
        };
        let alice_peer_id = peer_id_for_user(&alice.user).unwrap();
        let bob_peer_id = peer_id_for_user(&bob.user).unwrap();
        let alice_peers = HashMap::from([(bob_peer_id, bob.user.clone())]);
        let bob_peers = HashMap::from([(alice_peer_id, alice.user.clone())]);
        let requests = Arc::new(Mutex::new(Vec::new()));
        let request_id = "live-verification-reject".to_string();
        let mut alice_pending_outgoing =
            HashMap::from([((bob_peer_id, request_id.clone()), bob.user.clone())]);
        let mut bob_pending_incoming = HashMap::new();

        assert!(
            receive_identity_verification(
                &bob_context,
                &bob_peers,
                alice_peer_id,
                IdentityVerificationRequest {
                    request_id: request_id.clone(),
                    action: IdentityVerificationAction::Request,
                },
                &mut HashMap::new(),
                &mut bob_pending_incoming,
                &requests,
            )
            .accepted
        );
        assert!(
            receive_identity_verification(
                &alice_context,
                &alice_peers,
                bob_peer_id,
                IdentityVerificationRequest {
                    request_id: request_id.clone(),
                    action: IdentityVerificationAction::Reject,
                },
                &mut alice_pending_outgoing,
                &mut HashMap::new(),
                &requests,
            )
            .accepted
        );
        complete_identity_verification_response(
            &bob_context,
            &alice.user,
            &request_id,
            IdentityVerificationAction::Reject,
            alice_peer_id,
            MessageResponse {
                accepted: true,
                error: None,
            },
            &mut HashMap::new(),
            &mut bob_pending_incoming,
            &requests,
        )
        .unwrap();
        assert_eq!(
            alice_store.contact_identity(&bob.user).unwrap().status,
            crate::services::local_chat_store::IdentityTrustStatus::Unverified
        );
        assert_eq!(
            bob_store.contact_identity(&alice.user).unwrap().status,
            crate::services::local_chat_store::IdentityTrustStatus::Unverified
        );
        assert!(requests.lock().unwrap().is_empty());
    }

    #[test]
    fn libp2p_peer_id_uses_the_persistent_account_identity() {
        let (identity, _) = IdentityService::new()
            .create("p2p-test-user", "P2P Test", "a sufficiently long password")
            .unwrap();
        let imported = identity_from_pkcs8(identity.private_key(), &identity.user.id).unwrap();
        let expected = peer_id_for_user(&identity.user).unwrap();
        assert_eq!(imported.public().to_peer_id(), expected);
    }

    #[test]
    fn endpoint_conversion_uses_quic_and_rejects_non_socket_addresses() {
        assert_eq!(
            direct_address("192.0.2.1:47831").unwrap().to_string(),
            "/ip4/192.0.2.1/udp/47831/quic-v1"
        );
        assert!(direct_address("not-an-endpoint").is_err());
    }

    #[test]
    fn reconfiguring_updates_the_existing_quic_listener() {
        let (identity, _) = IdentityService::new()
            .create(
                "p2p-refresh-user",
                "P2P Refresh",
                "a sufficiently long password",
            )
            .unwrap();
        let private_key = Arc::new(Zeroizing::new(identity.private_key().to_vec()));
        let context = PeerContext {
            account_id: identity.user.id.clone(),
            display_name: identity.user.name.clone(),
            private_key,
            friends: HashMap::new(),
            store: LocalChatStore::in_memory(),
        };
        let socket = std::net::UdpSocket::bind(("0.0.0.0", 0)).unwrap();
        let port = socket.local_addr().unwrap().port();
        drop(socket);

        let transport = P2pTransport::new(port);
        transport.configure(&context).unwrap();
        transport.configure(&context).unwrap();
        transport.clear();
    }

    #[test]
    fn file_chunks_require_the_authenticated_conversation_participant() {
        let (owner, _) = IdentityService::new()
            .create("file-owner", "Owner", "a sufficiently long password")
            .unwrap();
        let (member, _) = IdentityService::new()
            .create("file-member", "Member", "a sufficiently long password")
            .unwrap();
        let (witness, _) = IdentityService::new()
            .create("file-witness", "Witness", "a sufficiently long password")
            .unwrap();
        let (outsider, _) = IdentityService::new()
            .create("file-outsider", "Outsider", "a sufficiently long password")
            .unwrap();

        let directory =
            std::env::temp_dir().join(format!("resonance-p2p-file-{}", new_id().unwrap()));
        let store =
            LocalChatStore::open_at(directory.join("local.json"), b"file transfer test").unwrap();
        let group_id = "0123456789abcdef0123456789abcdef";
        let members = vec![
            owner.user.clone(),
            member.user.clone(),
            witness.user.clone(),
        ];
        store.create_group(group_id, "Files", &members).unwrap();
        let attachment_id = "fedcba9876543210fedcba9876543210";
        let plaintext = b"direct";
        let (key, nonce, ciphertext) = attachment::encrypt(attachment_id, plaintext).unwrap();
        let metadata = AttachmentMetadata {
            attachment_id: attachment_id.into(),
            file_name: "direct.txt".into(),
            content_type: "text/plain".into(),
            size: plaintext.len(),
            kind: AttachmentKind::File,
            key,
            nonce,
            binding: None,
        };
        let mut message = Message::new(
            "message-id",
            group_id,
            &owner.user.id,
            &owner.user.name,
            "",
            "1",
            true,
        );
        message.attachments = vec![metadata];
        store
            .add_group_message(message, group_id, "Files", &members)
            .unwrap();
        store
            .save_attachment_ciphertext(attachment_id, &ciphertext)
            .unwrap();
        let context = PeerContext {
            account_id: owner.user.id.clone(),
            display_name: owner.user.name.clone(),
            private_key: Arc::new(Zeroizing::new(owner.private_key().to_vec())),
            friends: HashMap::new(),
            store: store.clone(),
        };
        let member_peer_id = peer_id_for_user(&member.user).unwrap();
        let peer_directory = HashMap::from([(member_peer_id, member.user.clone())]);
        let request = FileChunkRequest {
            attachment_id: attachment_id.into(),
            conversation_id: group_id.into(),
            offset: 0,
            requested_size: 16,
            verified: false,
        };

        let first = receive_file_chunk(&context, &peer_directory, member_peer_id, request.clone());
        assert_eq!(first.error, None);
        assert_eq!(first.total_size, ciphertext.len() as u64);
        assert_eq!(first.bytes, ciphertext[..16]);
        let duplicate =
            receive_file_chunk(&context, &peer_directory, member_peer_id, request.clone());
        assert_eq!(duplicate.bytes, first.bytes);
        assert_eq!(
            store
                .attachment_serving_progress(attachment_id)
                .unwrap()
                .unwrap()
                .0,
            16
        );
        let last_chunk = FileChunkRequest {
            offset: 16,
            requested_size: 16,
            ..request.clone()
        };
        let last = receive_file_chunk(&context, &peer_directory, member_peer_id, last_chunk);
        assert_eq!(last.bytes, ciphertext[16..]);
        let (served, total, _) = store
            .attachment_serving_progress(attachment_id)
            .unwrap()
            .unwrap();
        assert_eq!(
            (served, total),
            (ciphertext.len() as u64, ciphertext.len() as u64)
        );

        let confirmation = FileChunkRequest {
            attachment_id: attachment_id.into(),
            conversation_id: group_id.into(),
            offset: 0,
            requested_size: 0,
            verified: true,
        };
        let response = receive_file_chunk(
            &context,
            &peer_directory,
            member_peer_id,
            confirmation.clone(),
        );
        assert_eq!(response.error, None);
        assert!(response.bytes.is_empty());
        assert_eq!(
            store
                .attachment_transfer_receipt_status(attachment_id)
                .unwrap(),
            Some((1, 2))
        );
        let invalid_confirmation = FileChunkRequest {
            offset: 1,
            ..confirmation
        };
        assert!(
            receive_file_chunk(
                &context,
                &peer_directory,
                member_peer_id,
                invalid_confirmation
            )
            .error
            .is_some()
        );

        let outsider_peer_id = peer_id_for_user(&outsider.user).unwrap();
        let outsider_directory = HashMap::from([(outsider_peer_id, outsider.user.clone())]);
        let denied = receive_file_chunk(
            &context,
            &outsider_directory,
            outsider_peer_id,
            request.clone(),
        );
        assert!(denied.error.is_some());

        let wrong_conversation = FileChunkRequest {
            conversation_id: new_id().unwrap(),
            ..request.clone()
        };
        let denied = receive_file_chunk(
            &context,
            &peer_directory,
            member_peer_id,
            wrong_conversation,
        );
        assert!(denied.error.is_some());

        let oversized = FileChunkRequest {
            requested_size: (256 * 1024 + 1) as u32,
            ..request
        };
        assert!(
            receive_file_chunk(&context, &peer_directory, member_peer_id, oversized)
                .error
                .is_some()
        );

        drop(context);
        drop(store);
        let restored =
            LocalChatStore::open_at(directory.join("local.json"), b"file transfer test").unwrap();
        assert_eq!(
            restored
                .attachment_transfer_receipt_status(attachment_id)
                .unwrap(),
            Some((1, 2))
        );
        drop(restored);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
