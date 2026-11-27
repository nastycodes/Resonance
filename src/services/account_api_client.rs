use std::{
    collections::HashMap,
    env,
    net::{IpAddr, SocketAddr},
    time::Duration,
};

use reqwest::blocking::{Client, RequestBuilder};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    models::user::User,
    services::{
        device_enrollment::{
            EncryptedHistoryPackage, device_public_keys, device_secret_hash,
            enrollment_approval_data, enrollment_request_data, sign_device_data, verify_signature,
        },
        identity::EncryptedIdentity,
        local_chat_store::DeviceIdentity,
        relay::RelayEnvelope,
    },
    theme::colors,
};

#[derive(Clone)]
pub struct AccountApiClient {
    base_url: reqwest::Url,
    http: Client,
    device_auth: Option<DeviceAuth>,
}

#[derive(Clone)]
struct DeviceAuth {
    id: String,
    secret: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct AccountUser {
    pub username: String,
    pub display_name: String,
    pub public_key: String,
    #[serde(default)]
    pub profile_image: Option<String>,
}

#[derive(Serialize)]
struct InitializeIdentityRequest<'a> {
    public_key: &'a str,
    encrypted_identity: EncryptedIdentityPayload,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountSession {
    pub access_token: String,
    pub account: AccountUser,
    pub identity: Option<EncryptedIdentity>,
    pub identity_error: Option<String>,
    pub identity_missing: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountApiError {
    pub status: Option<u16>,
    pub message: String,
}

#[derive(Serialize)]
struct RegistrationRequest<'a> {
    username: &'a str,
    display_name: &'a str,
    password: &'a str,
    invite_code: &'a str,
    public_key: &'a str,
    encrypted_identity: EncryptedIdentityPayload,
}

#[derive(Serialize)]
struct LoginRequest<'a> {
    username: &'a str,
    password: &'a str,
}

#[derive(Serialize)]
struct ProfileRequest<'a> {
    display_name: &'a str,
}

#[derive(Serialize)]
struct ProfileImageRequest<'a> {
    profile_image: Option<&'a str>,
}

#[derive(Serialize)]
struct PasswordChangeRequest<'a> {
    current_password: &'a str,
    new_password: &'a str,
    public_key: &'a str,
    encrypted_identity: EncryptedIdentityPayload,
}

#[derive(Serialize)]
struct AddFriendRequest<'a> {
    username: &'a str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FriendRequestStatus {
    Pending,
    Accepted,
}

#[derive(Deserialize)]
struct FriendRequestResponse {
    status: FriendRequestStatus,
}

#[derive(Serialize)]
struct PresenceRequest {
    port: u16,
}

#[derive(Serialize)]
struct RelayMessageRequest<'a> {
    recipient_public_key: &'a str,
    group_id: Option<&'a str>,
    envelope: &'a RelayEnvelope,
}

#[derive(Serialize)]
struct CreateGroupRequest<'a> {
    group_id: &'a str,
    member_public_keys: &'a [String],
}

#[derive(Serialize)]
struct UpdateGroupMembersRequest<'a> {
    operation_id: &'a str,
    expected_version: u64,
    member_public_keys: &'a [String],
    deliveries: &'a [crate::services::relay::RelayDelivery],
}

#[derive(Deserialize)]
struct GroupRosterResponse {
    group_id: String,
    version: u64,
    owner_public_key: String,
    members: Vec<AccountUser>,
}

#[derive(Clone, Debug)]
pub struct GroupRoster {
    pub group_id: String,
    pub version: u64,
    pub owner_public_key: String,
    pub members: Vec<AccountUser>,
}

#[derive(Serialize)]
struct MlsKeyPackagesPublishRequest<'a> {
    key_packages: &'a [String],
}

#[derive(Serialize)]
struct MlsKeyPackagesClaimRequest<'a> {
    account_public_keys: &'a [String],
}

#[derive(Clone, Debug, Deserialize)]
pub struct ClaimedMlsKeyPackage {
    pub account_public_key: String,
    pub key_package: String,
}

#[derive(Deserialize)]
struct MlsKeyPackagesClaimResponse {
    key_packages: Vec<ClaimedMlsKeyPackage>,
}

#[derive(Serialize)]
struct MessageReceiptRequest<'a> {
    state: &'a str,
}

#[derive(Serialize)]
struct DeviceRegistrationRequest<'a> {
    device_id: &'a str,
    device_secret: &'a str,
    public_key: &'a str,
    signing_public_key: &'a str,
    encryption_public_key: &'a str,
    device_name: &'a str,
    signature: &'a str,
}

#[derive(Serialize)]
struct DeviceEnrollmentRequest<'a> {
    request_id: &'a str,
    nonce: &'a str,
    device_id: &'a str,
    device_secret: &'a str,
    public_key: &'a str,
    signing_public_key: &'a str,
    encryption_public_key: &'a str,
    device_name: &'a str,
    signature: &'a str,
}

#[derive(Serialize)]
struct DeviceEnrollmentApprovalRequest<'a> {
    signature: &'a str,
}

#[derive(Serialize)]
struct DeviceRenameRequest<'a> {
    name: &'a str,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DeviceEnrollmentStatus {
    pub request_id: String,
    pub state: String,
    pub expires_at: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PendingDeviceEnrollment {
    pub request_id: String,
    pub device_id: String,
    pub device_name: String,
    pub device_secret_hash: String,
    pub signing_public_key: String,
    pub encryption_public_key: String,
    pub nonce: String,
    pub request_signature: String,
    pub created_at: String,
    pub expires_at: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct DeviceHistoryRecord {
    #[serde(flatten)]
    pub package: EncryptedHistoryPackage,
    pub created_at: String,
}

#[derive(Serialize)]
struct MessageAcknowledgementRequest<'a> {
    lease_token: &'a str,
    state: &'a str,
}

#[derive(Serialize)]
struct MessageReleaseRequest<'a> {
    lease_token: &'a str,
    failure_code: &'a str,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MessageReceipt {
    pub message_id: String,
    pub delivered_at: Option<String>,
    pub read_at: Option<String>,
    #[serde(default)]
    pub recipient_public_key: Option<String>,
    #[serde(default)]
    pub delivery_state: Option<String>,
    #[serde(default)]
    pub attempts: Option<u32>,
    #[serde(default)]
    pub next_retry_at: Option<String>,
    #[serde(default)]
    pub failure_code: Option<String>,
    #[serde(default)]
    pub device_available: Option<bool>,
    #[serde(default)]
    pub active_devices: Option<u32>,
    #[serde(default)]
    pub pending_devices: Option<u32>,
    #[serde(default)]
    pub delivered_devices: Option<u32>,
    #[serde(default)]
    pub failed_devices: Option<u32>,
    #[serde(default)]
    pub rejected_devices: Option<u32>,
    #[serde(default)]
    pub expired_devices: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct PendingRelayMessage {
    pub sender_public_key: String,
    pub envelope: RelayEnvelope,
    pub lease_token: String,
    #[serde(default)]
    pub queue_sequence: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RegisteredDevice {
    pub device_id: String,
    pub device_name: String,
    pub created_at: String,
    pub last_seen_at: String,
    pub revoked_at: Option<String>,
    pub signing_public_key: Option<String>,
    pub encryption_public_key: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct EncryptedIdentityPayload {
    salt: String,
    nonce: String,
    encrypted_private_key: String,
}

#[derive(Deserialize)]
struct SessionResponse {
    access_token: String,
    account: AccountUser,
    encrypted_identity: Option<EncryptedIdentityPayload>,
}

#[derive(Deserialize)]
struct ApiErrorBody {
    error: Option<String>,
    request_id: Option<String>,
}

#[derive(Deserialize)]
struct ApiFriend {
    public_key: String,
    username: String,
    display_name: String,
    #[serde(default)]
    profile_image: Option<String>,
    endpoint_ip: Option<String>,
    endpoint_port: Option<u16>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SettingsResponse {
    Object(HashMap<String, String>),
    Sequence(Vec<serde_json::Value>),
}

impl AccountApiClient {
    pub fn from_env() -> Result<Self, AccountApiError> {
        let packaged_default = option_env!("RESONANCE_DEFAULT_API_URL").map(str::to_string);
        let base_url = env::var("RESONANCE_API_URL").ok().or(packaged_default);
        let base_url = base_url.ok_or_else(|| AccountApiError {
                status: None,
                message: "This app build has no server configured. Set RESONANCE_DEFAULT_API_URL when building the release.".into(),
            })?;
        Self::new(&base_url)
    }

    pub fn new(base_url: &str) -> Result<Self, AccountApiError> {
        let parsed = reqwest::Url::parse(base_url).map_err(|error| AccountApiError {
            status: None,
            message: format!("The configured account API URL is invalid: {error}"),
        })?;
        let host = parsed.host_str().ok_or_else(|| AccountApiError {
            status: None,
            message: "The configured account API URL must include a host.".into(),
        })?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(AccountApiError {
                status: None,
                message: "The configured account API URL must use HTTP or HTTPS.".into(),
            });
        }
        if parsed.query().is_some() || parsed.fragment().is_some() {
            return Err(AccountApiError {
                status: None,
                message:
                    "The configured account API URL must be the PHP API file URL without a query or fragment."
                        .into(),
            });
        }
        let loopback = host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<IpAddr>()
                .map(|address| address.is_loopback())
                .unwrap_or(false);
        if parsed.scheme() == "http" && !loopback {
            return Err(AccountApiError {
                status: None,
                message: "Use HTTPS for non-local account API URLs.".into(),
            });
        }
        let http = build_http_client(Duration::from_secs(10))?;
        Ok(Self {
            base_url: parsed,
            http,
            device_auth: None,
        })
    }

    pub(crate) fn independent_client(&self) -> Result<Self, AccountApiError> {
        let mut client = Self::new(self.base_url.as_str())?;
        client.device_auth = self.device_auth.clone();
        Ok(client)
    }

    pub(crate) fn without_device_auth(&self) -> Self {
        let mut client = self.clone();
        client.device_auth = None;
        client
    }

    pub fn has_device_auth(&self) -> bool {
        self.device_auth.is_some()
    }

    pub fn with_device_identity(&self, device: &DeviceIdentity) -> Self {
        let mut client = self.clone();
        client.device_auth = Some(DeviceAuth {
            id: device.id.to_ascii_lowercase(),
            secret: device.secret.to_ascii_lowercase(),
        });
        client
    }

    pub fn register_device(
        &self,
        token: &str,
        device: &DeviceIdentity,
        account_public_key: &str,
        identity_private_key: &[u8],
    ) -> Result<Self, AccountApiError> {
        let account_public_key = account_public_key.to_ascii_lowercase();
        let device_id = device.id.to_ascii_lowercase();
        let device_secret = device.secret.to_ascii_lowercase();
        if !is_hex_identifier(&account_public_key, 64)
            || !is_hex_identifier(&device_id, 32)
            || !is_hex_identifier(&device_secret, 64)
        {
            return Err(AccountApiError {
                status: None,
                message: "The local device identity is invalid.".into(),
            });
        }
        let (signing_public_key, encryption_public_key) =
            device_public_keys(device).map_err(|message| AccountApiError {
                status: None,
                message,
            })?;
        let signer =
            Ed25519KeyPair::from_pkcs8(identity_private_key).map_err(|_| AccountApiError {
                status: None,
                message: "The account identity key cannot authenticate this device.".into(),
            })?;
        if encode_hex(signer.public_key().as_ref()) != account_public_key {
            return Err(AccountApiError {
                status: None,
                message: "The account identity key does not match the device account.".into(),
            });
        }
        let signed = [
            b"Resonance device registration v2\0".as_slice(),
            account_public_key.as_bytes(),
            b"\0",
            device_id.as_bytes(),
            b"\0",
            device_secret.as_bytes(),
            b"\0",
            signing_public_key.as_bytes(),
            b"\0",
            encryption_public_key.as_bytes(),
            b"\0",
            device.name.as_bytes(),
        ]
        .concat();
        let signature = encode_hex(signer.sign(&signed).as_ref());
        let response = self
            .authorized(self.http.post(self.url("/v1/devices/register")), token)
            .json(&DeviceRegistrationRequest {
                device_id: &device_id,
                device_secret: &device_secret,
                public_key: &account_public_key,
                signing_public_key: &signing_public_key,
                encryption_public_key: &encryption_public_key,
                device_name: &device.name,
                signature: &signature,
            })
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(response_error(status.as_u16(), response));
        }
        let mut registered = self.clone();
        registered.device_auth = Some(DeviceAuth {
            id: device_id,
            secret: device_secret,
        });
        Ok(registered)
    }

    pub fn begin_device_enrollment(
        &self,
        token: &str,
        device: &DeviceIdentity,
        account_public_key: &str,
    ) -> Result<DeviceEnrollmentStatus, AccountApiError> {
        let account_public_key = account_public_key.to_ascii_lowercase();
        let device_id = device.id.to_ascii_lowercase();
        let device_secret = device.secret.to_ascii_lowercase();
        let request_id = device.enrollment_request_id.to_ascii_lowercase();
        let nonce = device.enrollment_nonce.to_ascii_lowercase();
        let (signing_public_key, encryption_public_key) =
            device_public_keys(device).map_err(|message| AccountApiError {
                status: None,
                message,
            })?;
        if !is_hex_identifier(&account_public_key, 64)
            || !is_hex_identifier(&device_id, 32)
            || !is_hex_identifier(&device_secret, 64)
            || !is_hex_identifier(&request_id, 32)
            || !is_hex_identifier(&nonce, 64)
        {
            return Err(AccountApiError {
                status: None,
                message: "The local device enrollment identity is invalid.".into(),
            });
        }
        let signed = enrollment_request_data(
            &account_public_key,
            &device_id,
            &device_secret_hash(&device_secret).map_err(|message| AccountApiError {
                status: None,
                message,
            })?,
            &signing_public_key,
            &encryption_public_key,
            &request_id,
            &nonce,
            &device.name,
        );
        let signature = sign_device_data(device, &signed).map_err(|message| AccountApiError {
            status: None,
            message,
        })?;
        self.decode(
            self.authorized(self.http.post(self.url("/v1/devices/enrollments")), token)
                .json(&DeviceEnrollmentRequest {
                    request_id: &request_id,
                    nonce: &nonce,
                    device_id: &device_id,
                    device_secret: &device_secret,
                    public_key: &account_public_key,
                    signing_public_key: &signing_public_key,
                    encryption_public_key: &encryption_public_key,
                    device_name: &device.name,
                    signature: &signature,
                })
                .send(),
        )
    }

    pub fn device_enrollment_status(
        &self,
        token: &str,
        request_id: &str,
    ) -> Result<DeviceEnrollmentStatus, AccountApiError> {
        self.decode(
            self.authorized(
                self.http
                    .get(self.url(&format!("/v1/devices/enrollments/{request_id}"))),
                token,
            )
            .send(),
        )
    }

    pub fn registered_devices(
        &self,
        token: &str,
    ) -> Result<Vec<RegisteredDevice>, AccountApiError> {
        self.decode(
            self.device_authorized(self.http.get(self.url("/v1/devices")), token)?
                .send(),
        )
    }

    pub fn pending_device_enrollments(
        &self,
        token: &str,
    ) -> Result<Vec<PendingDeviceEnrollment>, AccountApiError> {
        self.decode(
            self.device_authorized(self.http.get(self.url("/v1/devices/enrollments")), token)?
                .send(),
        )
    }

    pub fn approve_device_enrollment(
        &self,
        token: &str,
        enrollment: &PendingDeviceEnrollment,
        device: &DeviceIdentity,
        account_public_key: &str,
    ) -> Result<(), AccountApiError> {
        let account_public_key = account_public_key.to_ascii_lowercase();
        let request_proof = enrollment_request_data(
            &account_public_key,
            &enrollment.device_id,
            &enrollment.device_secret_hash,
            &enrollment.signing_public_key,
            &enrollment.encryption_public_key,
            &enrollment.request_id,
            &enrollment.nonce,
            &enrollment.device_name,
        );
        if !verify_signature(
            &enrollment.signing_public_key,
            &request_proof,
            &enrollment.request_signature,
        ) {
            return Err(AccountApiError {
                status: Some(403),
                message: "The enrollment request signature is invalid.".into(),
            });
        }
        let signature = sign_device_data(
            device,
            &enrollment_approval_data(
                &account_public_key,
                &enrollment.request_id,
                &enrollment.device_id,
                &enrollment.signing_public_key,
                &enrollment.encryption_public_key,
                &enrollment.nonce,
                &enrollment.request_signature,
            ),
        )
        .map_err(|message| AccountApiError {
            status: None,
            message,
        })?;
        let response = self
            .device_authorized(
                self.http.post(self.url(&format!(
                    "/v1/devices/enrollments/{}/approve",
                    enrollment.request_id
                ))),
                token,
            )?
            .json(&DeviceEnrollmentApprovalRequest {
                signature: &signature,
            })
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(response_error(status.as_u16(), response))
        }
    }

    pub fn rename_device(
        &self,
        token: &str,
        device_id: &str,
        name: &str,
    ) -> Result<(), AccountApiError> {
        let response = self
            .device_authorized(
                self.http
                    .post(self.url(&format!("/v1/devices/{device_id}/rename"))),
                token,
            )?
            .json(&DeviceRenameRequest { name })
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(response_error(status.as_u16(), response))
        }
    }

    pub fn send_history_package(
        &self,
        token: &str,
        package: &EncryptedHistoryPackage,
    ) -> Result<(), AccountApiError> {
        let response = self
            .device_authorized(self.http.post(self.url("/v1/devices/history")), token)?
            .json(package)
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(response_error(status.as_u16(), response))
        }
    }

    pub fn pending_history_packages(
        &self,
        token: &str,
    ) -> Result<Vec<DeviceHistoryRecord>, AccountApiError> {
        self.decode(
            self.device_authorized(self.http.get(self.url("/v1/devices/history")), token)?
                .send(),
        )
    }

    pub fn acknowledge_history_package(
        &self,
        token: &str,
        sync_id: &str,
    ) -> Result<(), AccountApiError> {
        let response = self
            .device_authorized(
                self.http
                    .post(self.url(&format!("/v1/devices/history/{sync_id}/ack"))),
                token,
            )?
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(response_error(status.as_u16(), response))
        }
    }

    pub fn health(&self) -> Result<(), AccountApiError> {
        let response = self
            .http
            .get(self.url("/health"))
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(response_error(status.as_u16(), response));
        }
        let body = response.text().map_err(request_error)?;
        if body.trim() != "ok" {
            return Err(AccountApiError {
                status: Some(status.as_u16()),
                message: "The account API health endpoint returned an unexpected response.".into(),
            });
        }
        Ok(())
    }

    pub fn register(
        &self,
        username: &str,
        display_name: &str,
        password: &str,
        invite_code: &str,
        identity: &EncryptedIdentity,
    ) -> Result<AccountSession, AccountApiError> {
        let request = RegistrationRequest {
            username,
            display_name,
            password,
            invite_code,
            public_key: &identity.id,
            encrypted_identity: EncryptedIdentityPayload::from(identity),
        };
        let response = self
            .http
            .post(self.url("/v1/accounts"))
            .json(&request)
            .send()
            .map_err(request_error)?;
        session_response(response)
    }

    pub fn login(&self, username: &str, password: &str) -> Result<AccountSession, AccountApiError> {
        let response = self
            .http
            .post(self.url("/v1/sessions"))
            .json(&LoginRequest { username, password })
            .send()
            .map_err(request_error)?;
        session_response(response)
    }

    pub fn update_profile(
        &self,
        token: &str,
        display_name: &str,
    ) -> Result<AccountUser, AccountApiError> {
        self.decode(
            self.authorized(self.http.patch(self.url("/v1/profile")), token)
                .json(&ProfileRequest { display_name })
                .send(),
        )
    }

    pub fn update_profile_image(
        &self,
        token: &str,
        profile_image: Option<&str>,
    ) -> Result<AccountUser, AccountApiError> {
        self.decode(
            self.authorized(self.http.patch(self.url("/v1/profile/image")), token)
                .json(&ProfileImageRequest { profile_image })
                .send(),
        )
    }

    pub fn change_password(
        &self,
        token: &str,
        current_password: &str,
        new_password: &str,
        identity: &EncryptedIdentity,
    ) -> Result<(), AccountApiError> {
        let response = self
            .authorized(self.http.post(self.url("/v1/account/password")), token)
            .json(&PasswordChangeRequest {
                current_password,
                new_password,
                public_key: &identity.id,
                encrypted_identity: EncryptedIdentityPayload::from(identity),
            })
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(response_error(status.as_u16(), response))
        }
    }

    pub fn logout(&self, token: &str) -> Result<(), AccountApiError> {
        let response = self
            .authorized(self.http.delete(self.url("/v1/sessions/current")), token)
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(response_error(status.as_u16(), response))
        }
    }

    pub fn initialize_identity(
        &self,
        token: &str,
        identity: &EncryptedIdentity,
    ) -> Result<AccountUser, AccountApiError> {
        self.decode(
            self.authorized(self.http.post(self.url("/v1/identity")), token)
                .json(&InitializeIdentityRequest {
                    public_key: &identity.id,
                    encrypted_identity: EncryptedIdentityPayload::from(identity),
                })
                .send(),
        )
    }

    pub fn settings(&self, token: &str) -> Result<HashMap<String, String>, AccountApiError> {
        let response: SettingsResponse = self.decode(
            self.authorized(self.http.get(self.url("/v1/settings")), token)
                .send(),
        )?;
        match response {
            SettingsResponse::Object(settings) => Ok(settings),
            SettingsResponse::Sequence(settings) if settings.is_empty() => Ok(HashMap::new()),
            SettingsResponse::Sequence(_) => Err(AccountApiError {
                status: Some(200),
                message: "The account API route /v1/settings returned a JSON array; expected a settings object.".into(),
            }),
        }
    }

    pub fn set_setting(&self, token: &str, key: &str, value: &str) -> Result<(), AccountApiError> {
        #[derive(Serialize)]
        struct SettingRequest<'a> {
            value: &'a str,
        }
        let response = self
            .authorized(
                self.http.put(self.url(&format!("/v1/settings/{key}"))),
                token,
            )
            .json(&SettingRequest { value })
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(response_error(status.as_u16(), response))
        }
    }

    pub fn friends(&self, token: &str) -> Result<Vec<User>, AccountApiError> {
        let friends = self.decode::<Vec<ApiFriend>>(
            self.authorized(self.http.get(self.url("/v1/friends")), token)
                .send(),
        )?;
        account_friends_to_users(friends)
    }

    pub fn friend_requests(&self, token: &str) -> Result<Vec<User>, AccountApiError> {
        let requests = self.decode::<Vec<ApiFriend>>(
            self.authorized(self.http.get(self.url("/v1/friend-requests")), token)
                .send(),
        )?;
        account_friends_to_users(requests)
    }

    pub fn outgoing_friend_requests(&self, token: &str) -> Result<Vec<User>, AccountApiError> {
        let requests = self.decode::<Vec<ApiFriend>>(
            self.authorized(
                self.http.get(self.url("/v1/friend-requests/outgoing")),
                token,
            )
            .send(),
        )?;
        account_friends_to_users(requests)
    }

    pub fn send_friend_request(
        &self,
        token: &str,
        username: &str,
    ) -> Result<FriendRequestStatus, AccountApiError> {
        let response = self
            .authorized(self.http.post(self.url("/v1/friends")), token)
            .json(&AddFriendRequest { username })
            .send()
            .map_err(request_error)?;
        let response: FriendRequestResponse = decode_response(response)?;
        Ok(response.status)
    }

    pub fn accept_friend_request(
        &self,
        token: &str,
        public_key: &str,
    ) -> Result<(), AccountApiError> {
        let response = self
            .authorized(
                self.http
                    .post(self.url(&format!("/v1/friend-requests/{public_key}/accept"))),
                token,
            )
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(response_error(status.as_u16(), response))
        }
    }

    pub fn decline_friend_request(
        &self,
        token: &str,
        public_key: &str,
    ) -> Result<(), AccountApiError> {
        self.delete(&format!("/v1/friend-requests/{public_key}"), token)
    }

    pub fn cancel_friend_request(
        &self,
        token: &str,
        public_key: &str,
    ) -> Result<(), AccountApiError> {
        self.delete(&format!("/v1/friend-requests/{public_key}"), token)
    }

    pub fn remove_friend(&self, token: &str, public_key: &str) -> Result<(), AccountApiError> {
        self.delete(&format!("/v1/friends/{public_key}"), token)
    }

    pub fn publish_presence(&self, token: &str, port: u16) -> Result<(), AccountApiError> {
        let response = self
            .authorized(self.http.post(self.url("/v1/presence")), token)
            .json(&PresenceRequest { port })
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(response_error(status.as_u16(), response))
        }
    }

    pub fn clear_presence(&self, token: &str) -> Result<(), AccountApiError> {
        self.delete("/v1/presence", token)
    }

    pub fn send_relay_message(
        &self,
        token: &str,
        recipient_public_key: &str,
        group_id: Option<&str>,
        envelope: &RelayEnvelope,
    ) -> Result<u64, AccountApiError> {
        let response = self
            .device_authorized(self.http.post(self.url("/v1/messages")), token)?
            .json(&RelayMessageRequest {
                recipient_public_key,
                group_id,
                envelope,
            })
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if status.is_success() {
            let body = response
                .json::<serde_json::Value>()
                .map_err(request_error)?;
            body.get("queue_sequence")
                .and_then(|value| value.as_u64())
                .ok_or_else(|| AccountApiError {
                    status: Some(status.as_u16()),
                    message: "The relay response did not include a queue sequence.".into(),
                })
        } else {
            Err(response_error(status.as_u16(), response))
        }
    }

    pub fn create_group(
        &self,
        token: &str,
        group_id: &str,
        member_public_keys: &[String],
    ) -> Result<(), AccountApiError> {
        let response = self
            .authorized(self.http.post(self.url("/v1/groups")), token)
            .json(&CreateGroupRequest {
                group_id,
                member_public_keys,
            })
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(response_error(status.as_u16(), response))
        }
    }

    pub fn group_roster(
        &self,
        token: &str,
        group_id: &str,
    ) -> Result<GroupRoster, AccountApiError> {
        let response: GroupRosterResponse = self.decode(
            self.authorized(
                self.http.get(self.url(&format!("/v1/groups/{group_id}"))),
                token,
            )
            .send(),
        )?;
        if response.group_id != group_id {
            return Err(AccountApiError {
                status: None,
                message: "The account API returned a roster for another group.".into(),
            });
        }
        let owner_public_key = response.owner_public_key.to_ascii_lowercase();
        let member_ids = response
            .members
            .iter()
            .map(|member| member.public_key.to_ascii_lowercase())
            .collect::<std::collections::HashSet<_>>();
        let member_usernames = response
            .members
            .iter()
            .map(|member| member.username.trim().to_ascii_lowercase())
            .collect::<std::collections::HashSet<_>>();
        if response.version == 0
            || !(3..=12).contains(&response.members.len())
            || member_ids.len() != response.members.len()
            || member_usernames.len() != response.members.len()
            || !decode_hex(&owner_public_key).is_some_and(|key| key.len() == 32)
            || !member_ids.contains(&owner_public_key)
            || response.members.iter().any(|member| {
                member.username.trim().is_empty()
                    || !decode_hex(&member.public_key.to_ascii_lowercase())
                        .is_some_and(|key| key.len() == 32)
            })
        {
            return Err(AccountApiError {
                status: None,
                message: "The account API returned an invalid group roster.".into(),
            });
        }
        let mut members = response.members;
        for member in &mut members {
            member.public_key = member.public_key.to_ascii_lowercase();
        }
        Ok(GroupRoster {
            group_id: response.group_id,
            version: response.version,
            owner_public_key,
            members,
        })
    }

    pub fn update_group_members(
        &self,
        token: &str,
        group_id: &str,
        operation_id: &str,
        expected_version: u64,
        member_public_keys: &[String],
        deliveries: &[crate::services::relay::RelayDelivery],
    ) -> Result<u64, AccountApiError> {
        let expected_result_version =
            expected_version
                .checked_add(1)
                .ok_or_else(|| AccountApiError {
                    status: None,
                    message: "The expected group version cannot be incremented.".into(),
                })?;
        let mut unique_members = std::collections::HashSet::new();
        let mut unique_messages = std::collections::HashSet::new();
        let owner_id = deliveries
            .first()
            .map(|delivery| delivery.envelope.sender_id.as_str());
        if !is_hex_identifier(group_id, 32)
            || !is_hex_identifier(operation_id, 32)
            || !(3..=12).contains(&member_public_keys.len())
            || member_public_keys.iter().any(|member_id| {
                !is_hex_identifier(member_id, 64) || !unique_members.insert(member_id.as_str())
            })
            || !(2..=12).contains(&deliveries.len())
            || owner_id.is_none()
            || owner_id.is_some_and(|owner| {
                !member_public_keys
                    .iter()
                    .any(|member_id| member_id == owner)
            })
            || deliveries.iter().any(|delivery| {
                Some(delivery.envelope.sender_id.as_str()) != owner_id
                    || delivery.envelope.recipient_id != delivery.recipient_public_key
                    || delivery.envelope.group_id.as_deref() != Some(group_id)
                    || !is_hex_identifier(&delivery.recipient_public_key, 64)
                    || !is_hex_identifier(&delivery.envelope.message_id, 32)
                    || !unique_messages.insert(delivery.envelope.message_id.as_str())
            })
        {
            return Err(AccountApiError {
                status: None,
                message: "The group membership update request is invalid.".into(),
            });
        }
        #[derive(Deserialize)]
        struct Response {
            version: u64,
        }
        let response: Response = self.decode(
            self.authorized(
                self.http
                    .put(self.url(&format!("/v1/groups/{group_id}/members"))),
                token,
            )
            .json(&UpdateGroupMembersRequest {
                operation_id,
                expected_version,
                member_public_keys,
                deliveries,
            })
            .send(),
        )?;
        if response.version != expected_result_version {
            return Err(AccountApiError {
                status: None,
                message: "The account API returned an unexpected group roster version.".into(),
            });
        }
        Ok(response.version)
    }

    pub fn publish_mls_key_packages(
        &self,
        token: &str,
        key_packages: &[String],
    ) -> Result<(), AccountApiError> {
        validate_mls_key_packages_for_publish(key_packages)?;
        let response = self
            .authorized(self.http.put(self.url("/v1/mls/key-packages")), token)
            .json(&MlsKeyPackagesPublishRequest { key_packages })
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            let mut error = response_error(status.as_u16(), response);
            if status.as_u16() == 400 && error.message == "An MLS key package is invalid." {
                error.message = "The account API rejected an MLS key package that passed the client's hexadecimal and size checks. Verify that RESONANCE_API_URL points to the current API endpoint.".into();
            }
            Err(error)
        }
    }

    pub fn claim_mls_key_packages(
        &self,
        token: &str,
        account_public_keys: &[String],
    ) -> Result<Vec<ClaimedMlsKeyPackage>, AccountApiError> {
        let response: MlsKeyPackagesClaimResponse = self.decode(
            self.authorized(
                self.http.post(self.url("/v1/mls/key-packages/claim")),
                token,
            )
            .json(&MlsKeyPackagesClaimRequest {
                account_public_keys,
            })
            .send(),
        )?;
        validate_claimed_mls_key_packages(response.key_packages, account_public_keys)
    }

    pub fn pending_relay_messages(
        &self,
        token: &str,
    ) -> Result<Vec<PendingRelayMessage>, AccountApiError> {
        self.decode(
            self.device_authorized(self.http.get(self.url("/v1/messages")), token)?
                .send(),
        )
    }

    pub fn message_receipts(&self, token: &str) -> Result<Vec<MessageReceipt>, AccountApiError> {
        self.decode(
            self.device_authorized(self.http.get(self.url("/v1/messages/receipts")), token)?
                .send(),
        )
    }

    pub fn inbound_message_receipts(
        &self,
        token: &str,
    ) -> Result<Vec<MessageReceipt>, AccountApiError> {
        self.decode(
            self.device_authorized(
                self.http.get(self.url("/v1/messages/receipts/inbound")),
                token,
            )?
            .send(),
        )
    }

    pub fn send_message_receipt(
        &self,
        token: &str,
        message_id: &str,
        state: &str,
    ) -> Result<(), AccountApiError> {
        let response = self
            .device_authorized(
                self.http
                    .post(self.url(&format!("/v1/messages/{message_id}/receipt"))),
                token,
            )?
            .json(&MessageReceiptRequest { state })
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(response_error(status.as_u16(), response))
        }
    }

    pub fn acknowledge_relay_message(
        &self,
        token: &str,
        message_id: &str,
        lease_token: &str,
        state: &str,
    ) -> Result<(), AccountApiError> {
        if !matches!(state, "delivered" | "rejected") {
            return Err(AccountApiError {
                status: None,
                message: "The delivery acknowledgement state is invalid.".into(),
            });
        }
        let response = self
            .device_authorized(
                self.http
                    .post(self.url(&format!("/v1/messages/{message_id}/ack"))),
                token,
            )?
            .json(&MessageAcknowledgementRequest { lease_token, state })
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(response_error(status.as_u16(), response))
        }
    }

    pub fn release_relay_message(
        &self,
        token: &str,
        message_id: &str,
        lease_token: &str,
        failure_code: &str,
    ) -> Result<(), AccountApiError> {
        if !matches!(failure_code, "temporary" | "missing_mls_state") {
            return Err(AccountApiError {
                status: None,
                message: "The delivery retry reason is invalid.".into(),
            });
        }
        let response = self
            .device_authorized(
                self.http
                    .post(self.url(&format!("/v1/messages/{message_id}/release"))),
                token,
            )?
            .json(&MessageReleaseRequest {
                lease_token,
                failure_code,
            })
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(response_error(status.as_u16(), response))
        }
    }

    pub fn revoke_device(&self, token: &str, device_id: &str) -> Result<(), AccountApiError> {
        if !is_hex_identifier(device_id, 32) {
            return Err(AccountApiError {
                status: None,
                message: "The device identifier is invalid.".into(),
            });
        }
        let response = self
            .device_authorized(
                self.http
                    .post(self.url(&format!("/v1/devices/{device_id}/revoke"))),
                token,
            )?
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(response_error(status.as_u16(), response))
        }
    }

    fn delete(&self, path: &str, token: &str) -> Result<(), AccountApiError> {
        let response = self
            .authorized(self.http.delete(self.url(path)), token)
            .send()
            .map_err(request_error)?;
        let status = response.status();
        if status.is_success() {
            Ok(())
        } else {
            Err(response_error(status.as_u16(), response))
        }
    }

    fn url(&self, path: &str) -> String {
        let mut url = self.base_url.clone();
        url.query_pairs_mut().append_pair("route", path);
        url.to_string()
    }

    fn authorized(&self, request: RequestBuilder, token: &str) -> RequestBuilder {
        let request = request
            .bearer_auth(token)
            .header("X-Resonance-Session", token);
        if let Some(device) = &self.device_auth {
            request
                .header("X-Resonance-Device-Id", &device.id)
                .header("X-Resonance-Device-Secret", &device.secret)
        } else {
            request
        }
    }

    fn device_authorized(
        &self,
        request: RequestBuilder,
        token: &str,
    ) -> Result<RequestBuilder, AccountApiError> {
        if self.device_auth.is_none() {
            return Err(AccountApiError {
                status: None,
                message: "Register this installation as a device before using message delivery."
                    .into(),
            });
        }
        Ok(self.authorized(request, token))
    }

    fn decode<T: DeserializeOwned>(
        &self,
        request: Result<reqwest::blocking::Response, reqwest::Error>,
    ) -> Result<T, AccountApiError> {
        let response = request.map_err(request_error)?;
        decode_response(response)
    }
}

impl From<&EncryptedIdentity> for EncryptedIdentityPayload {
    fn from(identity: &EncryptedIdentity) -> Self {
        Self {
            salt: encode_hex(&identity.salt),
            nonce: encode_hex(&identity.nonce),
            encrypted_private_key: encode_hex(&identity.encrypted_private_key),
        }
    }
}

fn build_http_client(timeout: Duration) -> Result<Client, AccountApiError> {
    Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|error| AccountApiError {
            status: None,
            message: format!("Could not initialize the account API client: {error}"),
        })
}

fn session_response(
    response: reqwest::blocking::Response,
) -> Result<AccountSession, AccountApiError> {
    let response: SessionResponse = decode_response(response)?;
    account_session(response)
}

fn account_session(response: SessionResponse) -> Result<AccountSession, AccountApiError> {
    let public_key = decode_hex(&response.account.public_key).ok_or_else(|| AccountApiError {
        status: None,
        message: "The account API returned an invalid public identity key.".into(),
    })?;
    let (identity, identity_error, identity_missing) = match response.encrypted_identity {
        Some(encrypted) => {
            let decoded = decode_identity_payload(&response.account, public_key, encrypted);
            match decoded {
                Ok(identity) => (Some(identity), None, false),
                Err(error) => (None, Some(error), false),
            }
        }
        None => (None, None, true),
    };
    Ok(AccountSession {
        access_token: response.access_token,
        account: response.account,
        identity,
        identity_error,
        identity_missing,
    })
}

fn decode_identity_payload(
    account: &AccountUser,
    public_key: Vec<u8>,
    encrypted: EncryptedIdentityPayload,
) -> Result<EncryptedIdentity, String> {
    let salt = decode_hex(&encrypted.salt)
        .ok_or_else(|| "The account API returned invalid encrypted identity data.".to_string())?;
    let nonce = decode_hex(&encrypted.nonce)
        .ok_or_else(|| "The account API returned invalid encrypted identity data.".to_string())?;
    let encrypted_private_key = decode_hex(&encrypted.encrypted_private_key)
        .ok_or_else(|| "The account API returned invalid encrypted identity data.".to_string())?;
    if salt.len() != 16 || nonce.len() != 12 || !(32..=4096).contains(&encrypted_private_key.len())
    {
        return Err("The account API returned corrupted encrypted identity data.".into());
    }
    Ok(EncryptedIdentity {
        id: account.public_key.clone(),
        username: account.username.clone(),
        display_name: account.display_name.clone(),
        public_key,
        salt,
        nonce,
        encrypted_private_key,
    })
}
fn decode_response<T: DeserializeOwned>(
    response: reqwest::blocking::Response,
) -> Result<T, AccountApiError> {
    let status = response.status();
    if !status.is_success() {
        return Err(response_error(status.as_u16(), response));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("missing")
        .to_owned();
    let route = response
        .url()
        .query_pairs()
        .find(|(key, _)| key == "route")
        .map(|(_, value)| value.into_owned())
        .unwrap_or_else(|| response.url().path().to_owned());
    let body = response.bytes().map_err(request_error)?;
    serde_json::from_slice(&body).map_err(|error| AccountApiError {
        status: Some(status.as_u16()),
        message: format!(
            "The account API route {route} returned invalid data (HTTP {status}, Content-Type {content_type}): {error}"
        ),
    })
}

fn response_error(status: u16, response: reqwest::blocking::Response) -> AccountApiError {
    let body = response.json::<ApiErrorBody>().ok();
    let mut message = body
        .as_ref()
        .and_then(|body| body.error.clone())
        .unwrap_or_else(|| format!("Account API request failed with HTTP {status}."));
    if let Some(request_id) = body.and_then(|body| body.request_id) {
        message.push_str(&format!(" (reference: {request_id})"));
    }
    AccountApiError {
        status: Some(status),
        message,
    }
}

fn request_error(error: reqwest::Error) -> AccountApiError {
    let cause = std::error::Error::source(&error).map(|cause| cause.to_string());
    AccountApiError {
        status: error.status().map(|status| status.as_u16()),
        message: match cause {
            Some(cause) => {
                format!("Could not reach the MySQL-backed account API: {error} (cause: {cause})")
            }
            None => format!("Could not reach the MySQL-backed account API: {error}"),
        },
    }
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

fn decode_hex(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().chunks_exact(2) {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        bytes.push(((high << 4) | low) as u8);
    }
    Some(bytes)
}

fn initials(name: &str) -> String {
    name.split_whitespace()
        .take(2)
        .filter_map(|word| word.chars().next())
        .flat_map(char::to_uppercase)
        .collect()
}

fn endpoint(ip: Option<String>, port: Option<u16>) -> Option<String> {
    let address = ip?.parse::<IpAddr>().ok()?;
    Some(SocketAddr::new(address, port?).to_string())
}

fn account_friends_to_users(friends: Vec<ApiFriend>) -> Result<Vec<User>, AccountApiError> {
    let mut identities = std::collections::HashSet::new();
    let mut usernames = std::collections::HashSet::new();
    let mut users = Vec::with_capacity(friends.len());
    for mut friend in friends {
        friend.public_key = friend.public_key.to_ascii_lowercase();
        let username = friend.username.trim().to_ascii_lowercase();
        if !is_hex_identifier(&friend.public_key, 64)
            || username.is_empty()
            || !identities.insert(friend.public_key.clone())
            || !usernames.insert(username)
        {
            return Err(AccountApiError {
                status: None,
                message: "The account API returned inconsistent friend identities.".into(),
            });
        }
        friend.username = friend.username.trim().to_string();
        users.push(account_friend_to_user(friend));
    }
    Ok(users)
}

fn validate_claimed_mls_key_packages(
    mut packages: Vec<ClaimedMlsKeyPackage>,
    requested_account_keys: &[String],
) -> Result<Vec<ClaimedMlsKeyPackage>, AccountApiError> {
    let requested = requested_account_keys
        .iter()
        .map(|key| key.to_ascii_lowercase())
        .collect::<std::collections::HashSet<_>>();
    let mut claimed = std::collections::HashSet::new();
    for package in &mut packages {
        package.account_public_key = package.account_public_key.to_ascii_lowercase();
        if !is_hex_identifier(&package.account_public_key, 64)
            || !requested.contains(&package.account_public_key)
            || !claimed.insert(package.account_public_key.clone())
            || package.key_package.is_empty()
        {
            return Err(AccountApiError {
                status: None,
                message: "The account API returned an inconsistent MLS key-package claim.".into(),
            });
        }
    }
    Ok(packages)
}

fn validate_mls_key_packages_for_publish(packages: &[String]) -> Result<(), AccountApiError> {
    if !(1..=10).contains(&packages.len()) {
        return Err(AccountApiError {
            status: None,
            message: "Publish between 1 and 10 MLS key packages.".into(),
        });
    }
    if let Some((index, package)) = packages.iter().enumerate().find(|(_, package)| {
        !(64..=4096).contains(&package.len())
            || package.len() % 2 != 0
            || !package.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        return Err(AccountApiError {
            status: None,
            message: format!(
                "MLS key package {} must contain 32 to 2048 bytes encoded as hexadecimal (got {} characters).",
                index + 1,
                package.len()
            ),
        });
    }
    Ok(())
}

fn account_friend_to_user(friend: ApiFriend) -> User {
    let initials = initials(&friend.display_name);
    let mut user = User::new(
        &friend.public_key,
        &friend.username,
        &friend.display_name,
        &initials,
        colors::ACCENT,
        "Offline",
    );
    user.profile_image = friend.profile_image;
    user.endpoint = endpoint(friend.endpoint_ip, friend.endpoint_port);
    user.status = if user.endpoint.is_some() {
        "Online".into()
    } else {
        "Offline".into()
    };
    user
}

fn is_hex_identifier(value: &str, length: usize) -> bool {
    value.len() == length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{BufRead, BufReader, Read, Write},
        net::TcpListener,
        thread,
    };

    #[test]
    fn hexadecimal_encoding_round_trips() {
        let bytes = [0, 15, 16, 255];
        let encoded = encode_hex(&bytes);
        assert_eq!(encoded, "000f10ff");
        assert_eq!(decode_hex(&encoded), Some(bytes.to_vec()));
        assert_eq!(decode_hex("xyz"), None);
        assert_eq!(decode_hex("0"), None);
    }

    #[test]
    fn friend_responses_require_unique_valid_identity_keys_and_account_names() {
        let friend = |username: &str, public_key: &str| ApiFriend {
            public_key: public_key.to_string(),
            username: username.to_string(),
            display_name: username.to_string(),
            profile_image: None,
            endpoint_ip: None,
            endpoint_port: None,
        };
        let users = account_friends_to_users(vec![friend("alice", &"AB".repeat(32))]).unwrap();
        assert_eq!(users[0].id, "ab".repeat(32));
        assert!(account_friends_to_users(vec![friend("alice", "not-a-public-key")]).is_err());
        assert!(
            account_friends_to_users(vec![
                friend("alice", &"01".repeat(32)),
                friend("ALICE", &"02".repeat(32)),
            ])
            .is_err()
        );
        assert!(
            account_friends_to_users(vec![
                friend("alice", &"01".repeat(32)),
                friend("bob", &"01".repeat(32)),
            ])
            .is_err()
        );
    }

    #[test]
    fn mls_key_package_claims_cannot_substitute_or_duplicate_requested_accounts() {
        let package = |account_public_key: String| ClaimedMlsKeyPackage {
            account_public_key,
            key_package: "deadbeef".into(),
        };
        let requested = vec!["a".repeat(64)];
        let claimed =
            validate_claimed_mls_key_packages(vec![package("A".repeat(64))], &requested).unwrap();
        assert_eq!(claimed[0].account_public_key, requested[0]);
        assert!(
            validate_claimed_mls_key_packages(vec![package("b".repeat(64))], &requested).is_err()
        );
        assert!(
            validate_claimed_mls_key_packages(
                vec![package("a".repeat(64)), package("A".repeat(64))],
                &requested,
            )
            .is_err()
        );
    }

    #[test]
    fn mls_key_package_publish_validation_matches_the_server_format_limits() {
        assert!(validate_mls_key_packages_for_publish(&["AB".repeat(32)]).is_ok());
        assert!(validate_mls_key_packages_for_publish(&["cd".repeat(2048)]).is_ok());
        assert!(validate_mls_key_packages_for_publish(&[]).is_err());
        assert!(validate_mls_key_packages_for_publish(&["ab".repeat(31)]).is_err());
        assert!(validate_mls_key_packages_for_publish(&["ab".repeat(2049)]).is_err());
        assert!(validate_mls_key_packages_for_publish(&["abc".into()]).is_err());
        assert!(validate_mls_key_packages_for_publish(&["xz".repeat(32)]).is_err());
    }

    #[test]
    fn api_url_must_be_http_or_https() {
        assert!(AccountApiClient::new("not a url").is_err());
        assert!(AccountApiClient::new("ftp://example.com").is_err());
        assert!(AccountApiClient::new("https://example.com").is_ok());
        assert!(AccountApiClient::new("http://127.0.0.1:8080").is_ok());
        assert!(AccountApiClient::new("http://example.com").is_err());
        assert!(AccountApiClient::new("https://example.com/api.php?route=/health").is_err());
    }

    #[test]
    fn api_routes_use_the_single_configured_php_file() {
        let client = AccountApiClient::new("https://example.com/resonance.php").unwrap();
        let url = reqwest::Url::parse(&client.url("/health")).unwrap();
        assert_eq!(url.path(), "/resonance.php");
        assert_eq!(
            url.query_pairs()
                .find(|(key, _)| key == "route")
                .map(|(_, value)| value.into_owned()),
            Some("/health".into())
        );
    }

    #[test]
    fn api_errors_include_server_reference_ids() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            assert!(!request_line.is_empty());
            loop {
                let mut header = String::new();
                reader.read_line(&mut header).unwrap();
                if header == "\r\n" {
                    break;
                }
            }
            let body = r#"{"error":"Account service is temporarily unavailable.","request_id":"0123456789abcdef"}"#;
            write!(
                reader.get_mut(),
                "HTTP/1.1 500 Internal Server Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body,
            )
            .unwrap();
        });

        let response = reqwest::blocking::get(format!("http://{address}")).unwrap();
        let error = response_error(500, response);
        assert_eq!(
            error.message,
            "Account service is temporarily unavailable. (reference: 0123456789abcdef)"
        );
        server.join().unwrap();
    }

    #[test]
    fn device_registration_proves_account_identity_and_scopes_queue_requests() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (signing_private_key, encryption_private_key) =
            crate::services::device_enrollment::generate_device_keys().unwrap();
        let device = DeviceIdentity {
            id: "1".repeat(32),
            secret: "2".repeat(64),
            signing_private_key,
            encryption_private_key,
            enrollment_request_id: "3".repeat(32),
            enrollment_nonce: "4".repeat(64),
            name: "Test device".into(),
        };
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            for index in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut headers = String::new();
                let mut content_length = 0;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        content_length = value.trim().parse::<usize>().unwrap();
                    }
                    headers.push_str(&header);
                }
                let mut body = vec![0; content_length];
                reader.read_exact(&mut body).unwrap();
                let response_body = if index == 0 {
                    "{\"device_id\":\"11111111111111111111111111111111\"}"
                } else {
                    "[]"
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                );
                reader.get_mut().write_all(response.as_bytes()).unwrap();
                requests.push((request_line, headers, body));
            }
            requests
        });

        let pkcs8 =
            ring::signature::Ed25519KeyPair::generate_pkcs8(&ring::rand::SystemRandom::new())
                .unwrap();
        let signer = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref()).unwrap();
        let public_key = encode_hex(signer.public_key().as_ref());
        let client = AccountApiClient::new(&format!("http://{address}")).unwrap();
        assert!(client.pending_relay_messages("session-token").is_err());
        let registered = client
            .register_device("session-token", &device, &public_key, pkcs8.as_ref())
            .unwrap();
        assert!(
            registered
                .pending_relay_messages("session-token")
                .unwrap()
                .is_empty()
        );
        let requests = server.join().unwrap();
        let registration: serde_json::Value = serde_json::from_slice(&requests[0].2).unwrap();
        let (signing_public_key, encryption_public_key) =
            crate::services::device_enrollment::device_public_keys(&device).unwrap();
        let signed = [
            b"Resonance device registration v2\0".as_slice(),
            public_key.as_bytes(),
            b"\0",
            device.id.as_bytes(),
            b"\0",
            device.secret.as_bytes(),
            b"\0",
            signing_public_key.as_bytes(),
            b"\0",
            encryption_public_key.as_bytes(),
            b"\0",
            device.name.as_bytes(),
        ]
        .concat();
        let signature = decode_hex(registration["signature"].as_str().unwrap()).unwrap();
        ring::signature::UnparsedPublicKey::new(
            &ring::signature::ED25519,
            signer.public_key().as_ref(),
        )
        .verify(&signed, &signature)
        .unwrap();
        assert!(requests[0].0.contains("route=%2Fv1%2Fdevices%2Fregister"));
        assert!(
            !requests[0]
                .1
                .to_ascii_lowercase()
                .contains("x-resonance-device-id")
        );
        assert!(requests[1].0.contains("route=%2Fv1%2Fmessages"));
        let delivery_headers = requests[1].1.to_ascii_lowercase();
        assert!(delivery_headers.contains(&format!("x-resonance-device-id: {}", device.id)));
        assert!(
            delivery_headers.contains(&format!("x-resonance-device-secret: {}", device.secret))
        );
    }

    #[test]
    fn group_roster_and_membership_retries_use_the_same_idempotency_payload() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let group_id = "0123456789abcdef0123456789abcdef".to_string();
        let owner_id = "a".repeat(64);
        let recipient_id = "b".repeat(64);
        let server_group_id = group_id.clone();
        let server_owner_id = owner_id.clone();
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            for index in 0..3 {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut content_length = 0;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        content_length = value.trim().parse::<usize>().unwrap();
                    }
                }
                let mut body = vec![0; content_length];
                reader.read_exact(&mut body).unwrap();
                let response_body = if index == 0 {
                    format!(
                        "{{\"group_id\":\"{server_group_id}\",\"version\":1,\"owner_public_key\":\"{server_owner_id}\",\"members\":[{{\"username\":\"owner\",\"display_name\":\"Owner\",\"public_key\":\"{server_owner_id}\"}},{{\"username\":\"one\",\"display_name\":\"One\",\"public_key\":\"{}\"}},{{\"username\":\"two\",\"display_name\":\"Two\",\"public_key\":\"{}\"}}]}}",
                        "b".repeat(64),
                        "c".repeat(64)
                    )
                } else {
                    "{\"version\":2}".to_string()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body
                );
                reader.get_mut().write_all(response.as_bytes()).unwrap();
                requests.push((request_line, body));
            }
            requests
        });

        let client = AccountApiClient::new(&format!("http://{address}")).unwrap();
        let roster = client.group_roster(&"c".repeat(64), &group_id).unwrap();
        assert_eq!(roster.version, 1);
        assert_eq!(roster.owner_public_key, owner_id);
        let deliveries = [recipient_id, "c".repeat(64)]
            .into_iter()
            .enumerate()
            .map(
                |(index, recipient_public_key)| crate::services::relay::RelayDelivery {
                    recipient_public_key: recipient_public_key.clone(),
                    envelope: RelayEnvelope {
                        message_id: format!("{:032x}", index + 1),
                        sender_id: owner_id.clone(),
                        recipient_id: recipient_public_key.clone(),
                        group_id: Some(group_id.clone()),
                        sent_at: "123".into(),
                        ephemeral_public_key: "2".repeat(64),
                        nonce: "3".repeat(24),
                        ciphertext: "4".repeat(34),
                        signature: "5".repeat(128),
                    },
                },
            )
            .collect::<Vec<_>>();
        for _ in 0..2 {
            assert_eq!(
                client
                    .update_group_members(
                        &"c".repeat(64),
                        &group_id,
                        "6".repeat(32).as_str(),
                        1,
                        &[owner_id.clone(), "d".repeat(64), "e".repeat(64)],
                        &deliveries,
                    )
                    .unwrap(),
                2
            );
        }
        let requests = server.join().unwrap();
        assert_eq!(
            requests[0].0,
            format!("GET /?route=%2Fv1%2Fgroups%2F{group_id} HTTP/1.1\r\n")
        );
        assert!(requests[1].0.starts_with("PUT /?route=%2Fv1%2Fgroups%2F"));
        assert_eq!(requests[1].0, requests[2].0);
        assert_eq!(requests[1].1, requests[2].1);
        let body: serde_json::Value = serde_json::from_slice(&requests[1].1).unwrap();
        assert_eq!(body["operation_id"], "6".repeat(32));
        assert_eq!(body["expected_version"], 1);
        assert_eq!(body["deliveries"][0]["envelope"]["group_id"], group_id);
    }

    #[test]
    fn outgoing_friend_requests_load_and_cancel_through_their_api_routes() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            (0..2)
                .map(|index| {
                    let (mut stream, _) = listener.accept().unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut request_line = String::new();
                    reader.read_line(&mut request_line).unwrap();
                    let response = if index == 0 {
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]".as_slice()
                    } else {
                        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".as_slice()
                    };
                    stream.write_all(response).unwrap();
                    request_line
                })
                .collect::<Vec<_>>()
        });

        let client = AccountApiClient::new(&format!("http://{address}")).unwrap();
        assert!(
            client
                .outgoing_friend_requests(&"a".repeat(64))
                .unwrap()
                .is_empty()
        );
        client
            .cancel_friend_request(&"a".repeat(64), &"b".repeat(64))
            .unwrap();

        assert_eq!(
            server.join().unwrap(),
            vec![
                "GET /?route=%2Fv1%2Ffriend-requests%2Foutgoing HTTP/1.1\r\n",
                "DELETE /?route=%2Fv1%2Ffriend-requests%2Fbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb HTTP/1.1\r\n",
            ]
        );
    }

    #[test]
    fn friend_request_response_distinguishes_pending_from_auto_acceptance() {
        let pending: FriendRequestResponse =
            serde_json::from_str(r#"{"status":"pending"}"#).unwrap();
        let accepted: FriendRequestResponse =
            serde_json::from_str(r#"{"status":"accepted"}"#).unwrap();
        assert_eq!(pending.status, FriendRequestStatus::Pending);
        assert_eq!(accepted.status, FriendRequestStatus::Accepted);
        assert!(serde_json::from_str::<FriendRequestResponse>(r#"{"status":"unknown"}"#).is_err());
    }

    #[test]
    fn invalid_api_json_reports_status_and_content_type() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: 18\r\nConnection: close\r\n\r\n<html>error</html>",
                )
                .unwrap();
        });

        let response = Client::new()
            .get(format!("http://{address}/?route=%2Fv1%2Fsessions"))
            .send()
            .unwrap();
        let error = decode_response::<serde_json::Value>(response).unwrap_err();
        server.join().unwrap();

        assert!(error.message.contains("route /v1/sessions"));
        assert!(error.message.contains("HTTP 200"));
        assert!(error.message.contains("Content-Type text/html"));
    }

    #[test]
    fn empty_settings_array_is_accepted_as_no_saved_preferences() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]",
                )
                .unwrap();
        });

        let client = AccountApiClient::new(&format!("http://{address}")).unwrap();
        assert!(client.settings("session-token").unwrap().is_empty());
        server.join().unwrap();
    }

    #[test]
    fn presence_heartbeat_uses_post() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            (0..3)
                .map(|_| {
                    let (mut stream, _) = listener.accept().unwrap();
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut request_line = String::new();
                    reader.read_line(&mut request_line).unwrap();
                    stream
                        .write_all(
                            b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                        )
                        .unwrap();
                    request_line
                })
                .collect::<Vec<_>>()
        });

        let client = AccountApiClient::new(&format!("http://{address}")).unwrap();
        let requests = (0..3)
            .map(|_| {
                client
                    .independent_client()
                    .unwrap()
                    .publish_presence(&"a".repeat(64), 47831)
            })
            .collect::<Result<Vec<_>, _>>();
        assert!(requests.is_ok());

        assert_eq!(
            server.join().unwrap(),
            vec!["POST /?route=%2Fv1%2Fpresence HTTP/1.1\r\n"; 3]
        );
    }

    #[test]
    fn friend_endpoint_is_built_as_a_socket_address_and_add_uses_username() {
        assert_eq!(
            endpoint(Some("2001:db8::1".into()), Some(47831)),
            Some("[2001:db8::1]:47831".into())
        );
        assert_eq!(endpoint(Some("not-an-ip".into()), Some(47831)), None);

        let body = serde_json::to_value(AddFriendRequest { username: "sam" }).unwrap();
        assert_eq!(body["username"], "sam");
        assert!(body.get("public_key").is_none());
    }

    #[test]
    fn group_relay_requests_are_bound_to_a_group_id() {
        let group_id = "0123456789abcdef0123456789abcdef";
        let members = vec!["a".repeat(64), "b".repeat(64), "c".repeat(64)];
        let create = serde_json::to_value(CreateGroupRequest {
            group_id,
            member_public_keys: &members,
        })
        .unwrap();
        let relay = serde_json::to_value(RelayMessageRequest {
            recipient_public_key: &members[1],
            group_id: Some(group_id),
            envelope: &RelayEnvelope {
                message_id: "1".repeat(32),
                sender_id: members[0].clone(),
                recipient_id: members[1].clone(),
                group_id: Some(group_id.into()),
                sent_at: "1".into(),
                ephemeral_public_key: "2".repeat(64),
                nonce: "3".repeat(24),
                ciphertext: "4".repeat(34),
                signature: "5".repeat(128),
            },
        })
        .unwrap();

        assert_eq!(create["group_id"], group_id);
        assert_eq!(create["member_public_keys"].as_array().unwrap().len(), 3);
        assert_eq!(relay["group_id"], group_id);
        assert_eq!(relay["envelope"]["group_id"], group_id);
    }

    #[test]
    fn account_registration_sends_an_invitation_code() {
        let body = serde_json::to_value(RegistrationRequest {
            username: "sam",
            display_name: "Sam",
            password: "correct horse battery staple",
            invite_code: &"a".repeat(48),
            public_key: &"b".repeat(64),
            encrypted_identity: EncryptedIdentityPayload {
                salt: "c".repeat(32),
                nonce: "d".repeat(24),
                encrypted_private_key: "e".repeat(64),
            },
        })
        .unwrap();
        assert_eq!(body["invite_code"], "a".repeat(48));
    }

    #[test]
    fn account_settings_requests_update_password_and_profile_image() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (identity, encrypted_identity) = crate::services::identity::IdentityService::new()
            .create("test_user", "Test User", "current secure password")
            .unwrap();
        let server_identity = encrypted_identity.clone();
        let server = thread::spawn(move || {
            let mut requests = Vec::new();
            for index in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut request_line = String::new();
                reader.read_line(&mut request_line).unwrap();
                let mut headers = String::new();
                let mut content_length = 0;
                loop {
                    let mut header = String::new();
                    reader.read_line(&mut header).unwrap();
                    if header == "\r\n" {
                        break;
                    }
                    if let Some((name, value)) = header.split_once(':')
                        && name.eq_ignore_ascii_case("content-length")
                    {
                        content_length = value.trim().parse::<usize>().unwrap();
                    }
                    headers.push_str(&header);
                }
                let mut body = vec![0; content_length];
                reader.read_exact(&mut body).unwrap();
                let (status, response_body) = if index == 0 {
                    ("204 No Content", String::new())
                } else {
                    (
                        "200 OK",
                        serde_json::json!({
                            "username": server_identity.username,
                            "display_name": server_identity.display_name,
                            "public_key": server_identity.id,
                            "profile_image": "YWJj"
                        })
                        .to_string(),
                    )
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response_body.len(),
                    response_body,
                );
                reader.get_mut().write_all(response.as_bytes()).unwrap();
                requests.push((request_line, headers, body));
            }
            requests
        });

        let client = AccountApiClient::new(&format!("http://{address}")).unwrap();
        client
            .change_password(
                "session-token",
                "current secure password",
                "another secure password",
                &encrypted_identity,
            )
            .unwrap();
        let updated = client
            .update_profile_image("session-token", Some("YWJj"))
            .unwrap();
        assert_eq!(updated.profile_image.as_deref(), Some("YWJj"));

        let requests = server.join().unwrap();
        assert!(
            requests[0]
                .0
                .starts_with("POST /?route=%2Fv1%2Faccount%2Fpassword ")
        );
        assert!(
            requests[0]
                .1
                .to_ascii_lowercase()
                .contains("x-resonance-session: session-token")
        );
        let password_body: serde_json::Value = serde_json::from_slice(&requests[0].2).unwrap();
        assert_eq!(password_body["current_password"], "current secure password");
        assert_eq!(password_body["new_password"], "another secure password");
        assert_eq!(password_body["public_key"], identity.user.id);
        assert_eq!(
            password_body["encrypted_identity"]["salt"],
            encode_hex(&encrypted_identity.salt)
        );

        assert!(
            requests[1]
                .0
                .starts_with("PATCH /?route=%2Fv1%2Fprofile%2Fimage ")
        );
        let image_body: serde_json::Value = serde_json::from_slice(&requests[1].2).unwrap();
        assert_eq!(image_body["profile_image"], "YWJj");
    }

    #[test]
    fn account_without_an_encrypted_identity_is_marked_for_initialization() {
        let session = account_session(SessionResponse {
            access_token: "token".into(),
            account: AccountUser {
                username: "test_user".into(),
                display_name: "Test User".into(),
                public_key: "00".repeat(32),
                profile_image: None,
            },
            encrypted_identity: None,
        })
        .unwrap();

        assert!(session.identity_missing);
        assert!(session.identity.is_none());
        assert!(session.identity_error.is_none());
    }

    #[test]
    fn corrupted_encrypted_identity_is_not_marked_as_missing() {
        let session = account_session(SessionResponse {
            access_token: "token".into(),
            account: AccountUser {
                username: "test_user".into(),
                display_name: "Test User".into(),
                public_key: "00".repeat(32),
                profile_image: None,
            },
            encrypted_identity: Some(EncryptedIdentityPayload {
                salt: "invalid".into(),
                nonce: String::new(),
                encrypted_private_key: String::new(),
            }),
        })
        .unwrap();

        assert!(!session.identity_missing);
        assert!(session.identity.is_none());
        assert!(session.identity_error.is_some());
    }
}
