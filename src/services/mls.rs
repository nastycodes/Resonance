use std::collections::HashMap;

use ed25519_dalek::SigningKey;
use openmls::prelude::tls_codec::{Deserialize as TlsDeserialize, Serialize as TlsSerialize};
use openmls::{
    group::MlsGroup,
    prelude::{
        BasicCredential, Ciphersuite, CredentialWithKey, GroupId, KeyPackage, KeyPackageIn,
        MlsGroupCreateConfig, MlsGroupJoinConfig, MlsMessageIn, ProcessedMessageContent,
        ProtocolVersion, StagedWelcome,
    },
};
use openmls_basic_credential::SignatureKeyPair;
use openmls_memory_storage::MemoryStorage;
use openmls_rust_crypto::RustCrypto;
use openmls_traits::OpenMlsProvider;
use ring::{
    aead, hkdf,
    signature::{self, Ed25519KeyPair, KeyPair, UnparsedPublicKey},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::services::local_chat_store::LocalChatStore;

const CREDENTIAL_CONTEXT: &[u8] = b"Resonance MLS identity binding v1\0";
const SIGNING_KEY_SALT: &[u8] = b"Resonance MLS signing key v1";
const CIPHERSUITE: Ciphersuite = Ciphersuite::MLS_128_DHKEMX25519_AES128GCM_SHA256_Ed25519;
pub(crate) const MAX_STORED_MLS_BYTES: usize = 32 * 1024 * 1024;
pub const MLS_MESSAGE_PREFIX: &str = "resonance-mls-message-v1:";

#[derive(Serialize, Deserialize)]
struct StorageSnapshot(Vec<(Vec<u8>, Vec<u8>)>);

struct Provider {
    crypto: RustCrypto,
    storage: MemoryStorage,
}

impl Provider {
    fn load(snapshot: &[u8]) -> Result<Self, String> {
        let values = if snapshot.is_empty() {
            HashMap::new()
        } else {
            let StorageSnapshot(entries) = serde_json::from_slice(snapshot)
                .map_err(|error| format!("The local MLS key store is invalid: {error}"))?;
            entries.into_iter().collect()
        };
        Ok(Self {
            crypto: RustCrypto::default(),
            storage: MemoryStorage {
                values: std::sync::RwLock::new(values),
            },
        })
    }

    fn snapshot(&self) -> Result<Vec<u8>, String> {
        let values = self
            .storage
            .values
            .read()
            .map_err(|_| "The local MLS key store is unavailable.".to_string())?;
        let snapshot = StorageSnapshot(
            values
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        );
        let bytes = serde_json::to_vec(&snapshot)
            .map_err(|error| format!("Could not encode the local MLS key store: {error}"))?;
        if bytes.len() > MAX_STORED_MLS_BYTES {
            return Err("The local MLS key store exceeded its size limit.".into());
        }
        Ok(bytes)
    }
}

impl OpenMlsProvider for Provider {
    type CryptoProvider = RustCrypto;
    type RandProvider = RustCrypto;
    type StorageProvider = MemoryStorage;

    fn storage(&self) -> &Self::StorageProvider {
        &self.storage
    }

    fn crypto(&self) -> &Self::CryptoProvider {
        &self.crypto
    }

    fn rand(&self) -> &Self::RandProvider {
        &self.crypto
    }
}

pub struct MlsService;

#[derive(Debug)]
pub struct PreparedMembershipChange {
    pub commit: String,
    pub welcome: Option<String>,
    pub expected_member_ids: Vec<String>,
    pub snapshot: Vec<u8>,
}

pub struct GroupStateSnapshot {
    pub epoch: u64,
    pub owner_id: String,
    pub member_ids: Vec<String>,
    pub credential_keys: HashMap<String, String>,
    pub credential_changes: Vec<String>,
}

pub struct DecryptedApplication {
    pub sender_id: String,
    pub plaintext: Vec<u8>,
    pub snapshot: Vec<u8>,
}

impl MlsService {
    pub fn generate_key_packages(
        store: &LocalChatStore,
        account_id: &str,
        identity_private_key: &[u8],
        count: usize,
    ) -> Result<Vec<String>, String> {
        if !(1..=20).contains(&count) {
            return Err("The MLS key package count must be between 1 and 20.".into());
        }
        let (provider, signer, credential) = load_context(store, account_id, identity_private_key)?;
        let credential_with_key = CredentialWithKey {
            credential: credential.into(),
            signature_key: signer.public().into(),
        };
        let mut packages = Vec::with_capacity(count);
        for _ in 0..count {
            let bundle = KeyPackage::builder()
                .build(CIPHERSUITE, &provider, &signer, credential_with_key.clone())
                .map_err(|error| format!("Could not create an MLS key package: {error}"))?;
            packages.push(encode_hex(
                &bundle
                    .key_package()
                    .tls_serialize_detached()
                    .map_err(|error| format!("Could not serialize an MLS key package: {error}"))?,
            ));
        }
        store.save_mls_storage_snapshot(provider.snapshot()?)?;
        Ok(packages)
    }

    pub fn create_group(
        store: &LocalChatStore,
        account_id: &str,
        identity_private_key: &[u8],
        conversation_id: &str,
        member_key_packages: &[(String, String)],
    ) -> Result<String, String> {
        validate_group_id(conversation_id)?;
        if member_key_packages.is_empty() || member_key_packages.len() > 11 {
            return Err("An MLS group must add between 1 and 11 peers.".into());
        }
        let (provider, signer, credential) = load_context(store, account_id, identity_private_key)?;
        let mut key_packages = Vec::<KeyPackage>::with_capacity(member_key_packages.len());
        let mut member_ids = std::collections::HashSet::new();
        for (member_id, encoded) in member_key_packages {
            if member_id == account_id || !member_ids.insert(member_id.as_str()) {
                return Err("The MLS group contains a duplicate or local member.".into());
            }
            let package_bytes = decode_hex(encoded)
                .ok_or_else(|| "A peer MLS key package is not valid hexadecimal.".to_string())?;
            let package = KeyPackageIn::tls_deserialize_exact(package_bytes)
                .map_err(|error| format!("A peer MLS key package is invalid: {error}"))?;
            validate_credential_binding(
                package
                    .unverified_credential()
                    .credential
                    .serialized_content(),
                Some(member_id),
            )?;
            key_packages.push(
                package
                    .validate(provider.crypto(), ProtocolVersion::Mls10)
                    .map_err(|error| {
                        format!("A peer MLS key package failed validation: {error}")
                    })?,
            );
        }

        let config = MlsGroupCreateConfig::builder()
            .ciphersuite(CIPHERSUITE)
            .use_ratchet_tree_extension(true)
            .build();
        let mut group = MlsGroup::new_with_group_id(
            &provider,
            &signer,
            &config,
            GroupId::from_slice(conversation_id.as_bytes()),
            CredentialWithKey {
                credential: credential.into(),
                signature_key: signer.public().into(),
            },
        )
        .map_err(|error| format!("Could not create the MLS group: {error}"))?;
        let (_, welcome, _) = group
            .add_members(&provider, &signer, &key_packages)
            .map_err(|error| format!("Could not add members to the MLS group: {error}"))?;
        group
            .merge_pending_commit(&provider)
            .map_err(|error| format!("Could not commit the initial MLS membership: {error}"))?;
        let welcome = welcome
            .tls_serialize_detached()
            .map_err(|error| format!("Could not serialize the MLS group welcome: {error}"))?;
        store.save_mls_storage_snapshot(provider.snapshot()?)?;
        Ok(encode_hex(&welcome))
    }

    pub fn prepare_membership_change(
        store: &LocalChatStore,
        account_id: &str,
        identity_private_key: &[u8],
        conversation_id: &str,
        add_key_package: Option<(&str, &str)>,
        remove_member_id: Option<&str>,
    ) -> Result<PreparedMembershipChange, String> {
        validate_group_id(conversation_id)?;
        if add_key_package.is_some() == remove_member_id.is_some() {
            return Err("An MLS change must add or remove exactly one member.".into());
        }
        let (provider, signer, _) = load_context(store, account_id, identity_private_key)?;
        let mut group = MlsGroup::load(
            provider.storage(),
            &GroupId::from_slice(conversation_id.as_bytes()),
        )
        .map_err(|error| format!("Could not load the MLS group: {error}"))?
        .ok_or_else(|| "The MLS group has not been established.".to_string())?;
        let state = snapshot_group_state(&group)?;
        if state.owner_id != account_id {
            return Err("Only the MLS group owner may change its membership.".into());
        }
        if group.pending_commit().is_some() {
            return Err("The MLS group already has an uncommitted membership change.".into());
        }
        let mut expected_member_ids = state.member_ids.clone();
        let (commit, welcome) = if let Some((member_id, encoded_key_package)) = add_key_package {
            if expected_member_ids.len() >= 12
                || expected_member_ids.contains(&member_id.to_string())
            {
                return Err("The MLS group cannot add this member.".into());
            }
            let package_bytes = decode_hex(encoded_key_package)
                .ok_or_else(|| "The peer MLS key package is not valid hexadecimal.".to_string())?;
            let package = KeyPackageIn::tls_deserialize_exact(package_bytes)
                .map_err(|error| format!("The peer MLS key package is invalid: {error}"))?;
            validate_credential_binding(
                package
                    .unverified_credential()
                    .credential
                    .serialized_content(),
                Some(member_id),
            )?;
            let package = package
                .validate(provider.crypto(), ProtocolVersion::Mls10)
                .map_err(|error| format!("The peer MLS key package failed validation: {error}"))?;
            let (commit, welcome, _) = group
                .add_members(&provider, &signer, &[package])
                .map_err(|error| format!("Could not add the MLS group member: {error}"))?;
            expected_member_ids.push(member_id.to_string());
            (commit, Some(welcome))
        } else {
            let Some(member_id) = remove_member_id else {
                return Err("The MLS membership change is invalid.".into());
            };
            if expected_member_ids.len() <= 3 || member_id == account_id {
                return Err("The MLS group cannot remove this member.".into());
            }
            let member = group
                .members()
                .find(|member| {
                    validate_credential_binding(member.credential.serialized_content(), None)
                        .map_or(false, |id| id == member_id)
                })
                .ok_or_else(|| "The member is not part of this MLS group.".to_string())?;
            let (commit, _, _) = group
                .remove_members(&provider, &signer, &[member.index])
                .map_err(|error| format!("Could not remove the MLS group member: {error}"))?;
            expected_member_ids.retain(|id| id != member_id);
            (commit, None)
        };
        let commit = commit
            .tls_serialize_detached()
            .map_err(|error| format!("Could not serialize the MLS membership commit: {error}"))?;
        let welcome = welcome
            .map(|welcome| {
                welcome.tls_serialize_detached().map_err(|error| {
                    format!("Could not serialize the MLS membership welcome: {error}")
                })
            })
            .transpose()?;
        expected_member_ids.sort();
        Ok(PreparedMembershipChange {
            commit: encode_hex(&commit),
            welcome: welcome.as_deref().map(encode_hex),
            expected_member_ids,
            snapshot: provider.snapshot()?,
        })
    }

    pub fn group_state(
        store: &LocalChatStore,
        account_id: &str,
        identity_private_key: &[u8],
        conversation_id: &str,
    ) -> Result<GroupStateSnapshot, String> {
        validate_group_id(conversation_id)?;
        let (provider, _, _) = load_context(store, account_id, identity_private_key)?;
        let group = MlsGroup::load(
            provider.storage(),
            &GroupId::from_slice(conversation_id.as_bytes()),
        )
        .map_err(|error| format!("Could not load the MLS group: {error}"))?
        .ok_or_else(|| "The MLS group has not been established.".to_string())?;
        snapshot_group_state(&group)
    }

    pub fn merge_pending_membership_change(
        store: &LocalChatStore,
        account_id: &str,
        identity_private_key: &[u8],
        conversation_id: &str,
        pending_snapshot: &[u8],
    ) -> Result<(GroupStateSnapshot, Vec<u8>), String> {
        validate_group_id(conversation_id)?;
        let provider = Provider::load(pending_snapshot)?;
        let mut group = MlsGroup::load(
            provider.storage(),
            &GroupId::from_slice(conversation_id.as_bytes()),
        )
        .map_err(|error| format!("Could not load the pending MLS group: {error}"))?
        .ok_or_else(|| "The pending MLS group is unavailable.".to_string())?;
        if group.pending_commit().is_none() {
            return Err("The pending MLS membership commit is missing.".into());
        }
        group.merge_pending_commit(&provider).map_err(|error| {
            format!("Could not apply the pending MLS membership commit: {error}")
        })?;
        let state = snapshot_group_state(&group)?;
        if state.owner_id != account_id {
            return Err("The local MLS group owner does not match this account.".into());
        }
        let (_, _, _) = load_context(store, account_id, identity_private_key)?;
        Ok((state, provider.snapshot()?))
    }

    pub fn process_membership_commit(
        store: &LocalChatStore,
        account_id: &str,
        identity_private_key: &[u8],
        conversation_id: &str,
        encoded_commit: &str,
    ) -> Result<(String, GroupStateSnapshot, Vec<u8>), String> {
        let result = Self::process_membership_commit_pending(
            store,
            account_id,
            identity_private_key,
            conversation_id,
            encoded_commit,
        )?;
        store.save_mls_storage_snapshot(result.2.clone())?;
        Ok(result)
    }

    pub fn process_membership_commit_pending(
        store: &LocalChatStore,
        account_id: &str,
        identity_private_key: &[u8],
        conversation_id: &str,
        encoded_commit: &str,
    ) -> Result<(String, GroupStateSnapshot, Vec<u8>), String> {
        validate_group_id(conversation_id)?;
        let (provider, _, _) = load_context(store, account_id, identity_private_key)?;
        let mut group = MlsGroup::load(
            provider.storage(),
            &GroupId::from_slice(conversation_id.as_bytes()),
        )
        .map_err(|error| format!("Could not load the MLS group: {error}"))?
        .ok_or_else(|| "The MLS group has not been established.".to_string())?;
        let bytes = decode_hex(encoded_commit)
            .ok_or_else(|| "The MLS membership commit is not valid hexadecimal.".to_string())?;
        let message = MlsMessageIn::tls_deserialize_exact(bytes)
            .map_err(|error| format!("The MLS membership commit is invalid: {error}"))?;
        let protocol_message = message
            .try_into_protocol_message()
            .map_err(|_| "The MLS membership commit has an unsupported wire format.".to_string())?;
        let processed = group
            .process_message(&provider, protocol_message)
            .map_err(|error| format!("The MLS membership commit failed authentication: {error}"))?;
        let sender_id =
            validate_credential_binding(processed.credential().serialized_content(), None)?;
        let before = snapshot_group_state(&group)?;
        let owner_id = before.owner_id.clone();
        if sender_id != owner_id {
            return Err("Only the MLS group owner may change its membership.".into());
        }
        let staged_commit = match processed.into_content() {
            ProcessedMessageContent::StagedCommitMessage(staged_commit) => staged_commit,
            _ => return Err("The received MLS membership event is not a commit.".into()),
        };
        group
            .merge_staged_commit(&provider, *staged_commit)
            .map_err(|error| format!("Could not apply the MLS membership commit: {error}"))?;
        let mut state = snapshot_group_state(&group)?;
        state.credential_changes = changed_existing_credentials(&before, &state);
        Ok((sender_id, state, provider.snapshot()?))
    }

    pub fn has_group(
        store: &LocalChatStore,
        account_id: &str,
        identity_private_key: &[u8],
        conversation_id: &str,
    ) -> Result<bool, String> {
        validate_group_id(conversation_id)?;
        let (provider, _, _) = load_context(store, account_id, identity_private_key)?;
        MlsGroup::load(
            provider.storage(),
            &GroupId::from_slice(conversation_id.as_bytes()),
        )
        .map(|group| group.is_some())
        .map_err(|error| format!("Could not read the MLS conversation: {error}"))
    }

    pub fn delete_group(
        store: &LocalChatStore,
        account_id: &str,
        identity_private_key: &[u8],
        conversation_id: &str,
    ) -> Result<(), String> {
        validate_group_id(conversation_id)?;
        let (provider, _, _) = load_context(store, account_id, identity_private_key)?;
        if let Some(mut group) = MlsGroup::load(
            provider.storage(),
            &GroupId::from_slice(conversation_id.as_bytes()),
        )
        .map_err(|error| format!("Could not load the MLS conversation: {error}"))?
        {
            group
                .delete(provider.storage())
                .map_err(|error| format!("Could not remove the MLS conversation: {error}"))?;
        }
        store.save_mls_storage_snapshot(provider.snapshot()?)
    }

    pub fn join_group(
        store: &LocalChatStore,
        account_id: &str,
        identity_private_key: &[u8],
        conversation_id: &str,
        encoded_welcome: &str,
        expected_member_ids: &[String],
        expected_owner_id: &str,
    ) -> Result<(), String> {
        validate_group_id(conversation_id)?;
        if !(2..=12).contains(&expected_member_ids.len()) {
            return Err("The expected MLS group membership is invalid.".into());
        }
        let (provider, _, _) = load_context(store, account_id, identity_private_key)?;
        let welcome_bytes = decode_hex(encoded_welcome)
            .ok_or_else(|| "The MLS group welcome is not valid hexadecimal.".to_string())?;
        let message = MlsMessageIn::tls_deserialize_exact(welcome_bytes)
            .map_err(|error| format!("The MLS group welcome is invalid: {error}"))?;
        let welcome = match message.extract() {
            openmls::prelude::MlsMessageBodyIn::Welcome(welcome) => welcome,
            _ => return Err("The received MLS message is not a group welcome.".into()),
        };
        let config = MlsGroupJoinConfig::builder()
            .use_ratchet_tree_extension(true)
            .build();
        let group = StagedWelcome::new_from_welcome(&provider, &config, welcome, None)
            .map_err(|error| format!("Could not verify the MLS group welcome: {error}"))?
            .into_group(&provider)
            .map_err(|error| format!("Could not join the MLS group: {error}"))?;
        if group.group_id().as_slice() != conversation_id.as_bytes() {
            return Err("The MLS welcome belongs to a different conversation.".into());
        }
        let mut actual_member_ids = std::collections::HashSet::new();
        for member in group.members() {
            let member_id =
                validate_credential_binding(member.credential.serialized_content(), None)?;
            actual_member_ids.insert(member_id);
        }
        let expected_member_ids: std::collections::HashSet<_> =
            expected_member_ids.iter().map(String::as_str).collect();
        if actual_member_ids.len() != expected_member_ids.len()
            || actual_member_ids
                .iter()
                .any(|member_id| !expected_member_ids.contains(member_id.as_str()))
        {
            return Err("The MLS welcome membership does not match the advertised roster.".into());
        }
        if snapshot_group_state(&group)?.owner_id != expected_owner_id {
            return Err("The MLS welcome was not authored by the expected group owner.".into());
        }
        store.save_mls_storage_snapshot(provider.snapshot()?)
    }

    pub fn encrypt(
        store: &LocalChatStore,
        account_id: &str,
        identity_private_key: &[u8],
        conversation_id: &str,
        plaintext: &[u8],
    ) -> Result<String, String> {
        validate_group_id(conversation_id)?;
        if plaintext.is_empty() || plaintext.len() > 12_000 {
            return Err("The encrypted message must contain 1-12000 bytes.".into());
        }
        let (provider, signer, _) = load_context(store, account_id, identity_private_key)?;
        let mut group = MlsGroup::load(
            provider.storage(),
            &GroupId::from_slice(conversation_id.as_bytes()),
        )
        .map_err(|error| format!("The MLS conversation is unavailable: {error}"))?
        .ok_or_else(|| "The MLS conversation has not been established.".to_string())?;
        let message = group
            .create_message(&provider, &signer, plaintext)
            .map_err(|error| format!("Could not encrypt the MLS message: {error}"))?;
        let ciphertext = message
            .tls_serialize_detached()
            .map_err(|error| format!("Could not serialize the MLS message: {error}"))?;
        store.save_mls_storage_snapshot(provider.snapshot()?)?;
        Ok(encode_hex(&ciphertext))
    }

    pub fn decrypt(
        store: &LocalChatStore,
        account_id: &str,
        identity_private_key: &[u8],
        conversation_id: &str,
        encoded_message: &str,
    ) -> Result<Option<(String, Vec<u8>)>, String> {
        let Some(decrypted) = Self::decrypt_pending(
            store,
            account_id,
            identity_private_key,
            conversation_id,
            encoded_message,
        )?
        else {
            return Ok(None);
        };
        store.save_mls_storage_snapshot(decrypted.snapshot)?;
        Ok(Some((decrypted.sender_id, decrypted.plaintext)))
    }

    pub fn decrypt_pending(
        store: &LocalChatStore,
        account_id: &str,
        identity_private_key: &[u8],
        conversation_id: &str,
        encoded_message: &str,
    ) -> Result<Option<DecryptedApplication>, String> {
        validate_group_id(conversation_id)?;
        let (provider, _, _) = load_context(store, account_id, identity_private_key)?;
        let mut group = MlsGroup::load(
            provider.storage(),
            &GroupId::from_slice(conversation_id.as_bytes()),
        )
        .map_err(|error| format!("The MLS conversation is unavailable: {error}"))?
        .ok_or_else(|| "The MLS conversation has not been established.".to_string())?;
        let bytes = decode_hex(encoded_message)
            .ok_or_else(|| "The MLS message is not valid hexadecimal.".to_string())?;
        let message = MlsMessageIn::tls_deserialize_exact(bytes)
            .map_err(|error| format!("The received MLS message is invalid: {error}"))?;
        let protocol_message = message
            .try_into_protocol_message()
            .map_err(|_| "The received MLS message has an unsupported wire format.".to_string())?;
        let processed = group
            .process_message(&provider, protocol_message)
            .map_err(|error| format!("The received MLS message failed authentication: {error}"))?;
        let sender_id =
            validate_credential_binding(processed.credential().serialized_content(), None)?;
        let plaintext = match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(message) => message.into_bytes(),
            ProcessedMessageContent::StagedCommitMessage(_) => {
                return Err(
                    "MLS membership commits require a versioned roster synchronization event."
                        .into(),
                );
            }
            ProcessedMessageContent::ProposalMessage(_)
            | ProcessedMessageContent::ExternalJoinProposalMessage(_) => {
                return Err(
                    "The received MLS proposal is not authorized by the application.".into(),
                );
            }
        };
        Ok(Some(DecryptedApplication {
            sender_id,
            plaintext,
            snapshot: provider.snapshot()?,
        }))
    }

    pub fn direct_group_id(
        first_account_id: &str,
        second_account_id: &str,
    ) -> Result<String, String> {
        decode_hex(first_account_id)
            .filter(|key| key.len() == 32)
            .ok_or_else(|| "The first direct-chat identity is invalid.".to_string())?;
        decode_hex(second_account_id)
            .filter(|key| key.len() == 32)
            .ok_or_else(|| "The second direct-chat identity is invalid.".to_string())?;
        if first_account_id == second_account_id {
            return Err("A direct MLS conversation requires two different accounts.".into());
        }
        let (lower, higher) = if first_account_id < second_account_id {
            (first_account_id, second_account_id)
        } else {
            (second_account_id, first_account_id)
        };
        let mut digest = Sha256::new();
        digest.update(b"Resonance direct MLS group v1\0");
        digest.update(lower.as_bytes());
        digest.update([0]);
        digest.update(higher.as_bytes());
        Ok(encode_hex(&digest.finalize()[..16]))
    }
}

fn snapshot_group_state(group: &MlsGroup) -> Result<GroupStateSnapshot, String> {
    let members = group
        .members()
        .map(|member| {
            let credential = member.credential.serialized_content();
            let account_id = validate_credential_binding(credential, None)?;
            Ok((
                member.index,
                account_id,
                if credential.len() == 128 {
                    encode_hex(&credential[32..64])
                } else {
                    return Err("An MLS credential has an invalid identity binding.".into());
                },
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let owner_id = members
        .iter()
        .find(|(index, _, _)| *index == openmls::prelude::LeafNodeIndex::new(0))
        .map(|(_, account_id, _)| account_id.clone())
        .ok_or_else(|| "The MLS group has no owner credential.".to_string())?;
    let mut credential_keys = HashMap::new();
    for (_, account_id, credential_key) in &members {
        if credential_keys
            .insert(account_id.clone(), credential_key.clone())
            .is_some()
        {
            return Err("The MLS group contains multiple credentials for one account.".into());
        }
    }
    let mut member_ids = members
        .into_iter()
        .map(|(_, account_id, _)| account_id)
        .collect::<Vec<_>>();
    member_ids.sort();
    Ok(GroupStateSnapshot {
        epoch: group.epoch().as_u64(),
        owner_id,
        member_ids,
        credential_keys,
        credential_changes: Vec::new(),
    })
}

fn changed_existing_credentials(
    before: &GroupStateSnapshot,
    after: &GroupStateSnapshot,
) -> Vec<String> {
    let mut changes = before
        .credential_keys
        .iter()
        .filter_map(|(account_id, previous_credential_key)| {
            after
                .credential_keys
                .get(account_id)
                .filter(|credential_key| *credential_key != previous_credential_key)
                .map(|_| account_id.clone())
        })
        .collect::<Vec<_>>();
    changes.sort();
    changes
}

fn load_context(
    store: &LocalChatStore,
    account_id: &str,
    identity_private_key: &[u8],
) -> Result<(Provider, SignatureKeyPair, BasicCredential), String> {
    let account_public_key = decode_hex(account_id)
        .filter(|key| key.len() == 32)
        .ok_or_else(|| "The account public identity is invalid.".to_string())?;
    let identity_signer = Ed25519KeyPair::from_pkcs8(identity_private_key)
        .map_err(|_| "The unlocked identity cannot authenticate MLS credentials.".to_string())?;
    if identity_signer.public_key().as_ref() != account_public_key {
        return Err("The unlocked identity does not match the MLS account.".into());
    }

    let mut mls_seed = Zeroizing::new([0u8; 32]);
    hkdf::Salt::new(hkdf::HKDF_SHA256, SIGNING_KEY_SALT)
        .extract(identity_private_key)
        .expand(&[b"credential-signing-key"], &aead::AES_256_GCM)
        .map_err(|_| "Could not derive the MLS credential key.".to_string())?
        .fill(&mut mls_seed[..])
        .map_err(|_| "Could not derive the MLS credential key.".to_string())?;
    let mls_signing_key = SigningKey::from_bytes(&mls_seed);
    let mls_public_key = mls_signing_key.verifying_key().to_bytes();
    let signer = SignatureKeyPair::from_raw(
        CIPHERSUITE.signature_algorithm(),
        mls_seed.to_vec(),
        mls_public_key.to_vec(),
    );
    let mut binding_message = Vec::with_capacity(
        CREDENTIAL_CONTEXT.len() + account_public_key.len() + mls_public_key.len(),
    );
    binding_message.extend_from_slice(CREDENTIAL_CONTEXT);
    binding_message.extend_from_slice(&account_public_key);
    binding_message.extend_from_slice(&mls_public_key);
    let signature = identity_signer.sign(&binding_message);
    let mut credential_identity = Vec::with_capacity(binding_message.len() + 64);
    credential_identity.extend_from_slice(&account_public_key);
    credential_identity.extend_from_slice(&mls_public_key);
    credential_identity.extend_from_slice(signature.as_ref());
    if credential_identity.len() != 128 {
        return Err("Could not create the MLS identity binding.".into());
    }
    let credential = BasicCredential::new(credential_identity);
    let provider = Provider::load(&store.mls_storage_snapshot()?)?;
    signer
        .store(provider.storage())
        .map_err(|error| format!("Could not store the MLS signing key: {error}"))?;
    Ok((provider, signer, credential))
}

fn validate_credential_binding(
    identity: &[u8],
    expected_account_id: Option<&str>,
) -> Result<String, String> {
    if identity.len() != 128 {
        return Err("An MLS credential has an invalid identity binding.".into());
    }
    let account_public_key = &identity[..32];
    let mls_public_key = &identity[32..64];
    let signature = &identity[64..];
    let account_id = encode_hex(account_public_key);
    if expected_account_id.is_some_and(|expected| expected != account_id) {
        return Err("An MLS key package belongs to a different account.".into());
    }
    let mut binding_message = Vec::with_capacity(
        CREDENTIAL_CONTEXT.len() + account_public_key.len() + mls_public_key.len(),
    );
    binding_message.extend_from_slice(CREDENTIAL_CONTEXT);
    binding_message.extend_from_slice(account_public_key);
    binding_message.extend_from_slice(mls_public_key);
    UnparsedPublicKey::new(&signature::ED25519, account_public_key)
        .verify(&binding_message, signature)
        .map_err(|_| "An MLS credential is not signed by its account identity.".to_string())?;
    Ok(account_id)
}

fn validate_group_id(group_id: &str) -> Result<(), String> {
    if group_id.len() != 32 || !group_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("The MLS conversation identifier is invalid.".into());
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
    use crate::services::identity::IdentityService;

    #[test]
    fn direct_conversation_ids_are_canonical_for_both_peers() {
        let first = "01".repeat(32);
        let second = "02".repeat(32);
        assert_eq!(
            MlsService::direct_group_id(&first, &second).unwrap(),
            MlsService::direct_group_id(&second, &first).unwrap()
        );
        assert!(MlsService::direct_group_id(&first, &first).is_err());
    }

    #[test]
    fn account_bound_mls_credentials_reject_key_substitution_and_allow_new_key_packages() {
        let identities = IdentityService::new();
        let (alice, _) = identities
            .create("mls_key_owner", "Alice", "alice credential password")
            .unwrap();
        let store = LocalChatStore::in_memory();
        let packages =
            MlsService::generate_key_packages(&store, &alice.user.id, alice.private_key(), 10)
                .unwrap();
        assert_eq!(packages.len(), 10);
        assert_ne!(packages[0], packages[1]);
        assert!(packages.iter().all(|package| {
            (64..=4096).contains(&package.len())
                && package.bytes().all(|byte| byte.is_ascii_hexdigit())
        }));

        let mut credential_keys = Vec::new();
        for encoded in packages {
            let bytes = decode_hex(&encoded).unwrap();
            let package = KeyPackageIn::tls_deserialize_exact(bytes).unwrap();
            let unverified_credential = package.unverified_credential();
            let credential = unverified_credential.credential.serialized_content();
            assert_eq!(
                validate_credential_binding(credential, Some(&alice.user.id)).unwrap(),
                alice.user.id
            );
            credential_keys.push(encode_hex(&credential[32..64]));

            let mut substituted = credential.to_vec();
            substituted[32] ^= 0x01;
            assert!(validate_credential_binding(&substituted, Some(&alice.user.id)).is_err());
        }
        assert_eq!(credential_keys[0], credential_keys[1]);
    }

    #[test]
    fn credential_continuity_detects_existing_key_rotation_and_ignores_member_additions() {
        let before = GroupStateSnapshot {
            epoch: 1,
            owner_id: "owner".into(),
            member_ids: vec!["owner".into(), "member".into()],
            credential_keys: HashMap::from([
                ("owner".into(), "a".repeat(64)),
                ("member".into(), "b".repeat(64)),
            ]),
            credential_changes: Vec::new(),
        };
        let added_device = GroupStateSnapshot {
            epoch: 2,
            owner_id: "owner".into(),
            member_ids: vec!["owner".into(), "member".into(), "new-member".into()],
            credential_keys: HashMap::from([
                ("owner".into(), "a".repeat(64)),
                ("member".into(), "b".repeat(64)),
                ("new-member".into(), "c".repeat(64)),
            ]),
            credential_changes: Vec::new(),
        };
        assert!(changed_existing_credentials(&before, &added_device).is_empty());

        let mut changed_credential = GroupStateSnapshot {
            credential_keys: HashMap::from([
                ("owner".into(), "a".repeat(64)),
                ("member".into(), "d".repeat(64)),
            ]),
            ..added_device
        };
        changed_credential.credential_changes =
            changed_existing_credentials(&before, &changed_credential);
        assert_eq!(changed_credential.credential_changes, vec!["member"]);
    }

    #[test]
    fn mls_welcome_persists_group_state_and_encrypts_messages_end_to_end() {
        let identities = IdentityService::new();
        let (alice, _) = identities
            .create("mls_alice", "Alice", "alice secure password")
            .unwrap();
        let (bob, _) = identities
            .create("mls_bob", "Bob", "bob secure password")
            .unwrap();
        let alice_store = LocalChatStore::in_memory();
        let bob_store = LocalChatStore::in_memory();
        let bob_key_package =
            MlsService::generate_key_packages(&bob_store, &bob.user.id, bob.private_key(), 1)
                .unwrap()
                .remove(0);
        let conversation_id = "0123456789abcdef0123456789abcdef";

        let welcome = MlsService::create_group(
            &alice_store,
            &alice.user.id,
            alice.private_key(),
            conversation_id,
            &[(bob.user.id.clone(), bob_key_package)],
        )
        .unwrap();
        assert!(
            MlsService::join_group(
                &bob_store,
                &bob.user.id,
                bob.private_key(),
                conversation_id,
                &welcome,
                &[alice.user.id.clone(), "ff".repeat(32)],
                &alice.user.id,
            )
            .is_err()
        );
        assert!(
            MlsService::join_group(
                &bob_store,
                &bob.user.id,
                bob.private_key(),
                conversation_id,
                &welcome,
                &[alice.user.id.clone(), bob.user.id.clone()],
                &bob.user.id,
            )
            .is_err()
        );
        MlsService::join_group(
            &bob_store,
            &bob.user.id,
            bob.private_key(),
            conversation_id,
            &welcome,
            &[alice.user.id.clone(), bob.user.id.clone()],
            &alice.user.id,
        )
        .unwrap();

        let ciphertext = MlsService::encrypt(
            &alice_store,
            &alice.user.id,
            alice.private_key(),
            conversation_id,
            b"MLS application data",
        )
        .unwrap();
        assert_eq!(
            MlsService::decrypt(
                &bob_store,
                &bob.user.id,
                bob.private_key(),
                conversation_id,
                &ciphertext,
            )
            .unwrap(),
            Some((alice.user.id, b"MLS application data".to_vec()))
        );
    }

    #[test]
    fn membership_changes_rotate_epochs_and_evict_removed_members() {
        let identities = IdentityService::new();
        let (alice, _) = identities
            .create("mls_owner", "Owner", "owner secure password")
            .unwrap();
        let (bob, _) = identities
            .create("mls_member", "Member", "member secure password")
            .unwrap();
        let (charlie, _) = identities
            .create("mls_remove", "Remove", "remove secure password")
            .unwrap();
        let (dave, _) = identities
            .create("mls_add", "Added", "added secure password")
            .unwrap();
        let alice_store = LocalChatStore::in_memory();
        let bob_store = LocalChatStore::in_memory();
        let charlie_store = LocalChatStore::in_memory();
        let dave_store = LocalChatStore::in_memory();
        let bob_package =
            MlsService::generate_key_packages(&bob_store, &bob.user.id, bob.private_key(), 1)
                .unwrap()
                .remove(0);
        let charlie_package = MlsService::generate_key_packages(
            &charlie_store,
            &charlie.user.id,
            charlie.private_key(),
            1,
        )
        .unwrap()
        .remove(0);
        let conversation_id = "0123456789abcdef0123456789abcdef";
        let initial_welcome = MlsService::create_group(
            &alice_store,
            &alice.user.id,
            alice.private_key(),
            conversation_id,
            &[
                (bob.user.id.clone(), bob_package),
                (charlie.user.id.clone(), charlie_package),
            ],
        )
        .unwrap();
        for (identity, store) in [(&bob, &bob_store), (&charlie, &charlie_store)] {
            MlsService::join_group(
                store,
                &identity.user.id,
                identity.private_key(),
                conversation_id,
                &initial_welcome,
                &[
                    alice.user.id.clone(),
                    bob.user.id.clone(),
                    charlie.user.id.clone(),
                ],
                &alice.user.id,
            )
            .unwrap();
        }

        assert_eq!(
            MlsService::group_state(
                &alice_store,
                &alice.user.id,
                alice.private_key(),
                conversation_id,
            )
            .unwrap()
            .epoch,
            1
        );
        assert!(
            MlsService::prepare_membership_change(
                &bob_store,
                &bob.user.id,
                bob.private_key(),
                conversation_id,
                Some((&dave.user.id, "not-a-package")),
                None,
            )
            .unwrap_err()
            .contains("Only the MLS group owner")
        );
        assert!(
            MlsService::prepare_membership_change(
                &bob_store,
                &bob.user.id,
                bob.private_key(),
                conversation_id,
                None,
                Some(&charlie.user.id),
            )
            .unwrap_err()
            .contains("Only the MLS group owner")
        );

        let dave_package =
            MlsService::generate_key_packages(&dave_store, &dave.user.id, dave.private_key(), 1)
                .unwrap()
                .remove(0);
        let addition = MlsService::prepare_membership_change(
            &alice_store,
            &alice.user.id,
            alice.private_key(),
            conversation_id,
            Some((&dave.user.id, &dave_package)),
            None,
        )
        .unwrap();
        let (alice_state, alice_snapshot) = MlsService::merge_pending_membership_change(
            &alice_store,
            &alice.user.id,
            alice.private_key(),
            conversation_id,
            &addition.snapshot,
        )
        .unwrap();
        assert_eq!(alice_state.epoch, 2);
        alice_store
            .save_mls_storage_snapshot(alice_snapshot)
            .unwrap();
        MlsService::join_group(
            &dave_store,
            &dave.user.id,
            dave.private_key(),
            conversation_id,
            addition.welcome.as_deref().unwrap(),
            &[
                alice.user.id.clone(),
                bob.user.id.clone(),
                charlie.user.id.clone(),
                dave.user.id.clone(),
            ],
            &alice.user.id,
        )
        .unwrap();
        for (identity, store) in [(&bob, &bob_store), (&charlie, &charlie_store)] {
            let (_, state, snapshot) = MlsService::process_membership_commit(
                store,
                &identity.user.id,
                identity.private_key(),
                conversation_id,
                &addition.commit,
            )
            .unwrap();
            assert_eq!(state.epoch, 2);
            store.save_mls_storage_snapshot(snapshot).unwrap();
        }

        let removal = MlsService::prepare_membership_change(
            &alice_store,
            &alice.user.id,
            alice.private_key(),
            conversation_id,
            None,
            Some(&charlie.user.id),
        )
        .unwrap();
        let (alice_state, alice_snapshot) = MlsService::merge_pending_membership_change(
            &alice_store,
            &alice.user.id,
            alice.private_key(),
            conversation_id,
            &removal.snapshot,
        )
        .unwrap();
        assert_eq!(alice_state.epoch, 3);
        assert!(!alice_state.member_ids.contains(&charlie.user.id));
        alice_store
            .save_mls_storage_snapshot(alice_snapshot)
            .unwrap();

        for (identity, store) in [
            (&bob, &bob_store),
            (&charlie, &charlie_store),
            (&dave, &dave_store),
        ] {
            let (_, state, snapshot) = MlsService::process_membership_commit(
                store,
                &identity.user.id,
                identity.private_key(),
                conversation_id,
                &removal.commit,
            )
            .unwrap();
            assert_eq!(state.epoch, 3);
            store.save_mls_storage_snapshot(snapshot).unwrap();
        }
        let post_removal_ciphertext = MlsService::encrypt(
            &alice_store,
            &alice.user.id,
            alice.private_key(),
            conversation_id,
            b"only current members can read this",
        )
        .unwrap();
        assert!(
            MlsService::decrypt(
                &charlie_store,
                &charlie.user.id,
                charlie.private_key(),
                conversation_id,
                &post_removal_ciphertext,
            )
            .is_err()
        );
        assert!(
            MlsService::decrypt(
                &bob_store,
                &bob.user.id,
                bob.private_key(),
                conversation_id,
                &post_removal_ciphertext,
            )
            .unwrap()
            .is_some()
        );
    }

    #[test]
    fn mls_key_packages_are_bound_to_their_account_identity() {
        let identities = IdentityService::new();
        let (alice, _) = identities
            .create("mls_sender", "Sender", "sender secure password")
            .unwrap();
        let (bob, _) = identities
            .create("mls_expected", "Expected", "expected secure password")
            .unwrap();
        let (charlie, _) = identities
            .create("mls_other", "Other", "other secure password")
            .unwrap();
        let bob_store = LocalChatStore::in_memory();
        let package =
            MlsService::generate_key_packages(&bob_store, &bob.user.id, bob.private_key(), 1)
                .unwrap()
                .remove(0);
        let alice_store = LocalChatStore::in_memory();

        assert!(
            MlsService::create_group(
                &alice_store,
                &alice.user.id,
                alice.private_key(),
                "fedcba9876543210fedcba9876543210",
                &[(charlie.user.id, package)],
            )
            .is_err()
        );
    }
}
