use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};
use std::time::{Duration, Instant};

use gpui::Entity;
use gpui::{
    AppContext, ClipboardItem, Context, FocusHandle, Image, ImageFormat, IntoElement, KeyDownEvent,
    PathPromptOptions, Render, ScrollHandle, Task, Window, px, rgb,
};
use serde::{Deserialize, Serialize};
use sqlly_textbox::{ComponentStyle, Mode, TextBox, TextWrap, VerticalAlign};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;
use zeroize::Zeroizing;

use crate::{
    models::{
        conversation::Conversation,
        message::{GroupRecipientDelivery, Message, RecipientDeliveryState},
        user::User,
    },
    navigation::{navigation_state::NavigationState, route::Route},
    services::{
        account_api_client::{
            AccountApiClient, AccountApiError, AccountSession, AccountUser, DeviceEnrollmentStatus,
            PendingDeviceEnrollment, RegisteredDevice,
        },
        attachment::{self, AttachmentKind, AttachmentMetadata},
        audio_player::AudioPlayer,
        device_enrollment::{decrypt_history, encrypt_history, pairing_code},
        identity::{IdentityService, UnlockedIdentity},
        local_chat_store::{
            HistorySnapshot, IdentityTrustStatus, LocalChatStore, MlsWelcomeRecord,
            PendingGroupMembershipChange, new_id, timestamp,
        },
        mls::{MLS_MESSAGE_PREFIX, MlsService},
        p2p_transport::PendingIdentityVerification,
        peer_transport::PeerTransport,
        relay,
        voice_recorder::VoiceRecorder,
        voice_signal::{VoiceSignal, VoiceSignalAction},
    },
};

mod attachments;
mod auth;
mod delivery;
mod messaging;
mod settings;
mod voice;

use delivery::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMode {
    Login,
    Register,
}

fn same_group_member_ids(members: &[User], member_ids: &[String]) -> bool {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputField {
    Username,
    DisplayName,
    InviteCode,
    Password,
    RememberMe,
    ProfileName,
    CurrentPassword,
    NewPassword,
    ConfirmNewPassword,
    PeerUsername,
    VerificationFingerprint,
    GroupName,
    DeviceName,
    DevicePairingCode,
}

const AUTH_CREDENTIAL_URL: &str = "https://resonance.local/account";
const GROUP_RELAY_PREFIX: &str = "resonance-group-v1:";
const DIRECT_MLS_WELCOME_PREFIX: &str = "resonance-direct-mls-welcome-v1:";
const GROUP_MEMBERSHIP_PREFIX: &str = "resonance-mls-membership-v1:";
const RELAY_RETRY_INTERVAL_SECS: u64 = 60;
const GROUP_RELAY_MAX_ATTEMPTS: u8 = 5;

fn group_delivery_retry_at(attempts: u8, now: u64) -> String {
    let exponent = attempts.saturating_sub(1).min(4);
    let delay = 15u64.saturating_mul(2u64.pow(u32::from(exponent))).min(240);
    now.saturating_add(delay).to_string()
}

fn group_delivery_retry_due(delivery: &GroupRecipientDelivery, now: u64) -> Result<bool, String> {
    if delivery.api_accepted
        || delivery.api_failure_permanent
        || !matches!(
            delivery.state,
            RecipientDeliveryState::Pending | RecipientDeliveryState::Delivered
        )
        || delivery.attempts >= GROUP_RELAY_MAX_ATTEMPTS
    {
        return Ok(false);
    }
    let Some(next_attempt_at) = delivery.next_attempt_at.as_deref() else {
        return Ok(true);
    };
    let next_attempt_at = next_attempt_at
        .parse::<u64>()
        .map_err(|error| format!("The group retry time is invalid: {error}"))?;
    Ok(now >= next_attempt_at)
}

fn permanent_group_delivery_error(error: &AccountApiError) -> bool {
    matches!(error.status, Some(400 | 401 | 403 | 404 | 409 | 410))
        || error
            .message
            .contains("Register this installation as a device")
}

fn retryable_mls_delivery_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    [
        "has not been established",
        "out of sequence",
        "unknown epoch",
        "stale epoch",
        "future epoch",
        "missing commit",
        "commit is missing",
        "epochtooold",
        "unknown group",
        "unknown mls group",
        "group is unavailable",
        "epoch is not available",
    ]
    .iter()
    .any(|marker| error.contains(marker))
}

fn initial_group_membership_version() -> u64 {
    1
}

fn receive_device_history(
    api: &AccountApiClient,
    token: &str,
    account_public_key: &str,
    device_id: &str,
    store: &LocalChatStore,
    devices: &[RegisteredDevice],
) -> Vec<String> {
    let packages = match api.pending_history_packages(token) {
        Ok(packages) => packages,
        Err(error) => return vec![error.message],
    };
    let mut reports = Vec::new();
    for record in packages {
        let Some(source) = devices
            .iter()
            .find(|candidate| candidate.device_id == record.package.source_device_id)
        else {
            reports.push(format!(
                "History package {} has no registered source device.",
                record.package.sync_id
            ));
            continue;
        };
        let Some(signing_key) = source.signing_public_key.as_deref() else {
            reports.push(format!(
                "History package {} was sent by a device without a signing key.",
                record.package.sync_id
            ));
            continue;
        };
        let result = decrypt_history(
            account_public_key,
            &match store.device_identity() {
                Ok(device) => device,
                Err(error) => {
                    reports.push(error);
                    continue;
                }
            },
            signing_key,
            &record.package,
        )
        .and_then(|bytes| {
            serde_json::from_slice::<HistorySnapshot>(&bytes)
                .map_err(|error| format!("The history snapshot is invalid: {error}"))
        })
        .and_then(|snapshot| {
            store
                .import_history_snapshot(snapshot, account_public_key, device_id)
                .map_err(|error| format!("Could not import encrypted history: {error}"))
        });
        match result {
            Ok(imported) => {
                if let Err(error) = api.acknowledge_history_package(token, &record.package.sync_id)
                {
                    reports.push(format!(
                        "Imported history but could not acknowledge package {}: {}",
                        record.package.sync_id, error.message
                    ));
                    continue;
                }
                let unavailable = imported.unavailable_attachment_ids.len();
                reports.push(format!(
                    "Imported {} conversations, {} messages and {} encrypted attachments{}.",
                    imported.imported_conversations,
                    imported.imported_messages,
                    imported.imported_attachments,
                    if unavailable == 0 {
                        String::new()
                    } else {
                        format!(
                            "; {unavailable} attachment(s) were unavailable on the source device"
                        )
                    }
                ));
            }
            Err(error) => reports.push(error),
        }
    }
    reports
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum GroupRelayPayload {
    Invite {
        conversation_id: String,
        name: String,
        members: Vec<User>,
        #[serde(default = "initial_group_membership_version")]
        membership_version: u64,
        welcome: String,
    },
    Message {
        conversation_id: String,
        name: String,
        members: Vec<User>,
        body: String,
    },
}

#[derive(Serialize, Deserialize)]
struct GroupMembershipCommit {
    operation_id: String,
    membership_version: u64,
    members: Vec<User>,
    commit: String,
}

#[derive(Serialize, Deserialize)]
struct DirectMlsWelcome {
    conversation_id: String,
    welcome: String,
}
struct RemotePollReport {
    friends: Option<Vec<User>>,
    friend_requests: Option<Vec<User>>,
    outgoing_friend_requests: Option<Vec<User>>,
    new_friends: Vec<User>,
    messages: Result<Vec<Message>, String>,
    conversations: Result<Vec<Conversation>, String>,
    voice_signals: Vec<ReceivedVoiceSignal>,
    errors: Vec<String>,
}

fn peer_transport_friends_changed(previous: &[User], next: &[User]) -> bool {
    previous.len() != next.len()
        || next.iter().any(|friend| {
            !previous.iter().any(|current| {
                current.id == friend.id
                    && current.username == friend.username
                    && current.endpoint == friend.endpoint
            })
        })
}

struct ReceivedVoiceSignal {
    sender_id: String,
    local_conversation_id: String,
    signal: VoiceSignal,
}

struct VoiceSignalTarget {
    peer: User,
    participants: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
pub struct IncomingVoiceCall {
    pub call_id: String,
    pub conversation_id: String,
    pub caller_id: String,
    pub caller_name: String,
}

#[derive(Clone, Debug)]
pub struct ActiveVoiceCall {
    pub call_id: String,
    pub conversation_id: String,
    pub initiator_id: String,
    eligible_peers: HashSet<String>,
    pub participants: Vec<String>,
    pub muted: bool,
    pub connected: bool,
    connected_peers: HashSet<String>,
    connecting_peers: HashSet<String>,
    offering_peers: HashSet<String>,
    reconnect_attempts: HashMap<String, u8>,
}

#[derive(Clone, Debug)]
pub struct PendingAttachment {
    pub id: String,
    pub file_name: String,
    pub size: usize,
    pub kind: AttachmentKind,
    pub conversation_id: String,
    pub metadata: Option<AttachmentMetadata>,
}

pub struct ImagePreview {
    pub file_name: String,
    pub image: Arc<Image>,
}

struct AttachmentTransferProgress {
    transferred: AtomicU64,
    total: AtomicU64,
    speed_baseline: AtomicU64,
    started: Instant,
}

impl Default for AttachmentTransferProgress {
    fn default() -> Self {
        Self {
            transferred: AtomicU64::new(0),
            total: AtomicU64::new(0),
            speed_baseline: AtomicU64::new(u64::MAX),
            started: Instant::now(),
        }
    }
}

impl AttachmentTransferProgress {
    fn record(&self, transferred: u64, total: u64) {
        let _ = self.speed_baseline.compare_exchange(
            u64::MAX,
            transferred,
            Ordering::Relaxed,
            Ordering::Relaxed,
        );
        self.transferred.store(transferred, Ordering::Relaxed);
        self.total.store(total, Ordering::Relaxed);
    }
}

struct AttachmentDestination {
    conversation_id: String,
    store: LocalChatStore,
    binding: attachment::AttachmentBinding,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PendingConfirmation {
    RemoveFriend(String),
    DeleteConversation(String),
    RevokeDevice(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum AttachmentTransferState {
    Pending,
    Active,
    AwaitingConfirmation,
    Completed,
    Failed,
    Cancelled,
}

pub struct AppState {
    pub navigation: NavigationState,
    pub pending_confirmation: Option<PendingConfirmation>,
    pub current_user: Option<User>,
    pub current_device_id: Option<String>,
    pub friends: Vec<User>,
    pub conversations: Vec<Conversation>,
    pub messages: Vec<Message>,
    pub selected_conversation: Option<String>,
    pub active_menu: Option<String>,
    pub add_friend_modal_open: bool,
    pub incoming_friend_requests: Vec<User>,
    pub outgoing_friend_requests: Vec<User>,
    pub create_group_modal_open: bool,
    pub manage_group_id: Option<String>,
    pub group_name_input: String,
    pub selected_group_members: Vec<String>,
    pub auth_mode: AuthMode,
    pub username_input: String,
    pub display_name_input: String,
    pub invite_code_input: String,
    pub password_input: String,
    pub remember_me: bool,
    pub profile_name_input: String,
    pub current_password_input: String,
    pub new_password_input: String,
    pub confirm_password_input: String,
    pub device_name_input: String,
    pub device_rename_target: Option<String>,
    pub device_pairing_verification_input: String,
    pub registered_devices: Vec<RegisteredDevice>,
    pub pending_device_enrollments: Vec<PendingDeviceEnrollment>,
    pub device_enrollment_status: Option<DeviceEnrollmentStatus>,
    pub device_pairing_code: Option<String>,
    pub peer_username_input: String,
    pub identity_verification_input: String,
    pub verifying_friend_id: Option<String>,
    pub incoming_identity_verification_requests: Vec<PendingIdentityVerification>,
    pub message_input: Entity<TextBox>,
    message_body_editors: RefCell<HashMap<String, Entity<TextBox>>>,
    pub editing_message_id: Option<String>,
    active_message_send_id: Option<String>,
    pub pending_attachments: Vec<PendingAttachment>,
    pub attachment_uploads_in_progress: usize,
    pub image_preview: Option<ImagePreview>,
    pub image_preview_loading: Option<String>,
    pub audio_playback_id: Option<String>,
    pub audio_loading_attachment_id: Option<String>,
    attachment_download_cancel: Option<Arc<AtomicBool>>,
    attachment_download_progress: Option<(String, Arc<AttachmentTransferProgress>)>,
    attachment_transfer_states: HashMap<String, AttachmentTransferState>,
    attachment_upload_monitor_ids: Vec<String>,
    attachment_upload_monitor_active: bool,
    pub incoming_voice_call: Option<IncomingVoiceCall>,
    pub active_voice_call: Option<ActiveVoiceCall>,
    pub active_input: InputField,
    pub input_selection: Range<usize>,
    pub input_selection_reversed: bool,
    pub input_focus: FocusHandle,
    pub message_scroll_handle: ScrollHandle,
    pub group_member_scroll_handle: ScrollHandle,
    pub friend_requests_scroll_handle: ScrollHandle,
    pub message: Option<String>,
    toast_generation: u64,
    last_poll_error: Option<String>,
    pub notifications_enabled: bool,
    pub presence: String,
    pub startup_error: Option<String>,
    account_api: Option<AccountApiClient>,
    session_token: Option<String>,
    identity_service: IdentityService,
    identity: Option<UnlockedIdentity>,
    local_chat: Option<LocalChatStore>,
    peer_transport: Option<PeerTransport>,
    incoming_poll_task: Option<Task<()>>,
    pending_message_input_value: Option<String>,
    voice_recorder: Option<VoiceRecorder>,
    voice_recording_conversation: Option<String>,
    audio_player: Option<AudioPlayer>,
}

impl AppState {
    pub fn request_confirmation(
        &mut self,
        confirmation: PendingConfirmation,
        cx: &mut Context<Self>,
    ) {
        self.pending_confirmation = Some(confirmation);
        cx.notify();
    }

    pub fn cancel_confirmation(&mut self, cx: &mut Context<Self>) {
        self.pending_confirmation = None;
        cx.notify();
    }

    pub fn confirm_pending_action(&mut self, cx: &mut Context<Self>) {
        let Some(confirmation) = self.pending_confirmation.take() else {
            return;
        };
        match confirmation {
            PendingConfirmation::RemoveFriend(peer_id) => self.remove_friend(&peer_id, cx),
            PendingConfirmation::DeleteConversation(conversation_id) => {
                self.delete_conversation_by_id(&conversation_id, cx)
            }
            PendingConfirmation::RevokeDevice(device_id) => {
                self.revoke_registered_device(&device_id, cx)
            }
        }
        cx.notify();
    }

    pub fn new(cx: &mut Context<Self>) -> Self {
        let (account_api, startup_error) = match AccountApiClient::from_env() {
            Ok(client) => match client.health() {
                Ok(()) => (Some(client), None),
                Err(error) => (None, Some(error.message)),
            },
            Err(error) => (None, Some(error.message)),
        };
        let (peer_transport, peer_error) = match PeerTransport::bind() {
            Ok(transport) => (Some(transport), None),
            Err(error) => (None, Some(error)),
        };
        let startup_error = match (startup_error, peer_error) {
            (Some(api), Some(peer)) => Some(format!("{api} {peer}")),
            (Some(error), None) | (None, Some(error)) => Some(error),
            (None, None) => None,
        };
        let app_state = cx.entity().downgrade();
        let message_input = cx.new(|cx| {
            let mut style = ComponentStyle::default();
            style.background = rgb(crate::theme::colors::PANEL_ACTIVE).into();
            style.background_focused = rgb(crate::theme::colors::PANEL_ACTIVE).into();
            style.border_width_px = 0.0;
            style.corner_radius = px(0.0);
            style.padding = px(0.0);
            style.min_height = px(0.0);
            style.vertical_align = VerticalAlign::Top;
            style.line_height = px(20.0);
            style.font_size = px(14.0);
            style.text = rgb(crate::theme::colors::TEXT).into();
            style.placeholder = rgb(crate::theme::colors::MUTED).into();
            style.selection = rgb(crate::theme::colors::ACCENT).into();
            style.selected_text = rgb(crate::theme::colors::CANVAS).into();
            style.caret = rgb(crate::theme::colors::ACCENT).into();
            TextBox::multi_line(1, cx)
                .placeholder("Write a message…")
                .mode(Mode::MultiLine {
                    min_lines: 1,
                    max_lines: Some(6),
                    wrap: TextWrap::Soft,
                })
                .style(style)
                .on_commit(Arc::new(move |text, cx| {
                    app_state
                        .update(cx, |state, cx| {
                            state.send_message_text(text.to_string(), cx)
                        })
                        .expect(
                            "App state should remain alive while its message editor is mounted",
                        );
                }))
        });
        let state = Self {
            navigation: NavigationState::default(),
            pending_confirmation: None,
            current_user: None,
            current_device_id: None,
            friends: Vec::new(),
            conversations: Vec::new(),
            messages: Vec::new(),
            selected_conversation: None,
            active_menu: None,
            add_friend_modal_open: false,
            incoming_friend_requests: Vec::new(),
            outgoing_friend_requests: Vec::new(),
            create_group_modal_open: false,
            manage_group_id: None,
            group_name_input: String::new(),
            selected_group_members: Vec::new(),
            auth_mode: AuthMode::Login,
            username_input: String::new(),
            display_name_input: String::new(),
            invite_code_input: String::new(),
            password_input: String::new(),
            remember_me: false,
            profile_name_input: String::new(),
            current_password_input: String::new(),
            new_password_input: String::new(),
            confirm_password_input: String::new(),
            device_name_input: String::new(),
            device_rename_target: None,
            device_pairing_verification_input: String::new(),
            registered_devices: Vec::new(),
            pending_device_enrollments: Vec::new(),
            device_enrollment_status: None,
            device_pairing_code: None,
            peer_username_input: String::new(),
            identity_verification_input: String::new(),
            verifying_friend_id: None,
            incoming_identity_verification_requests: Vec::new(),
            message_input,
            message_body_editors: RefCell::new(HashMap::new()),
            editing_message_id: None,
            active_message_send_id: None,
            pending_attachments: Vec::new(),
            attachment_uploads_in_progress: 0,
            image_preview: None,
            image_preview_loading: None,
            audio_playback_id: None,
            audio_loading_attachment_id: None,
            attachment_download_cancel: None,
            attachment_download_progress: None,
            attachment_transfer_states: HashMap::new(),
            attachment_upload_monitor_ids: Vec::new(),
            attachment_upload_monitor_active: false,
            incoming_voice_call: None,
            active_voice_call: None,
            active_input: InputField::Username,
            input_selection: 0..0,
            input_selection_reversed: false,
            input_focus: cx.focus_handle(),
            message_scroll_handle: ScrollHandle::new(),
            group_member_scroll_handle: ScrollHandle::new(),
            friend_requests_scroll_handle: ScrollHandle::new(),
            message: None,
            toast_generation: 0,
            last_poll_error: None,
            notifications_enabled: true,
            presence: "Online".into(),
            startup_error,
            account_api,
            session_token: None,
            identity_service: IdentityService::new(),
            identity: None,
            local_chat: None,
            peer_transport,
            incoming_poll_task: None,
            pending_message_input_value: None,
            voice_recorder: None,
            voice_recording_conversation: None,
            audio_player: None,
        };
        cx.spawn(async move |this, cx| {
            let task = cx.update(|cx| cx.read_credentials(AUTH_CREDENTIAL_URL));
            let result = task.await;
            this.update(cx, |state, cx| match result {
                Ok(Some((username, password))) => match String::from_utf8(password) {
                    Ok(password) => {
                        if state.username_input.is_empty() && state.password_input.is_empty() {
                            state.username_input = username;
                            state.password_input = password;
                        }
                        state.remember_me = true;
                        cx.notify();
                    }
                    Err(_) => {
                        state.set_toast(
                            Some("Saved login credentials could not be decoded.".into()),
                            cx,
                        );
                        cx.notify();
                    }
                },
                Ok(None) => {}
                Err(error) => {
                    state.set_toast(
                        Some(format!("Could not read saved login credentials: {error}")),
                        cx,
                    );
                    cx.notify();
                }
            })
        })
        .detach();
        state
    }

    pub fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        self.active_menu = None;
        self.navigation.navigate(route);
        cx.notify();
    }

    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        self.active_menu = None;
        self.navigation.open_settings();
        cx.notify();
    }

    pub fn select_settings_page(
        &mut self,
        page: crate::navigation::route::SettingsPage,
        cx: &mut Context<Self>,
    ) {
        self.active_menu = None;
        self.navigation.select_settings_page(page);
        if page == crate::navigation::route::SettingsPage::Devices {
            self.refresh_device_management(cx);
        }
        cx.notify();
    }

    pub fn refresh_device_management(&mut self, cx: &mut Context<Self>) {
        let (mut api, token) = match self.api_session() {
            Ok(session) => session,
            Err(error) => {
                self.set_toast(Some(error), cx);
                return;
            }
        };
        let mut newly_authorized = false;
        if !api.has_device_auth() {
            let Some(status) = self.device_enrollment_status.clone() else {
                self.set_toast(
                    Some("This installation is not authorized by a trusted device.".into()),
                    cx,
                );
                return;
            };
            match api.device_enrollment_status(&token, &status.request_id) {
                Ok(status) if status.state == "authorized" => {
                    let Some(identity) = self.identity.as_ref() else {
                        self.set_toast(Some("The account identity is unavailable.".into()), cx);
                        return;
                    };
                    let Some(store) = self.local_chat.as_ref() else {
                        self.set_toast(Some("Local chat storage is unavailable.".into()), cx);
                        return;
                    };
                    match store.device_identity().and_then(|device| {
                        api.register_device(
                            &token,
                            &device,
                            &identity.user.id,
                            identity.private_key(),
                        )
                        .map_err(|error| error.message)
                    }) {
                        Ok(device_api) => {
                            api = device_api;
                            self.account_api = Some(api.clone());
                            self.device_enrollment_status = None;
                            self.device_pairing_code = None;
                            newly_authorized = true;
                        }
                        Err(error) => {
                            self.set_toast(Some(error), cx);
                            return;
                        }
                    }
                }
                Ok(status) => {
                    let expired = status.state == "expired";
                    self.device_enrollment_status = Some(status);
                    self.set_toast(
                        Some(if expired {
                            "The enrollment request expired. Start a new request to try again."
                                .into()
                        } else {
                            "This installation is still awaiting trusted-device approval.".into()
                        }),
                        cx,
                    );
                    return;
                }
                Err(error) => {
                    self.set_toast(Some(error.message), cx);
                    return;
                }
            }
        }
        let devices = match api.registered_devices(&token) {
            Ok(devices) => devices,
            Err(error) => {
                self.set_toast(Some(error.message), cx);
                return;
            }
        };
        let pending = match api.pending_device_enrollments(&token) {
            Ok(pending) => pending,
            Err(error) => {
                self.set_toast(Some(error.message), cx);
                return;
            }
        };
        self.registered_devices = devices;
        self.pending_device_enrollments = pending;
        let mut history_result = None;
        if let (Some(identity), Some(store)) = (&self.identity, &self.local_chat) {
            match store.device_identity() {
                Ok(device) => {
                    let reports = receive_device_history(
                        &api,
                        &token,
                        &identity.user.id,
                        &device.id,
                        store,
                        &self.registered_devices,
                    );
                    if !reports.is_empty() {
                        history_result = Some((
                            reports,
                            store.conversations().and_then(|conversations| {
                                store.messages().map(|messages| (conversations, messages))
                            }),
                        ));
                    }
                }
                Err(error) => self.set_toast(Some(error), cx),
            }
        }
        if let Some((reports, stored_messages)) = history_result {
            self.set_toast(Some(reports.join(" ")), cx);
            match stored_messages {
                Ok((conversations, messages)) => {
                    self.conversations = conversations;
                    self.messages = messages;
                }
                Err(error) => self.set_toast(Some(error), cx),
            }
        }
        if newly_authorized {
            self.load_remote_data(true, cx);
            self.start_incoming_message_poll(cx);
        }
        cx.notify();
    }

    pub fn restart_device_enrollment(&mut self, cx: &mut Context<Self>) {
        let result = (|| {
            let (api, token) = self.api_session().map_err(|error| error)?;
            let api = api.without_device_auth();
            let identity = self
                .identity
                .as_ref()
                .ok_or_else(|| "The account identity is unavailable.".to_string())?;
            let store = self
                .local_chat
                .as_ref()
                .ok_or_else(|| "Local chat storage is unavailable.".to_string())?;
            let device = store.restart_device_enrollment()?;
            let status = api
                .begin_device_enrollment(&token, &device, &identity.user.id)
                .map_err(|error| error.message)?;
            let (signing_key, encryption_key) =
                crate::services::device_enrollment::device_public_keys(&device)?;
            let code = pairing_code(
                &identity.user.id,
                &status.request_id,
                &device.id,
                &signing_key,
                &encryption_key,
                &device.enrollment_nonce,
            );
            Ok::<_, String>((status, code))
        })();
        match result {
            Ok((status, code)) => {
                self.device_enrollment_status = Some(status);
                self.device_pairing_code = Some(code);
                self.set_toast(
                    Some("A new device authorization request was created.".into()),
                    cx,
                );
            }
            Err(error) => self.set_toast(Some(error), cx),
        }
        cx.notify();
    }

    pub fn approve_device_enrollment(&mut self, request_id: &str, cx: &mut Context<Self>) {
        let result = (|| {
            let (api, token) = self.api_session().map_err(|error| error)?;
            let enrollment = self
                .pending_device_enrollments
                .iter()
                .find(|enrollment| enrollment.request_id == request_id)
                .cloned()
                .ok_or_else(|| "The enrollment request is no longer available.".to_string())?;
            let device = self
                .local_chat
                .as_ref()
                .ok_or_else(|| "Local chat storage is unavailable.".to_string())?
                .device_identity()?;
            let account_id = self
                .current_user
                .as_ref()
                .ok_or_else(|| "The account identity is unavailable.".to_string())?
                .id
                .clone();
            let expected_code = pairing_code(
                &account_id,
                &enrollment.request_id,
                &enrollment.device_id,
                &enrollment.signing_public_key,
                &enrollment.encryption_public_key,
                &enrollment.nonce,
            );
            if !self
                .device_pairing_verification_input
                .trim()
                .eq_ignore_ascii_case(&expected_code)
            {
                return Err("The pairing code does not match. Compare it with the new device before authorizing.".into());
            }
            api.approve_device_enrollment(&token, &enrollment, &device, &account_id)
                .map_err(|error| error.message)
        })();
        match result {
            Ok(_) => {
                self.device_pairing_verification_input.clear();
                self.set_toast(
                    Some("Device authorized. You can now securely send history to it.".into()),
                    cx,
                );
                self.refresh_device_management(cx);
            }
            Err(error) => self.set_toast(Some(error), cx),
        }
        cx.notify();
    }

    pub fn open_device_rename(&mut self, device_id: &str, cx: &mut Context<Self>) -> bool {
        let Some(device_name) = self
            .registered_devices
            .iter()
            .find(|device| device.device_id == device_id && device.revoked_at.is_none())
            .map(|device| device.device_name.clone())
        else {
            self.set_toast(
                Some("This device is no longer available to rename.".into()),
                cx,
            );
            return false;
        };
        self.device_name_input = device_name;
        self.device_rename_target = Some(device_id.to_string());
        self.activate_input(InputField::DeviceName);
        cx.notify();
        true
    }

    pub fn cancel_device_rename(&mut self, cx: &mut Context<Self>) {
        self.device_rename_target = None;
        self.device_name_input.clear();
        cx.notify();
    }

    pub fn confirm_device_rename(&mut self, cx: &mut Context<Self>) {
        let Some(device_id) = self.device_rename_target.clone() else {
            self.set_toast(
                Some("Select a device before confirming its new name.".into()),
                cx,
            );
            return;
        };
        self.rename_registered_device(&device_id, cx);
    }

    pub fn rename_registered_device(&mut self, device_id: &str, cx: &mut Context<Self>) {
        if self.device_rename_target.as_deref() != Some(device_id) {
            self.set_toast(
                Some("Select a device before confirming its new name.".into()),
                cx,
            );
            return;
        }

        let result = (|| {
            let (api, token) = self.api_session().map_err(|error| error)?;
            let name = self.device_name_input.trim();
            if name.is_empty() || name.chars().count() > 80 || name.chars().any(char::is_control) {
                return Err("Device names must contain 1-80 non-control characters.".into());
            }
            if !self
                .registered_devices
                .iter()
                .any(|device| device.device_id == device_id && device.revoked_at.is_none())
            {
                return Err("This device is no longer available to rename.".into());
            }
            api.rename_device(&token, device_id, name)
                .map_err(|error| error.message)?;
            if let Some(store) = &self.local_chat {
                if let Ok(device) = store.device_identity() {
                    if device.id == device_id {
                        store.rename_local_device(name)?;
                    }
                }
            }
            Ok::<(), String>(())
        })();
        match result {
            Ok(_) => {
                self.device_rename_target = None;
                self.device_name_input.clear();
                self.set_toast(Some("Device name updated.".into()), cx);
                self.refresh_device_management(cx);
            }
            Err(error) => self.set_toast(Some(error), cx),
        }
        cx.notify();
    }

    pub fn revoke_registered_device(&mut self, device_id: &str, cx: &mut Context<Self>) {
        let result = self
            .api_session()
            .map_err(|error| error)
            .and_then(|(api, token)| {
                api.revoke_device(&token, device_id)
                    .map_err(|error| error.message)
            });
        match result {
            Ok(_) => {
                self.set_toast(
                    Some(
                        "Device revoked; its future queue and history access are disabled.".into(),
                    ),
                    cx,
                );
                self.refresh_device_management(cx);
            }
            Err(error) => self.set_toast(Some(error), cx),
        }
        cx.notify();
    }

    pub fn send_device_history(&mut self, target_device_id: &str, cx: &mut Context<Self>) {
        let result = (|| {
            let (api, token) = self.api_session().map_err(|error| error)?;
            let identity = self
                .identity
                .as_ref()
                .ok_or_else(|| "The account identity is unavailable.".to_string())?;
            let store = self
                .local_chat
                .as_ref()
                .ok_or_else(|| "Local chat storage is unavailable.".to_string())?;
            let source = store.device_identity()?;
            let target = self
                .registered_devices
                .iter()
                .find(|device| device.device_id == target_device_id)
                .ok_or_else(|| "The target device is no longer registered.".to_string())?;
            let encryption_key = target
                .encryption_public_key
                .as_deref()
                .ok_or_else(|| "The target device has no history-encryption key.".to_string())?;
            let snapshot =
                store.export_history_snapshot(&identity.user.id, &source.id, target_device_id)?;
            let plaintext = serde_json::to_vec(&snapshot)
                .map_err(|error| format!("Could not encode the local history snapshot: {error}"))?;
            let package = encrypt_history(
                &identity.user.id,
                &source,
                target_device_id,
                encryption_key,
                &plaintext,
            )?;
            api.send_history_package(&token, &package)
                .map_err(|error| error.message)?;
            Ok::<String, String>(format!(
                "Encrypted history package sent to {}. Attachments unavailable locally are reported during import.",
                target.device_name
            ))
        })();
        match result {
            Ok(message) => self.set_toast(Some(message), cx),
            Err(error) => self.set_toast(Some(error), cx),
        }
        cx.notify();
    }

    pub fn close_settings(&mut self, cx: &mut Context<Self>) {
        self.device_rename_target = None;
        self.device_name_input.clear();
        self.navigation.close_settings();
        cx.notify();
    }

    pub fn toggle_menu(&mut self, menu_id: &str, cx: &mut Context<Self>) {
        self.active_menu = if self.active_menu.as_deref() == Some(menu_id) {
            None
        } else {
            Some(menu_id.to_string())
        };
        cx.notify();
    }

    pub fn dismiss_menu(&mut self, menu_id: &str, cx: &mut Context<Self>) {
        if self.active_menu.as_deref() == Some(menu_id) {
            self.active_menu = None;
            cx.notify();
        }
    }

    pub fn set_toast(&mut self, message: Option<String>, cx: &mut Context<Self>) {
        self.message = message;
        self.toast_generation = self.toast_generation.wrapping_add(1);
        if self.message.is_some() {
            let generation = self.toast_generation;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_secs(5)).await;
                this.update(cx, |state, cx| {
                    if state.toast_generation == generation {
                        state.set_toast(None, cx);
                    }
                })
            })
            .detach();
        }
        cx.notify();
    }

    pub fn dismiss_toast(&mut self, cx: &mut Context<Self>) {
        self.set_toast(None, cx);
    }

    pub fn toggle_add_friend_modal(&mut self, cx: &mut Context<Self>) {
        self.add_friend_modal_open = !self.add_friend_modal_open;
        if self.add_friend_modal_open {
            self.activate_input(InputField::PeerUsername);
            self.load_friend_requests(cx);
        }
        cx.notify();
    }

    fn load_friend_requests(&mut self, cx: &mut Context<Self>) {
        match self.api_session() {
            Ok((api, token)) => {
                match api.friend_requests(&token) {
                    Ok(requests) => self.incoming_friend_requests = requests,
                    Err(error) => self.set_toast(Some(error.message), cx),
                }
                match api.outgoing_friend_requests(&token) {
                    Ok(requests) => self.outgoing_friend_requests = requests,
                    Err(error) => self.set_toast(Some(error.message), cx),
                }
            }
            Err(error) => self.set_toast(Some(error), cx),
        }
        cx.notify();
    }

    pub fn toggle_create_group_modal(&mut self, cx: &mut Context<Self>) {
        self.create_group_modal_open = !self.create_group_modal_open;
        if self.create_group_modal_open {
            self.group_name_input.clear();
            self.selected_group_members.clear();
            self.activate_input(InputField::GroupName);
        }
        self.set_toast(None, cx);
        cx.notify();
    }

    pub fn toggle_group_member(&mut self, member_id: &str, cx: &mut Context<Self>) {
        if self.selected_group_members.iter().any(|id| id == member_id) {
            self.selected_group_members.retain(|id| id != member_id);
        } else if self.selected_group_members.len() < 11
            && self.friends.iter().any(|friend| friend.id == member_id)
        {
            self.selected_group_members.push(member_id.to_string());
        } else {
            self.set_toast(
                Some("Groups can include up to 12 members, including you.".into()),
                cx,
            );
        }
        cx.notify();
    }

    pub fn open_group_membership(&mut self, group_id: &str, cx: &mut Context<Self>) {
        let is_owner = self
            .conversations
            .iter()
            .find(|conversation| conversation.id == group_id && conversation.is_group())
            .and_then(|conversation| conversation.members.first())
            .zip(self.current_user.as_ref())
            .is_some_and(|(owner, user)| owner.id == user.id);
        if is_owner {
            self.manage_group_id = Some(group_id.to_string());
        } else {
            self.set_toast(
                Some("Only the group owner can manage membership.".into()),
                cx,
            );
        }
        cx.notify();
    }

    pub fn close_group_membership(&mut self, cx: &mut Context<Self>) {
        self.manage_group_id = None;
        cx.notify();
    }

    pub fn add_group_member(&mut self, group_id: &str, member_id: &str, cx: &mut Context<Self>) {
        self.change_group_membership(group_id, member_id, true, cx);
    }

    pub fn remove_group_member(&mut self, group_id: &str, member_id: &str, cx: &mut Context<Self>) {
        self.change_group_membership(group_id, member_id, false, cx);
    }

    fn change_group_membership(
        &mut self,
        group_id: &str,
        member_id: &str,
        adding: bool,
        cx: &mut Context<Self>,
    ) {
        let result = (|| {
            let current_user = self
                .current_user
                .clone()
                .ok_or_else(|| "Sign in before changing group membership.".to_string())?;
            let identity = self
                .identity
                .as_ref()
                .ok_or_else(|| "The unlocked account identity is unavailable.".to_string())?;
            let store = self
                .local_chat
                .as_ref()
                .ok_or_else(|| "Local message storage is unavailable.".to_string())?;
            let (api, token) = self.api_session()?;
            let conversation = store
                .conversations()?
                .into_iter()
                .find(|item| item.id == group_id && item.is_group())
                .ok_or_else(|| "The group conversation is unavailable.".to_string())?;
            if conversation
                .members
                .first()
                .map(|member| member.id.as_str())
                != Some(current_user.id.as_str())
            {
                return Err("Only the group owner can change membership.".into());
            }
            let target = if adding {
                self.friends
                    .iter()
                    .find(|friend| friend.id == member_id)
                    .cloned()
                    .ok_or_else(|| "Only a current friend can be added.".to_string())?
            } else {
                conversation
                    .members
                    .iter()
                    .find(|member| member.id == member_id)
                    .cloned()
                    .ok_or_else(|| "The member is not in this group.".to_string())?
            };
            if adding {
                store.ensure_identity_not_changed(&target)?;
            }
            prepare_group_membership_change(
                &api,
                &token,
                store,
                &current_user,
                identity.private_key(),
                &conversation,
                &target,
                adding,
            )?;
            retry_pending_group_membership_changes(
                &api,
                &token,
                store,
                &current_user.id,
                identity.private_key(),
            )
        })();
        match result {
            Ok(()) => self.set_toast(Some("MLS membership updated.".into()), cx),
            Err(error) => self.set_toast(
                Some(format!("Group membership change pending: {error}")),
                cx,
            ),
        }
        if let Some(store) = self.local_chat.clone() {
            self.refresh_conversations(&store, cx);
        }
        cx.notify();
    }

    pub fn create_group(&mut self, cx: &mut Context<Self>) {
        let name = self.group_name_input.trim();
        if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
            self.set_toast(
                Some("Group names must contain 1-64 visible characters.".into()),
                cx,
            );
            cx.notify();
            return;
        }
        if self.selected_group_members.len() < 2 {
            self.set_toast(Some("Select at least two friends for a group.".into()), cx);
            cx.notify();
            return;
        }
        let Some(current_user) = self.current_user.clone() else {
            self.set_toast(Some("Sign in before creating a group.".into()), cx);
            cx.notify();
            return;
        };
        let Some(store) = self.local_chat.clone() else {
            self.set_toast(Some("Local message storage is unavailable.".into()), cx);
            cx.notify();
            return;
        };
        let (api, token) = match self.api_session() {
            Ok(session) => session,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let private_key = match self.identity.as_ref() {
            Some(identity) => Zeroizing::new(identity.private_key().to_vec()),
            None => {
                self.set_toast(Some("Sign in before creating a group.".into()), cx);
                cx.notify();
                return;
            }
        };
        let peers: Vec<User> = self
            .friends
            .iter()
            .filter(|friend| self.selected_group_members.contains(&friend.id))
            .cloned()
            .collect();
        if peers.len() != self.selected_group_members.len() {
            self.set_toast(
                Some("One or more selected friends are no longer available.".into()),
                cx,
            );
            cx.notify();
            return;
        }
        for peer in &peers {
            if let Err(error) = store.ensure_identity_not_changed(peer) {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        }
        let mut members = vec![sanitized_group_member(current_user.clone())];
        members.extend(peers.iter().cloned().map(sanitized_group_member));
        let group_id = match new_id() {
            Ok(id) => id,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let member_public_keys: Vec<_> = members.iter().map(|member| member.id.clone()).collect();
        let conversation = match store.create_group(&group_id, name, &members) {
            Ok(conversation) => conversation,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let welcome = match create_mls_group_from_api(
            &api,
            &token,
            &store,
            &current_user.id,
            private_key.as_slice(),
            &conversation.id,
            &peers,
        ) {
            Ok(welcome) => welcome,
            Err(error) => {
                let _ = store.delete_conversation(&conversation.id);
                self.set_toast(Some(format!("Could not secure the group: {error}")), cx);
                cx.notify();
                return;
            }
        };
        if let Err(error) = api.create_group(&token, &group_id, &member_public_keys) {
            let _ = MlsService::delete_group(
                &store,
                &current_user.id,
                private_key.as_slice(),
                &conversation.id,
            );
            let _ = store.delete_conversation(&conversation.id);
            self.set_toast(
                Some(format!("Could not register the group: {}", error.message)),
                cx,
            );
            cx.notify();
            return;
        }

        let invite = match encode_group_payload(&GroupRelayPayload::Invite {
            conversation_id: conversation.id.clone(),
            name: conversation.name.clone(),
            members: conversation.members.clone(),
            membership_version: conversation.membership_version,
            welcome,
        }) {
            Ok(payload) => payload,
            Err(error) => {
                self.set_toast(
                    Some(format!("Group created, but invitations failed: {error}")),
                    cx,
                );
                self.refresh_conversations(&store, cx);
                cx.notify();
                return;
            }
        };
        let welcome_record = match store.mls_welcome(&conversation.id) {
            Ok(Some(record)) => record,
            Ok(None) => {
                self.set_toast(
                    Some("The encrypted group invitation could not be recovered.".into()),
                    cx,
                );
                cx.notify();
                return;
            }
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let mut failed_invites = Vec::new();
        for peer in &peers {
            let result = (|| {
                let envelope = relay::encrypt(
                    private_key.as_slice(),
                    &peer.id,
                    &peer.id,
                    &welcome_record.message_id,
                    &welcome_record.sent_at,
                    Some(&conversation.id),
                    &invite,
                )?;
                api.send_relay_message(&token, &peer.id, Some(&conversation.id), &envelope)
                    .map_err(|error| error.message)
            })();
            match result {
                Ok(_) => {
                    if let Err(error) = store.mark_mls_welcome_queued(&conversation.id, &peer.id) {
                        failed_invites.push(format!("@{}: {error}", peer.username));
                    }
                }
                Err(error) => failed_invites.push(format!("@{}: {error}", peer.username)),
            }
        }
        self.create_group_modal_open = false;
        self.manage_group_id = None;
        self.group_name_input.clear();
        self.selected_group_members.clear();
        self.refresh_conversations(&store, cx);
        if failed_invites.is_empty() {
            self.set_toast(Some("Group created and invitations sent.".into()), cx);
        } else {
            self.set_toast(
                Some(format!(
                    "Group created, but invitations could not be sent to {}.",
                    failed_invites.join(", ")
                )),
                cx,
            );
        }
        self.open_conversation(&conversation.id, cx);
        cx.notify();
    }

    fn refresh_conversations(&mut self, store: &LocalChatStore, cx: &mut Context<Self>) {
        match store.conversations() {
            Ok(conversations) => self.conversations = conversations,
            Err(error) => self.set_toast(Some(error), cx),
        }
    }

    pub fn conversation(&self, id: &str) -> Option<&Conversation> {
        self.conversations
            .iter()
            .find(|conversation| conversation.id == id)
    }

    pub fn conversation_messages(&self, id: &str) -> impl Iterator<Item = &Message> {
        self.messages
            .iter()
            .filter(move |message| message.conversation_id == id)
    }

    pub fn friend(&self, id: &str) -> Option<&User> {
        self.friends.iter().find(|friend| friend.id == id)
    }

    pub fn identity_status_label(&self, user: &User) -> String {
        if self
            .current_user
            .as_ref()
            .is_some_and(|current| current.id == user.id)
        {
            return "Your account identity".into();
        }
        let Some(store) = &self.local_chat else {
            return "Identity status unavailable".into();
        };
        match store.contact_identity(user) {
            Ok(identity) => match identity.status {
                IdentityTrustStatus::Unverified => "Not verified".into(),
                IdentityTrustStatus::Verified => "Verified on this device".into(),
                IdentityTrustStatus::Changed => "Identity key changed".into(),
                IdentityTrustStatus::Revoked => "Verification revoked".into(),
            },
            Err(_) => "Identity status unavailable".into(),
        }
    }

    pub fn group_identity_status_label(&self, group_id: &str, user: &User) -> String {
        let identity_status = self.identity_status_label(user);
        let Some(store) = &self.local_chat else {
            return format!("{identity_status} · MLS status unavailable");
        };
        match store.group_has_credential_change(group_id, &user.id) {
            Ok(true) => format!("{identity_status} · MLS credential changed"),
            Ok(false) => identity_status,
            Err(_) => format!("{identity_status} · MLS status unavailable"),
        }
    }

    pub fn open_identity_verification(&mut self, peer_id: &str, cx: &mut Context<Self>) {
        if self.friend(peer_id).is_none() {
            self.set_toast(
                Some("This account is no longer in your friend list.".into()),
                cx,
            );
            return;
        }
        self.verifying_friend_id = Some(peer_id.to_string());
        self.identity_verification_input.clear();
        self.set_toast(None, cx);
        cx.notify();
    }

    pub fn close_identity_verification(&mut self, cx: &mut Context<Self>) {
        self.verifying_friend_id = None;
        self.identity_verification_input.clear();
        self.activate_input(InputField::PeerUsername);
        cx.notify();
    }

    pub fn request_contact_identity_verification(&mut self, peer_id: &str, cx: &mut Context<Self>) {
        let Some(friend) = self.friend(peer_id).cloned() else {
            self.set_toast(
                Some("This account is no longer in your friend list.".into()),
                cx,
            );
            cx.notify();
            return;
        };
        let Some(transport) = self.peer_transport.clone() else {
            self.set_toast(Some("Peer networking is unavailable.".into()), cx);
            cx.notify();
            return;
        };
        self.set_toast(
            Some(format!(
                "Sending a live identity verification request to @{}…",
                friend.username
            )),
            cx,
        );
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    std::thread::spawn(move || transport.request_identity_verification(&friend))
                        .join()
                        .unwrap_or_else(|_| {
                            Err("The identity verification worker stopped unexpectedly.".into())
                        })
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                match result {
                    Ok(_) => {
                        state.verifying_friend_id = None;
                        state.set_toast(
                            Some(
                                "Verification request sent. Your friend must accept while online."
                                    .into(),
                            ),
                            cx,
                        );
                    }
                    Err(error) => state.set_toast(Some(error), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn respond_to_identity_verification(
        &mut self,
        peer_id: &str,
        request_id: &str,
        accepted: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(request) = self
            .incoming_identity_verification_requests
            .iter()
            .find(|request| request.friend.id == peer_id && request.request_id == request_id)
            .cloned()
        else {
            self.set_toast(
                Some("This identity verification request is no longer pending.".into()),
                cx,
            );
            cx.notify();
            return;
        };
        let Some(transport) = self.peer_transport.clone() else {
            self.set_toast(Some("Peer networking is unavailable.".into()), cx);
            cx.notify();
            return;
        };
        self.set_toast(
            Some(if accepted {
                format!(
                    "Accepting @{}'s live verification request…",
                    request.friend.username
                )
            } else {
                format!(
                    "Declining @{}'s live verification request…",
                    request.friend.username
                )
            }),
            cx,
        );
        cx.notify();
        let friend = request.friend.clone();
        let request_id = request.request_id.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    std::thread::spawn(move || {
                        transport.respond_to_identity_verification(&friend, &request_id, accepted)
                    })
                    .join()
                    .unwrap_or_else(|_| {
                        Err("The identity verification worker stopped unexpectedly.".into())
                    })
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                match result {
                    Ok(()) => {
                        state
                            .incoming_identity_verification_requests
                            .retain(|pending| {
                                pending.friend.id != request.friend.id
                                    || pending.request_id != request.request_id
                            });
                        state.set_toast(
                            Some(if accepted {
                                format!(
                                    "Verified @{}'s identity on this device.",
                                    request.friend.username
                                )
                            } else {
                                "Identity verification request declined.".into()
                            }),
                            cx,
                        );
                    }
                    Err(error) => state.set_toast(Some(error), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn verify_contact_identity(&mut self, cx: &mut Context<Self>) {
        let Some(peer_id) = self.verifying_friend_id.as_deref() else {
            return;
        };
        let Some(friend) = self.friend(peer_id).cloned() else {
            self.set_toast(
                Some("This account is no longer in your friend list.".into()),
                cx,
            );
            return;
        };
        let Some(store) = self.local_chat.clone() else {
            self.set_toast(Some("Local identity storage is unavailable.".into()), cx);
            return;
        };
        match store.verify_contact_identity(&friend, &self.identity_verification_input) {
            Ok(()) => {
                self.verifying_friend_id = None;
                self.identity_verification_input.clear();
                self.set_toast(
                    Some(format!(
                        "Verified @{}'s identity key on this device.",
                        friend.username
                    )),
                    cx,
                );
            }
            Err(error) => self.set_toast(Some(error), cx),
        }
        cx.notify();
    }

    pub fn revoke_contact_identity(&mut self, peer_id: &str, cx: &mut Context<Self>) {
        let Some(friend) = self.friend(peer_id).cloned() else {
            self.set_toast(
                Some("This account is no longer in your friend list.".into()),
                cx,
            );
            return;
        };
        let Some(store) = &self.local_chat else {
            self.set_toast(Some("Local identity storage is unavailable.".into()), cx);
            return;
        };
        match store.revoke_contact_verification(&friend) {
            Ok(()) => self.set_toast(
                Some(format!(
                    "Revoked identity verification for @{}.",
                    friend.username
                )),
                cx,
            ),
            Err(error) => self.set_toast(Some(error), cx),
        }
        cx.notify();
    }

    pub fn own_identity_fingerprint(&self) -> Result<String, String> {
        let identity = self
            .identity
            .as_ref()
            .ok_or_else(|| "Sign in to display your identity fingerprint.".to_string())?;
        crate::services::identity_verification::fingerprint(&identity.user.id)
    }

    pub fn own_identity_qr_svg(&self) -> Result<String, String> {
        let identity = self
            .identity
            .as_ref()
            .ok_or_else(|| "Sign in to display your identity QR code.".to_string())?;
        crate::services::identity_verification::qr_svg(&identity.user.id)
    }

    pub(crate) fn handle_menu_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.active_menu.is_some() {
            match event.keystroke.key.to_ascii_lowercase().as_str() {
                "escape" => {
                    self.active_menu = None;
                    cx.notify();
                    return true;
                }
                "arrowdown" | "down" => {
                    window.focus_next(cx);
                    return true;
                }
                "arrowup" | "up" => {
                    window.focus_prev(cx);
                    return true;
                }
                _ => return true,
            }
        }
        false
    }

    fn start_incoming_message_poll(&mut self, cx: &mut Context<Self>) {
        self.incoming_poll_task = Some(cx.spawn(async move |this, cx| {
            let mut poll_interval = Duration::from_secs(5);
            let mut service_unavailable = false;
            loop {
                let config = this
                    .update(cx, |state, _| {
                        let (api, token) = state.api_session().ok()?;
                        let identity = state.identity.as_ref()?;
                        let store = state.local_chat.clone()?;
                        let port = state.peer_transport.as_ref().map(PeerTransport::port);
                        Some((
                            api,
                            token,
                            identity.user.id.clone(),
                            Zeroizing::new(identity.private_key().to_vec()),
                            store,
                            state.friends.clone(),
                            state.peer_transport.clone(),
                            port,
                            if state.navigation.route == Route::Messages {
                                state.selected_conversation.clone()
                            } else {
                                None
                            },
                            state.navigation.route == Route::Messages,
                            state.active_voice_call.is_some() || state.incoming_voice_call.is_some(),
                        ))
                    })
                    .ok()
                    .flatten();
                let Some((
                    api,
                    token,
                    account_id,
                    private_key,
                    store,
                    known_friends,
                    peer_transport,
                    port,
                    read_conversation_id,
                    messages_view_active,
                    voice_call_active,
                )) = config
                else {
                    break;
                };
                let poll_delay = if voice_call_active && !service_unavailable {
                    Duration::from_secs(1)
                } else if messages_view_active && !service_unavailable {
                    Duration::from_secs(2)
                } else {
                    poll_interval
                };
                let report = cx
                    .background_spawn(async move {
                        std::thread::sleep(poll_delay);
                        let fallback_store = store.clone();
                        match std::thread::spawn(move || {
                            poll_account(
                                api,
                                token,
                                account_id,
                                private_key,
                                store,
                                known_friends,
                                peer_transport,
                                port,
                                read_conversation_id,
                            )
                        })
                        .join()
                        {
                            Ok(report) => report,
                            Err(_) => RemotePollReport {
                                friends: None,
                                friend_requests: None,
                                outgoing_friend_requests: None,
                                new_friends: Vec::new(),
                                messages: fallback_store.messages(),
                                conversations: fallback_store.conversations(),
                                voice_signals: Vec::new(),
                                errors: vec![
                                    "The account polling worker stopped unexpectedly.".into(),
                                ],
                            },
                        }
                    })
                    .await;
                service_unavailable = report
                    .errors
                    .iter()
                    .any(|error| error.starts_with("Account service is temporarily unavailable."));
                poll_interval = if service_unavailable {
                    Duration::from_secs((poll_interval.as_secs() * 2).min(60))
                } else {
                    Duration::from_secs(5)
                };
                let mut seen_errors = HashSet::new();
                let poll_error = report
                    .errors
                    .iter()
                    .filter(|error| seen_errors.insert(error.as_str()))
                    .cloned()
                    .collect::<Vec<_>>();
                let mut peer_configuration = None;
                let keep_polling = this
                    .update(cx, |state, cx| {
                        if state.current_user.is_none() {
                            return false;
                        }
                        let mut state_changed = false;
                        if let Some(friends) = report.friends {
                            if !report.new_friends.is_empty() {
                                state.set_toast(
                                    Some(format!(
                                        "New friend(s): {}.",
                                        report
                                            .new_friends
                                            .iter()
                                            .map(|friend| friend.name.as_str())
                                            .collect::<Vec<_>>()
                                            .join(", ")
                                    )),
                                    cx,
                                );
                            }
                            if state.friends != friends {
                                let peer_configuration_changed =
                                    peer_transport_friends_changed(&state.friends, &friends);
                                state.friends = friends;
                                state_changed = true;
                                if peer_configuration_changed {
                                    if let (Some(transport), Some(identity), Some(store)) = (
                                        &state.peer_transport,
                                        &state.identity,
                                        &state.local_chat,
                                    ) {
                                        peer_configuration = Some((
                                            transport.clone(),
                                            identity.clone(),
                                            state.friends.clone(),
                                            store.clone(),
                                        ));
                                    }
                                }
                            }
                        }
                        if let Some(requests) = report.friend_requests {
                            if state.incoming_friend_requests != requests {
                                state.incoming_friend_requests = requests;
                                state_changed = true;
                            }
                        }
                        if let Some(requests) = report.outgoing_friend_requests {
                            if state.outgoing_friend_requests != requests {
                                state.outgoing_friend_requests = requests;
                                state_changed = true;
                            }
                        }
                        if let Some(transport) = &state.peer_transport {
                            match transport.take_identity_verification_requests() {
                                Ok(requests) => {
                                    let mut incoming_names = Vec::new();
                                    for request in requests {
                                        if !state
                                            .incoming_identity_verification_requests
                                            .iter()
                                            .any(|pending| {
                                                pending.friend.id == request.friend.id
                                                    && pending.request_id == request.request_id
                                            })
                                        {
                                            incoming_names.push(request.friend.username.clone());
                                            state
                                                .incoming_identity_verification_requests
                                                .push(request);
                                        }
                                    }
                                    if !incoming_names.is_empty() {
                                        state.set_toast(
                                            Some(format!(
                                                "Live identity verification request from {}. Open Friends to accept or decline.",
                                                incoming_names
                                                    .iter()
                                                    .map(|username| format!("@{username}"))
                                                    .collect::<Vec<_>>()
                                                    .join(", ")
                                            )),
                                            cx,
                                        );
                                    }
                                }
                                Err(error) => state.set_toast(Some(error), cx),
                            }
                        }
                        match (report.messages, report.conversations) {
                            (Ok(messages), Ok(conversations)) => {
                                let has_new_selected_message = state
                                    .selected_conversation
                                    .as_deref()
                                    .map(|selected_id| {
                                        messages.iter().any(|message| {
                                            message.conversation_id == selected_id
                                                && !state
                                                    .messages
                                                    .iter()
                                                    .any(|current| current.id == message.id)
                                        })
                                    })
                                    .unwrap_or(false);
                                if has_new_selected_message {
                                    state.message_scroll_handle.scroll_to_bottom();
                                }
                                if messages != state.messages
                                    || conversations != state.conversations
                                {
                                    state.messages = messages;
                                    state.conversations = conversations;
                                    state_changed = true;
                                }
                            }
                            (Err(error), _) | (_, Err(error)) => state.set_toast(Some(error), cx),
                        }
                        for signal in report.voice_signals {
                            state.handle_voice_signal(signal, cx);
                        }
                        let error_message = (!poll_error.is_empty()).then(|| poll_error.join(" "));
                        if error_message != state.last_poll_error {
                            state.last_poll_error = error_message.clone();
                            if let Some(error_message) = error_message {
                                state.set_toast(Some(error_message), cx);
                            }
                        }
                        if state_changed {
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !keep_polling {
                    break;
                }
                if let Some((transport, identity, friends, store)) = peer_configuration {
                    let result = cx
                        .background_spawn(async move {
                            std::thread::spawn(move || {
                                transport.configure(&identity, &friends, store)
                            })
                            .join()
                            .unwrap_or_else(|_| {
                                Err("The peer configuration worker stopped unexpectedly.".into())
                            })
                        })
                        .await;
                    if let Err(error) = result {
                        let _ = this.update(cx, |state, cx| state.set_toast(Some(error), cx));
                    }
                }
            }
        }));
    }

    fn api_session(&self) -> Result<(AccountApiClient, String), String> {
        let api = self
            .account_api
            .clone()
            .ok_or_else(|| "The MySQL account API is unavailable.".to_string())?;
        let token = self
            .session_token
            .clone()
            .ok_or_else(|| "You must be signed in to use this feature.".to_string())?;
        Ok((api, token))
    }

    fn load_remote_data(&mut self, notify_new_friends: bool, cx: &mut Context<Self>) {
        let (api, token) = match self.api_session() {
            Ok(session) => session,
            Err(error) => {
                self.set_toast(Some(error), cx);
                return;
            }
        };
        if let (Some(identity), Some(store)) = (&self.identity, &self.local_chat) {
            match MlsService::generate_key_packages(
                store,
                &identity.user.id,
                identity.private_key(),
                10,
            ) {
                Ok(key_packages) => {
                    if let Err(error) = api.publish_mls_key_packages(&token, &key_packages) {
                        let status = error
                            .status
                            .map(|status| format!(" (HTTP {status})"))
                            .unwrap_or_default();
                        self.set_toast(
                            Some(format!(
                                "Could not publish encrypted conversation credentials: {}{}",
                                error.message, status
                            )),
                            cx,
                        );
                    }
                }
                Err(error) => self.set_toast(Some(error), cx),
            }
        }
        if let Some(transport) = &self.peer_transport {
            if let Err(error) = api.publish_presence(&token, transport.port()) {
                self.set_toast(
                    Some(format!(
                        "Could not publish the direct-chat endpoint: {}",
                        error.message
                    )),
                    cx,
                );
            }
        } else {
            self.set_toast(
                Some(
                    self.startup_error
                        .clone()
                        .unwrap_or_else(|| "Direct peer networking is unavailable.".into()),
                ),
                cx,
            );
        }
        match api.friends(&token) {
            Ok(friends) => {
                self.friends = friends;
                if let Some(store) = &self.local_chat {
                    match store.sync_friends(&self.friends) {
                        Ok(new_friends) if notify_new_friends && !new_friends.is_empty() => {
                            self.set_toast(
                                Some(format!(
                                    "New friend(s): {}.",
                                    new_friends
                                        .iter()
                                        .map(|friend| friend.name.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                )),
                                cx,
                            );
                        }
                        Ok(_) => {}
                        Err(error) => self.set_toast(Some(error), cx),
                    }
                }
                let configured = match (&self.peer_transport, &self.identity, &self.local_chat) {
                    (Some(transport), Some(identity), Some(store)) => {
                        transport.configure(identity, &self.friends, store.clone())
                    }
                    _ => Ok(()),
                };
                if let Err(error) = configured {
                    self.set_toast(Some(error), cx);
                }
            }
            Err(error) => self.set_toast(Some(error.message), cx),
        }
        match api.friend_requests(&token) {
            Ok(requests) => self.incoming_friend_requests = requests,
            Err(error) => self.set_toast(Some(error.message), cx),
        }
        match api.outgoing_friend_requests(&token) {
            Ok(requests) => self.outgoing_friend_requests = requests,
            Err(error) => self.set_toast(Some(error.message), cx),
        }
        if let Some(store) = &self.local_chat {
            match store.conversations() {
                Ok(conversations) => self.conversations = conversations,
                Err(error) => self.set_toast(Some(error), cx),
            }
        }
        match api.settings(&token) {
            Ok(settings) => {
                self.notifications_enabled = settings
                    .get("notifications")
                    .map(|value| value == "true")
                    .unwrap_or(true);
                self.presence = settings
                    .get("presence")
                    .filter(|value| matches!(value.as_str(), "Online" | "Away" | "Do not disturb"))
                    .cloned()
                    .unwrap_or_else(|| "Online".into());
                if let Some(user) = self.current_user.as_mut() {
                    user.status = self.presence.clone();
                }
            }
            Err(error) => self.set_toast(Some(error.message), cx),
        }
    }

    pub fn add_friend(&mut self, cx: &mut Context<Self>) {
        let username = self.peer_username_input.trim().to_ascii_lowercase();
        if !(3..=32).contains(&username.len())
            || !username
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            self.set_toast(Some("Enter a 3-32 character username.".into()), cx);
            cx.notify();
            return;
        }
        let Some(identity) = self.identity.as_ref() else {
            self.set_toast(Some("Sign in before adding a friend.".into()), cx);
            cx.notify();
            return;
        };
        if username == identity.user.username {
            self.set_toast(Some("You cannot add yourself as a friend.".into()), cx);
            cx.notify();
            return;
        }
        match self.api_session() {
            Ok((api, token)) => match api.send_friend_request(&token, &username) {
                Ok(status) => {
                    self.peer_username_input.clear();
                    let message = match status {
                        crate::services::account_api_client::FriendRequestStatus::Pending => {
                            "Friend request sent."
                        }
                        crate::services::account_api_client::FriendRequestStatus::Accepted => {
                            "Friend request accepted."
                        }
                    };
                    self.set_toast(Some(message.into()), cx);
                    self.load_remote_data(false, cx);
                }
                Err(error) => self.set_toast(Some(error.message), cx),
            },
            Err(error) => self.set_toast(Some(error), cx),
        }
        cx.notify();
    }

    pub fn accept_friend_request(&mut self, peer_id: &str, cx: &mut Context<Self>) {
        match self.api_session() {
            Ok((api, token)) => match api.accept_friend_request(&token, peer_id) {
                Ok(()) => {
                    self.incoming_friend_requests
                        .retain(|request| request.id != peer_id);
                    self.set_toast(Some("Friend request accepted.".into()), cx);
                    self.load_remote_data(false, cx);
                }
                Err(error) => self.set_toast(Some(error.message), cx),
            },
            Err(error) => self.set_toast(Some(error), cx),
        }
        cx.notify();
    }

    pub fn decline_friend_request(&mut self, peer_id: &str, cx: &mut Context<Self>) {
        match self.api_session() {
            Ok((api, token)) => match api.decline_friend_request(&token, peer_id) {
                Ok(()) => {
                    self.incoming_friend_requests
                        .retain(|request| request.id != peer_id);
                }
                Err(error) => self.set_toast(Some(error.message), cx),
            },
            Err(error) => self.set_toast(Some(error), cx),
        }
        cx.notify();
    }

    pub fn cancel_friend_request(&mut self, peer_id: &str, cx: &mut Context<Self>) {
        match self.api_session() {
            Ok((api, token)) => match api.cancel_friend_request(&token, peer_id) {
                Ok(()) => {
                    self.outgoing_friend_requests
                        .retain(|request| request.id != peer_id);
                }
                Err(error) => self.set_toast(Some(error.message), cx),
            },
            Err(error) => self.set_toast(Some(error), cx),
        }
        cx.notify();
    }

    pub fn remove_friend(&mut self, peer_id: &str, cx: &mut Context<Self>) {
        match self.api_session() {
            Ok((api, token)) => match api.remove_friend(&token, peer_id) {
                Ok(()) => {
                    self.set_toast(Some("Friend removed.".into()), cx);
                    self.load_remote_data(false, cx);
                }
                Err(error) => self.set_toast(Some(error.message), cx),
            },
            Err(error) => self.set_toast(Some(error), cx),
        }
        cx.notify();
    }

    pub fn open_or_create_conversation(&mut self, peer_id: &str, cx: &mut Context<Self>) {
        let Some(peer) = self.friend(peer_id).cloned() else {
            self.set_toast(
                Some("This account is no longer in your friend list.".into()),
                cx,
            );
            cx.notify();
            return;
        };
        match self.local_chat.as_ref() {
            Some(store) => match store.conversation_for(&peer) {
                Ok(conversation) => self.open_conversation(&conversation.id, cx),
                Err(error) => self.set_toast(Some(error), cx),
            },
            None => self.set_toast(Some("Local message storage is unavailable.".into()), cx),
        }
        cx.notify();
    }

    pub fn open_conversation_by_id(&mut self, conversation_id: &str, cx: &mut Context<Self>) {
        self.open_conversation(conversation_id, cx);
        cx.notify();
    }

    fn open_conversation(&mut self, conversation_id: &str, cx: &mut Context<Self>) {
        let Some(store) = self.local_chat.clone() else {
            self.set_toast(Some("Local message storage is unavailable.".into()), cx);
            return;
        };
        if let Some(conversation) = self
            .conversation(conversation_id)
            .filter(|conversation| conversation.is_group())
            .cloned()
            && let Err(error) =
                store.create_group(&conversation.id, &conversation.name, &conversation.members)
        {
            self.set_toast(Some(error), cx);
            return;
        }
        let read_message_ids = match store.mark_conversation_read(conversation_id) {
            Ok(message_ids) => message_ids,
            Err(error) => {
                self.set_toast(Some(error), cx);
                return;
            }
        };
        if !read_message_ids.is_empty() {
            match self.api_session() {
                Ok((api, token)) => {
                    for message_id in read_message_ids {
                        if let Err(error) = api.send_message_receipt(&token, &message_id, "read") {
                            self.set_toast(Some(error.message), cx);
                            break;
                        }
                    }
                }
                Err(error) => self.set_toast(Some(error), cx),
            }
        }
        match (store.messages(), store.conversations()) {
            (Ok(messages), Ok(conversations)) => {
                if self.selected_conversation.as_deref() != Some(conversation_id)
                    && self.editing_message_id.take().is_some()
                {
                    self.clear_message_draft();
                }
                self.conversations = conversations;
                self.messages = messages;
                self.selected_conversation = Some(conversation_id.to_string());
                self.message_scroll_handle.scroll_to_bottom();
                self.active_menu = None;
                self.navigation.navigate(Route::Messages);
            }
            (Err(error), _) | (_, Err(error)) => self.set_toast(Some(error), cx),
        }
    }

    pub fn save_profile(&mut self, cx: &mut Context<Self>) {
        let (api, token) = match self.api_session() {
            Ok(session) => session,
            Err(error) => {
                self.set_toast(Some(error), cx);
                cx.notify();
                return;
            }
        };
        let Some(identity) = self.identity.as_mut() else {
            self.set_toast(
                Some("You must be signed in to edit your profile.".into()),
                cx,
            );
            cx.notify();
            return;
        };
        match api.update_profile(&token, &self.profile_name_input) {
            Ok(account) if account.public_key == identity.user.id => {
                match self
                    .identity_service
                    .update_display_name(identity, &account.display_name)
                {
                    Ok(()) => {
                        identity.user.username = account.username;
                        self.current_user = Some(identity.user.clone());
                        self.profile_name_input = identity.user.name.clone();
                        self.set_toast(
                            Some("Profile saved to the MySQL account database.".into()),
                            cx,
                        );
                    }
                    Err(error) => self.set_toast(Some(error.to_string()), cx),
                }
            }
            Ok(_) => self.set_toast(
                Some("The account API returned a different identity.".into()),
                cx,
            ),
            Err(error) => self.set_toast(Some(error.message), cx),
        }
        cx.notify();
    }

    pub fn toggle_notifications(&mut self, cx: &mut Context<Self>) {
        let next = !self.notifications_enabled;
        match self.api_session() {
            Ok((api, token)) => match api.set_setting(&token, "notifications", &next.to_string()) {
                Ok(()) => {
                    self.notifications_enabled = next;
                    self.set_toast(
                        Some("Notification preference saved to the account database.".into()),
                        cx,
                    );
                }
                Err(error) => self.set_toast(Some(error.message), cx),
            },
            Err(error) => self.set_toast(Some(error), cx),
        }
        cx.notify();
    }

    pub fn set_presence(&mut self, presence: &str, cx: &mut Context<Self>) {
        if !matches!(presence, "Online" | "Away" | "Do not disturb") {
            return;
        }
        match self.api_session() {
            Ok((api, token)) => match api.set_setting(&token, "presence", presence) {
                Ok(()) => {
                    self.presence = presence.to_string();
                    if let Some(user) = self.current_user.as_mut() {
                        user.status = presence.to_string();
                    }
                    self.set_toast(Some("Presence preference saved.".into()), cx);
                }
                Err(error) => self.set_toast(Some(error.message), cx),
            },
            Err(error) => self.set_toast(Some(error), cx),
        }
        cx.notify();
    }

    pub fn logout(&mut self, cx: &mut Context<Self>) {
        if let Some(transport) = &self.peer_transport {
            transport.clear();
        }
        self.active_voice_call = None;
        self.incoming_voice_call = None;
        self.voice_recorder = None;
        self.voice_recording_conversation = None;
        let presence_error = match (&self.account_api, &self.session_token) {
            (Some(api), Some(token)) => api.clear_presence(token).err().map(|error| error.message),
            _ => None,
        };
        let revoke_error = match (&self.account_api, &self.session_token) {
            (Some(api), Some(token)) => api.logout(token).err().map(|error| error.message),
            _ => None,
        };
        self.incoming_poll_task = None;
        self.last_poll_error = None;
        self.pending_attachments.clear();
        self.attachment_uploads_in_progress = 0;
        self.attachment_transfer_states.clear();
        self.attachment_upload_monitor_ids.clear();
        self.attachment_upload_monitor_active = false;
        self.image_preview = None;
        self.image_preview_loading = None;
        self.audio_player = None;
        self.audio_playback_id = None;
        self.audio_loading_attachment_id = None;
        self.active_message_send_id = None;
        self.identity = None;
        self.session_token = None;
        self.local_chat = None;
        self.current_user = None;
        self.current_device_id = None;
        self.registered_devices.clear();
        self.pending_device_enrollments.clear();
        self.device_enrollment_status = None;
        self.device_pairing_code = None;
        self.device_pairing_verification_input.clear();
        self.friends.clear();
        self.conversations.clear();
        self.messages.clear();
        self.create_group_modal_open = false;
        self.manage_group_id = None;
        self.group_name_input.clear();
        self.selected_group_members.clear();
        self.selected_conversation = None;
        self.username_input.clear();
        self.display_name_input.clear();
        self.password_input.clear();
        self.profile_name_input.clear();
        self.current_password_input.clear();
        self.new_password_input.clear();
        self.confirm_password_input.clear();
        self.peer_username_input.clear();
        self.clear_message_draft();
        self.editing_message_id = None;
        self.activate_input(InputField::Username);
        self.notifications_enabled = true;
        self.presence = "Online".into();
        self.set_toast(
            presence_error
                .or(revoke_error)
                .map(|error| format!("Logged out locally; server cleanup failed: {error}")),
            cx,
        );
        self.auth_mode = AuthMode::Login;
        cx.notify();
    }
}

fn grapheme_offset_to_byte(text: &str, grapheme_offset: usize) -> usize {
    text.grapheme_indices(true)
        .nth(grapheme_offset)
        .map(|(byte_offset, _)| byte_offset)
        .unwrap_or(text.len())
}

fn sanitized_group_member(mut user: User) -> User {
    user.endpoint = None;
    user.status = "Offline".into();
    user
}

fn attachment_transfer_state_for_result<T>(result: &Result<T, String>) -> AttachmentTransferState {
    match result {
        Ok(_) => AttachmentTransferState::Completed,
        Err(error) if error == "Attachment transfer was cancelled." => {
            AttachmentTransferState::Cancelled
        }
        Err(_) => AttachmentTransferState::Failed,
    }
}

#[cfg(test)]
fn received_message_content(body: &str) -> Result<(String, Vec<AttachmentMetadata>), String> {
    if let Some((message_body, attachments)) = attachment::decode_message_payload(body)? {
        if message_body
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r'))
        {
            return Err("The received message is invalid.".into());
        }
        return Ok((message_body, attachments));
    }
    if body.trim().is_empty()
        || body
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r'))
    {
        return Err("The received message is invalid.".into());
    }
    Ok((body.to_string(), Vec::new()))
}

fn upload_attachment_data(
    destination: AttachmentDestination,
    contents: Zeroizing<Vec<u8>>,
    file_name: String,
    content_type: String,
    kind: AttachmentKind,
    attachment_id: String,
) -> Result<AttachmentMetadata, String> {
    let file_name: String = file_name
        .chars()
        .filter(|character| !character.is_control())
        .take(255)
        .collect();
    if file_name.trim().is_empty() {
        return Err("The selected file has no valid name.".into());
    }
    if contents.is_empty() || contents.len() > attachment::max_attachment_bytes()? {
        return Err("The file is empty or exceeds the configured attachment size limit.".into());
    }
    let file_name = if kind == AttachmentKind::Voice {
        format!("voice-note-{}.wav", &attachment_id[..8])
    } else {
        file_name
    };
    let (key, nonce, ciphertext) =
        attachment::encrypt_bound(&attachment_id, &contents, &destination.binding)?;
    let metadata = AttachmentMetadata {
        attachment_id: attachment_id.clone(),
        file_name,
        content_type,
        size: contents.len(),
        kind,
        key,
        nonce,
        binding: Some(destination.binding),
    };
    attachment::validate_metadata(&metadata)?;
    destination
        .store
        .save_attachment_ciphertext(&attachment_id, &ciphertext)?;
    Ok(metadata)
}

fn read_attachment_file(
    path: &std::path::Path,
    max_bytes: usize,
) -> Result<Zeroizing<Vec<u8>>, String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("Could not read the selected file: {error}"))?;
    let file_size = file
        .metadata()
        .map_err(|error| format!("Could not inspect the selected file: {error}"))?
        .len();
    if file_size == 0 || file_size > max_bytes as u64 {
        return Err("The file is empty or exceeds the configured attachment size limit.".into());
    }
    let read_limit = (max_bytes as u64).saturating_add(1);
    let mut contents = Vec::with_capacity(file_size as usize);
    file.take(read_limit)
        .read_to_end(&mut contents)
        .map_err(|error| format!("Could not read the selected file: {error}"))?;
    if contents.len() > max_bytes {
        return Err("The file exceeds the configured attachment size limit.".into());
    }
    Ok(Zeroizing::new(contents))
}

fn save_attachment_without_overwrite(
    path: &std::path::Path,
    plaintext: &[u8],
) -> Result<(), String> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                format!(
                    "The file {} already exists; choose a different name.",
                    path.display()
                )
            } else {
                format!("Could not create the attachment file: {error}")
            }
        })?;
    let result = file
        .write_all(plaintext)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("Could not save the attachment: {error}"));
    if result.is_err() {
        drop(file);
        std::fs::remove_file(path)
            .map_err(|error| format!("Could not clean up an incomplete attachment: {error}"))?;
    }
    result
}

fn fetch_decrypted_attachment(
    store: &LocalChatStore,
    source: Option<&(PeerTransport, User, String)>,
    metadata: &AttachmentMetadata,
    cancelled: &AtomicBool,
    progress: &AttachmentTransferProgress,
) -> Result<Zeroizing<Vec<u8>>, String> {
    let ciphertext = if store.has_attachment_ciphertext(&metadata.attachment_id)? {
        let ciphertext = store.read_attachment_ciphertext(&metadata.attachment_id)?;
        progress.record(ciphertext.len() as u64, ciphertext.len() as u64);
        ciphertext
    } else {
        match source {
            Some((transport, peer, conversation_id)) => transport.download_attachment(
                peer,
                conversation_id,
                metadata,
                cancelled,
                |transferred, total| progress.record(transferred, total),
            )?,
            None => {
                let ciphertext = store.read_attachment_ciphertext(&metadata.attachment_id)?;
                progress.record(ciphertext.len() as u64, ciphertext.len() as u64);
                ciphertext
            }
        }
    };
    let plaintext = attachment::decrypt(metadata, &ciphertext)?;
    if let Some((transport, peer, conversation_id)) = source {
        if let Err(error) = transport.acknowledge_attachment(peer, conversation_id, metadata) {
            eprintln!(
                "Could not report verified attachment {} to @{}: {error}",
                metadata.attachment_id, peer.username
            );
        }
    }
    Ok(plaintext)
}

fn attachment_content_type(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("wav") => "audio/wav",
        Some("mp3") => "audio/mpeg",
        Some("ogg") => "audio/ogg",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("tif" | "tiff") => "image/tiff",
        Some("flac") => "audio/flac",
        Some("pdf") => "application/pdf",
        Some("txt") => "text/plain",
        _ => "application/octet-stream",
    }
}

fn attachment_download_directory() -> PathBuf {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let downloads = home.join("Downloads");
    if downloads.is_dir() { downloads } else { home }
}

fn voice_route_id<'a>(call_id: &'a str, conversation: &'a Conversation) -> &'a str {
    if conversation.is_group() {
        &conversation.id
    } else {
        call_id
    }
}

fn should_initiate_voice_mesh_link(local_id: &str, peer_id: &str) -> bool {
    local_id < peer_id
}

fn transmit_voice_signal(
    api: AccountApiClient,
    token: String,
    private_key: Zeroizing<Vec<u8>>,
    action: VoiceSignalAction,
    call_id: String,
    group_id: Option<String>,
    initiator_id: String,
    targets: Vec<VoiceSignalTarget>,
    description: Option<String>,
    candidate: Option<String>,
) -> Result<(), String> {
    let sent_at = timestamp()?;
    let mut failures = Vec::new();
    let conversation_id = group_id.clone().unwrap_or_else(|| call_id.clone());
    for target in targets {
        let VoiceSignalTarget { peer, participants } = target;
        let signal = VoiceSignal {
            call_id: call_id.clone(),
            conversation_id: conversation_id.clone(),
            initiator_id: initiator_id.clone(),
            action,
            participants,
            description: description.clone(),
            candidate: candidate.clone(),
        };
        let result = (|| {
            let body = signal.encode()?;
            let message_id = new_id()?;
            let envelope = relay::encrypt(
                private_key.as_slice(),
                &peer.id,
                &peer.id,
                &message_id,
                &sent_at,
                group_id.as_deref(),
                &body,
            )?;
            api.send_relay_message(&token, &peer.id, group_id.as_deref(), &envelope)
                .map_err(|error| error.message)
        })();
        if let Err(error) = result {
            failures.push(format!("@{}: {error}", peer.username));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "Call signaling failed for conversation {conversation_id}: {}",
            failures.join("; ")
        ))
    }
}

fn next_auth_input(current: InputField, mode: AuthMode, reverse: bool) -> InputField {
    let index = match (mode, current) {
        (AuthMode::Login, InputField::Username) => 0,
        (AuthMode::Login, InputField::Password) => 1,
        (AuthMode::Login, InputField::RememberMe) => 2,
        (AuthMode::Register, InputField::Username) => 0,
        (AuthMode::Register, InputField::DisplayName) => 1,
        (AuthMode::Register, InputField::InviteCode) => 2,
        (AuthMode::Register, InputField::Password) => 3,
        _ => 0,
    };
    let next = if reverse {
        let field_count = if mode == AuthMode::Register { 4 } else { 3 };
        (index + field_count - 1) % field_count
    } else {
        let field_count = if mode == AuthMode::Register { 4 } else { 3 };
        (index + 1) % field_count
    };
    match (mode, next) {
        (AuthMode::Login, 0) | (AuthMode::Register, 0) => InputField::Username,
        (AuthMode::Register, 1) => InputField::DisplayName,
        (AuthMode::Register, 2) => InputField::InviteCode,
        (AuthMode::Login, 1) | (AuthMode::Register, 3) => InputField::Password,
        (AuthMode::Login, 2) => InputField::RememberMe,
        _ => unreachable!(),
    }
}

fn is_missing_credentials_error(error: &str) -> bool {
    error.contains("0x80070490")
}

fn sync_account_profile(
    service: &IdentityService,
    identity: &mut UnlockedIdentity,
    account: &AccountUser,
) -> Result<(), String> {
    if account.public_key != identity.user.id {
        return Err("The account API identity does not match the unlocked key.".into());
    }
    if account.display_name != identity.user.name {
        service
            .update_display_name(identity, &account.display_name)
            .map_err(|error| error.to_string())?;
    }
    identity.user.username = account.username.clone();
    identity.user.profile_image = account.profile_image.clone();
    Ok(())
}

impl Render for AppState {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        if let Some(value) = self.pending_message_input_value.take() {
            self.message_input
                .update(cx, |input, cx| input.set_value(value, cx));
        }
        if self.current_user.is_none() {
            crate::views::auth::render(self, window, cx).into_any_element()
        } else {
            crate::layout::app_shell::render(self, window, cx).into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AuthMode, GROUP_MEMBERSHIP_PREFIX, GroupMembershipCommit, GroupRelayPayload, InputField,
        PendingConfirmation, encode_group_payload, encode_membership_commit_payload,
        is_missing_credentials_error, next_auth_input, peer_transport_friends_changed,
        received_message_content, save_attachment_without_overwrite,
        should_initiate_voice_mesh_link, validate_group_members, validate_membership_roster,
        voice_route_id,
    };
    use crate::models::conversation::Conversation;
    use crate::models::user::User;
    use crate::services::attachment::{self, AttachmentKind, AttachmentMetadata};

    #[test]
    fn destructive_confirmation_targets_are_distinct() {
        let remove_friend = PendingConfirmation::RemoveFriend("friend-id".into());
        let delete_conversation = PendingConfirmation::DeleteConversation("conversation-id".into());
        let revoke_device = PendingConfirmation::RevokeDevice("device-id".into());

        assert_ne!(remove_friend, delete_conversation);
        assert_ne!(remove_friend, revoke_device);
        assert_ne!(delete_conversation, revoke_device);
    }

    #[test]
    fn peer_transport_only_reconfigures_for_network_relevant_friend_changes() {
        let friend = User::new("friend-id", "friend", "Friend", "F", 0, "Online");
        let mut changed_presence = friend.clone();
        changed_presence.status = "Offline".into();
        let mut changed_endpoint = friend.clone();
        changed_endpoint.endpoint = Some("127.0.0.1:1234".into());

        assert!(!peer_transport_friends_changed(
            std::slice::from_ref(&friend),
            &[changed_presence]
        ));
        assert!(peer_transport_friends_changed(
            std::slice::from_ref(&friend),
            &[changed_endpoint]
        ));
        assert!(peer_transport_friends_changed(
            std::slice::from_ref(&friend),
            &[]
        ));
    }

    #[test]
    fn saving_an_attachment_never_overwrites_an_existing_file() {
        let directory = std::env::temp_dir().join(format!(
            "resonance-attachment-save-{}",
            crate::services::local_chat_store::new_id().unwrap()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("existing.txt");
        std::fs::write(&path, b"keep this file").unwrap();

        assert!(save_attachment_without_overwrite(&path, b"replace").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"keep this file");

        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn voice_routes_are_stable_across_direct_and_group_devices() {
        let direct = Conversation::new("device-local-id", "peer", "Peer", "", "1");
        assert_eq!(voice_route_id("shared-call-id", &direct), "shared-call-id");

        let group_id = "a".repeat(32);
        let mut group = Conversation::new(&group_id, "self", "Group", "", "1");
        group.members.push(User::new(
            &"b".repeat(64),
            "member",
            "Member",
            "M",
            0,
            "Online",
        ));
        assert_eq!(voice_route_id("ignored-call-id", &group), group_id);
    }

    #[test]
    fn group_mesh_uses_one_deterministic_offer_per_peer_pair() {
        assert!(should_initiate_voice_mesh_link("a", "b"));
        assert!(!should_initiate_voice_mesh_link("b", "a"));
        assert!(!should_initiate_voice_mesh_link("a", "a"));
    }

    #[test]
    fn auth_fields_follow_tab_order_in_both_directions() {
        assert_eq!(
            next_auth_input(InputField::Username, AuthMode::Login, false),
            InputField::Password
        );
        assert_eq!(
            next_auth_input(InputField::Password, AuthMode::Login, false),
            InputField::RememberMe
        );
        assert_eq!(
            next_auth_input(InputField::RememberMe, AuthMode::Login, false),
            InputField::Username
        );
        assert_eq!(
            next_auth_input(InputField::Password, AuthMode::Register, true),
            InputField::InviteCode
        );
        assert_eq!(
            next_auth_input(InputField::InviteCode, AuthMode::Register, false),
            InputField::Password
        );
        assert_eq!(
            next_auth_input(InputField::InviteCode, AuthMode::Register, true),
            InputField::DisplayName
        );
        assert_eq!(
            next_auth_input(InputField::Password, AuthMode::Register, false),
            InputField::Username
        );
    }

    #[test]
    fn removing_missing_saved_credentials_is_not_an_error() {
        assert!(is_missing_credentials_error(
            "Element nicht gefunden. (0x80070490)"
        ));
        assert!(!is_missing_credentials_error("Access denied. (0x80070005)"));
    }

    #[test]
    fn group_invites_require_sender_and_recipient_in_the_member_list() {
        let members = vec![
            User::new(&"1".repeat(64), "owner", "Owner", "O", 1, "Online"),
            User::new(&"2".repeat(64), "alice", "Alice", "A", 2, "Online"),
            User::new(&"3".repeat(64), "bob", "Bob", "B", 3, "Online"),
        ];
        let payload = encode_group_payload(&GroupRelayPayload::Invite {
            conversation_id: "0123456789abcdef0123456789abcdef".into(),
            name: "Project".into(),
            members: members.clone(),
            membership_version: 1,
            welcome: "encrypted-welcome".into(),
        })
        .unwrap();
        assert!(matches!(
            super::decode_group_payload(&payload).unwrap(),
            Some(GroupRelayPayload::Invite { .. })
        ));
        assert!(
            validate_group_members(
                "0123456789abcdef0123456789abcdef",
                "Project",
                &members,
                &members[0].id,
                &members[1].id
            )
            .is_ok()
        );
        assert!(
            validate_group_members(
                "0123456789abcdef0123456789abcdef",
                "Project",
                &members,
                &members[0].id,
                &"4".repeat(64)
            )
            .is_err()
        );
    }

    #[test]
    fn membership_events_allow_removed_recipients_but_validate_the_new_roster() {
        let members = vec![
            User::new(&"1".repeat(64), "owner", "Owner", "O", 1, "Online"),
            User::new(&"2".repeat(64), "first", "First", "F", 2, "Online"),
            User::new(&"4".repeat(64), "added", "Added", "A", 4, "Online"),
        ];
        let payload = encode_membership_commit_payload(
            "0123456789abcdef0123456789abcdef",
            2,
            &members,
            "encrypted-commit",
        )
        .unwrap();
        let event: GroupMembershipCommit =
            serde_json::from_str(payload.strip_prefix(GROUP_MEMBERSHIP_PREFIX).unwrap()).unwrap();
        assert_eq!(event.membership_version, 2);
        assert_eq!(event.members, members);
        assert!(
            validate_membership_roster(
                "0123456789abcdef0123456789abcdef",
                "Project",
                &event.members
            )
            .is_ok()
        );

        let mut duplicate_members = members;
        duplicate_members[2].id = duplicate_members[1].id.clone();
        assert!(
            validate_membership_roster(
                "0123456789abcdef0123456789abcdef",
                "Project",
                &duplicate_members
            )
            .is_err()
        );
    }

    #[test]
    fn incoming_attachment_payload_is_decoded_for_the_attachment_card() {
        let attachment_id = "0123456789abcdef0123456789abcdef";
        let (key, nonce, _) = attachment::encrypt(attachment_id, b"voice").unwrap();
        let metadata = AttachmentMetadata {
            attachment_id: attachment_id.into(),
            file_name: "voice-message.wav".into(),
            content_type: "audio/wav".into(),
            size: 5,
            kind: AttachmentKind::Voice,
            key,
            nonce,
            binding: None,
        };
        let body = attachment::encode_relay_payload(&metadata).unwrap();
        let (display_body, decoded) = received_message_content(&body).unwrap();
        assert_eq!(display_body, "");
        assert_eq!(decoded, vec![metadata.clone()]);
        assert_eq!(
            received_message_content("ordinary message").unwrap(),
            ("ordinary message".into(), Vec::new())
        );
        let composed = attachment::encode_message_payload("caption", &[metadata.clone()]).unwrap();
        assert_eq!(
            received_message_content(&composed).unwrap(),
            ("caption".into(), vec![metadata])
        );
        assert!(received_message_content(" \n ").is_err());
    }
}
