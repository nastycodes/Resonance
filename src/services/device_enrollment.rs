use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    hkdf,
    rand::{SecureRandom, SystemRandom},
    signature::{ED25519, Ed25519KeyPair, KeyPair, UnparsedPublicKey},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use crate::services::local_chat_store::DeviceIdentity;

pub const MAX_HISTORY_SYNC_BYTES: usize = 3 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptedHistoryPackage {
    pub sync_id: String,
    pub source_device_id: String,
    pub target_device_id: String,
    pub ephemeral_public_key: String,
    pub nonce: String,
    pub ciphertext: String,
    pub signature: String,
}

pub fn generate_device_keys() -> Result<(String, String), String> {
    let signing_key = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map_err(|_| "Could not generate the device signing key.".to_string())?;
    let mut encryption_key = [0u8; 32];
    SystemRandom::new()
        .fill(&mut encryption_key)
        .map_err(|_| "Could not generate the device encryption key.".to_string())?;
    Ok((
        encode_hex(signing_key.as_ref()),
        encode_hex(&encryption_key),
    ))
}

pub fn device_public_keys(device: &DeviceIdentity) -> Result<(String, String), String> {
    let signing_key = decode_hex(&device.signing_private_key)
        .and_then(|bytes| Ed25519KeyPair::from_pkcs8(&bytes).ok())
        .ok_or_else(|| "The device signing key is unavailable.".to_string())?;
    let encryption_secret = decode_hex(&device.encryption_private_key)
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or_else(|| "The device encryption key is unavailable.".to_string())?;
    let encryption_public = PublicKey::from(&StaticSecret::from(encryption_secret));
    Ok((
        encode_hex(signing_key.public_key().as_ref()),
        encode_hex(encryption_public.as_bytes()),
    ))
}

pub fn sign_device_data(device: &DeviceIdentity, data: &[u8]) -> Result<String, String> {
    let private_key = decode_hex(&device.signing_private_key)
        .and_then(|bytes| Ed25519KeyPair::from_pkcs8(&bytes).ok())
        .ok_or_else(|| "The device signing key is unavailable.".to_string())?;
    Ok(encode_hex(private_key.sign(data).as_ref()))
}

pub fn device_secret_hash(secret: &str) -> Result<String, String> {
    let secret = decode_hex(secret)
        .filter(|secret| secret.len() == 32)
        .ok_or_else(|| "The local device secret is invalid.".to_string())?;
    Ok(encode_hex(&Sha256::digest(secret)))
}

pub fn verify_signature(public_key: &str, data: &[u8], signature: &str) -> bool {
    let Some(public_key) = decode_hex(public_key) else {
        return false;
    };
    let Some(signature) = decode_hex(signature) else {
        return false;
    };
    UnparsedPublicKey::new(&ED25519, public_key)
        .verify(data, &signature)
        .is_ok()
}

pub fn enrollment_request_data(
    account_public_key: &str,
    device_id: &str,
    device_secret_hash: &str,
    signing_public_key: &str,
    encryption_public_key: &str,
    request_id: &str,
    nonce: &str,
    device_name: &str,
) -> Vec<u8> {
    canonical_fields(
        b"Resonance device enrollment request v1\0",
        &[
            account_public_key,
            device_id,
            device_secret_hash,
            signing_public_key,
            encryption_public_key,
            request_id,
            nonce,
            device_name,
        ],
    )
}

pub fn enrollment_approval_data(
    account_public_key: &str,
    request_id: &str,
    device_id: &str,
    signing_public_key: &str,
    encryption_public_key: &str,
    nonce: &str,
    request_signature: &str,
) -> Vec<u8> {
    canonical_fields(
        b"Resonance device enrollment approval v1\0",
        &[
            account_public_key,
            request_id,
            device_id,
            signing_public_key,
            encryption_public_key,
            nonce,
            request_signature,
        ],
    )
}

pub fn pairing_code(
    account_public_key: &str,
    request_id: &str,
    device_id: &str,
    signing_public_key: &str,
    encryption_public_key: &str,
    nonce: &str,
) -> String {
    let data = canonical_fields(
        b"Resonance device pairing fingerprint v1\0",
        &[
            account_public_key,
            request_id,
            device_id,
            signing_public_key,
            encryption_public_key,
            nonce,
        ],
    );
    encode_hex(&Sha256::digest(data))[..16].to_string()
}

pub fn encrypt_history(
    account_public_key: &str,
    source: &DeviceIdentity,
    target_device_id: &str,
    target_encryption_key: &str,
    plaintext: &[u8],
) -> Result<EncryptedHistoryPackage, String> {
    if plaintext.len() > MAX_HISTORY_SYNC_BYTES {
        return Err(format!(
            "The encrypted history package exceeds the {} MiB transfer limit.",
            MAX_HISTORY_SYNC_BYTES / (1024 * 1024)
        ));
    }
    let target_public_bytes = decode_hex(target_encryption_key)
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or_else(|| "The target device encryption key is invalid.".to_string())?;
    let mut ephemeral_bytes = [0u8; 32];
    let mut nonce = [0u8; 12];
    SystemRandom::new()
        .fill(&mut ephemeral_bytes)
        .and_then(|_| SystemRandom::new().fill(&mut nonce))
        .map_err(|_| "Could not create secure history-encryption randomness.".to_string())?;
    let ephemeral_secret = StaticSecret::from(ephemeral_bytes);
    let ephemeral_public = PublicKey::from(&ephemeral_secret);
    let shared_secret = ephemeral_secret.diffie_hellman(&PublicKey::from(target_public_bytes));
    if shared_secret.as_bytes().iter().all(|byte| *byte == 0) {
        return Err("The target device encryption key is invalid.".into());
    }
    let sync_id = crate::services::local_chat_store::new_id()?;
    let ephemeral_public_key = encode_hex(ephemeral_public.as_bytes());
    let nonce_hex = encode_hex(&nonce);
    let mut package = EncryptedHistoryPackage {
        sync_id,
        source_device_id: source.id.clone(),
        target_device_id: target_device_id.to_string(),
        ephemeral_public_key,
        nonce: nonce_hex,
        ciphertext: String::new(),
        signature: String::new(),
    };
    let aad = history_aad(account_public_key, &package);
    let key = derive_history_key(shared_secret.as_bytes(), &package)?;
    let mut ciphertext = plaintext.to_vec();
    let key = LessSafeKey::new(
        UnboundKey::new(&aead::AES_256_GCM, &key[..])
            .map_err(|_| "Could not initialize history encryption.".to_string())?,
    );
    key.seal_in_place_append_tag(
        Nonce::assume_unique_for_key(nonce),
        Aad::from(aad.as_slice()),
        &mut ciphertext,
    )
    .map_err(|_| "Could not encrypt device history.".to_string())?;
    package.ciphertext = encode_hex(&ciphertext);
    let signature_data = history_signature_data(account_public_key, &package);
    package.signature = sign_device_data(source, &signature_data)?;
    Ok(package)
}

pub fn decrypt_history(
    account_public_key: &str,
    target: &DeviceIdentity,
    source_signing_key: &str,
    package: &EncryptedHistoryPackage,
) -> Result<Vec<u8>, String> {
    if package.target_device_id != target.id {
        return Err("The history package is addressed to another device.".into());
    }
    if package.ciphertext.len() > MAX_HISTORY_SYNC_BYTES.saturating_mul(2) + 32 {
        return Err("The encrypted history package exceeds the supported size limit.".into());
    }
    let signature_data = history_signature_data(account_public_key, package);
    if !verify_signature(source_signing_key, &signature_data, &package.signature) {
        return Err("The history package signature is invalid.".into());
    }
    let ephemeral_public = decode_hex(&package.ephemeral_public_key)
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or_else(|| "The history package encryption key is invalid.".to_string())?;
    let nonce_bytes = decode_hex(&package.nonce)
        .and_then(|bytes| <[u8; 12]>::try_from(bytes).ok())
        .ok_or_else(|| "The history package nonce is invalid.".to_string())?;
    let target_secret = decode_hex(&target.encryption_private_key)
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok())
        .ok_or_else(|| "The target device encryption key is unavailable.".to_string())?;
    let target_secret = StaticSecret::from(target_secret);
    let shared_secret = target_secret.diffie_hellman(&PublicKey::from(ephemeral_public));
    if shared_secret.as_bytes().iter().all(|byte| *byte == 0) {
        return Err("The history package encryption key is invalid.".into());
    }
    let key = derive_history_key(shared_secret.as_bytes(), package)?;
    let ciphertext = decode_hex(&package.ciphertext)
        .ok_or_else(|| "The history package ciphertext is invalid.".to_string())?;
    let mut plaintext = ciphertext;
    let key = LessSafeKey::new(
        UnboundKey::new(&aead::AES_256_GCM, &key[..])
            .map_err(|_| "Could not initialize history decryption.".to_string())?,
    );
    let plaintext = key
        .open_in_place(
            Nonce::try_assume_unique_for_key(&nonce_bytes)
                .map_err(|_| "The history package nonce is invalid.".to_string())?,
            Aad::from(history_aad(account_public_key, package).as_slice()),
            &mut plaintext,
        )
        .map_err(|_| "The encrypted history package could not be authenticated.".to_string())?;
    Ok(plaintext.to_vec())
}

fn derive_history_key(
    shared_secret: &[u8; 32],
    package: &EncryptedHistoryPackage,
) -> Result<Zeroizing<[u8; 32]>, String> {
    let salt_bytes = decode_hex(&package.ephemeral_public_key)
        .ok_or_else(|| "The history package encryption key is invalid.".to_string())?;
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, &salt_bytes);
    let pseudorandom_key = salt.extract(shared_secret);
    let info = canonical_fields(
        b"Resonance encrypted device history v1\0",
        &[
            &package.sync_id,
            &package.source_device_id,
            &package.target_device_id,
        ],
    );
    let mut key = Zeroizing::new([0u8; 32]);
    pseudorandom_key
        .expand(&[info.as_slice()], &aead::AES_256_GCM)
        .map_err(|_| "Could not derive the history-encryption key.".to_string())?
        .fill(&mut key[..])
        .map_err(|_| "Could not derive the history-encryption key.".to_string())?;
    Ok(key)
}

fn history_aad(account_public_key: &str, package: &EncryptedHistoryPackage) -> Vec<u8> {
    canonical_fields(
        b"Resonance device history package v1\0",
        &[
            account_public_key,
            &package.sync_id,
            &package.source_device_id,
            &package.target_device_id,
            &package.ephemeral_public_key,
            &package.nonce,
        ],
    )
}

fn history_signature_data(account_public_key: &str, package: &EncryptedHistoryPackage) -> Vec<u8> {
    let mut data = history_aad(account_public_key, package);
    data.push(0);
    data.extend_from_slice(package.ciphertext.as_bytes());
    data
}

fn canonical_fields(domain: &[u8], fields: &[&str]) -> Vec<u8> {
    let mut data = domain.to_vec();
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            data.push(0);
        }
        data.extend_from_slice(field.as_bytes());
    }
    data
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut output = Vec::with_capacity(value.len() / 2);
    for pair in value.as_bytes().chunks_exact(2) {
        let high = (pair[0] as char).to_digit(16)? as u8;
        let low = (pair[1] as char).to_digit(16)? as u8;
        output.push((high << 4) | low);
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::local_chat_store::DeviceIdentity;

    fn device() -> DeviceIdentity {
        let private_key = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new()).unwrap();
        let mut encryption_key = [0u8; 32];
        SystemRandom::new().fill(&mut encryption_key).unwrap();
        DeviceIdentity {
            id: "0123456789abcdef0123456789abcdef".into(),
            secret: "a".repeat(64),
            signing_private_key: encode_hex(private_key.as_ref()),
            encryption_private_key: encode_hex(&encryption_key),
            enrollment_request_id: "a1".repeat(16),
            enrollment_nonce: "b2".repeat(32),
            name: "Test device".into(),
        }
    }

    #[test]
    fn device_history_is_encrypted_signed_and_bound_to_the_target() {
        let source = device();
        let mut target = device();
        target.id = "fedcba9876543210fedcba9876543210".into();
        let (source_signing, _) = device_public_keys(&source).unwrap();
        let (_, target_encryption) = device_public_keys(&target).unwrap();
        let package = encrypt_history(
            &"ab".repeat(32),
            &source,
            &target.id,
            &target_encryption,
            b"encrypted history fixture",
        )
        .unwrap();
        assert_eq!(
            decrypt_history(&"ab".repeat(32), &target, &source_signing, &package).unwrap(),
            b"encrypted history fixture"
        );
        let mut wrong_target = target.clone();
        wrong_target.id = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".into();
        assert!(
            decrypt_history(&"ab".repeat(32), &wrong_target, &source_signing, &package).is_err()
        );
        let mut tampered = package;
        tampered.target_device_id = wrong_target.id;
        assert!(decrypt_history(&"ab".repeat(32), &target, &source_signing, &tampered).is_err());
    }

    #[test]
    fn enrollment_signatures_bind_the_request_nonce_and_device_keys() {
        let device = device();
        let (signing_key, encryption_key) = device_public_keys(&device).unwrap();
        let request = enrollment_request_data(
            &"ab".repeat(32),
            &device.id,
            &device_secret_hash(&device.secret).unwrap(),
            &signing_key,
            &encryption_key,
            &device.enrollment_request_id,
            &device.enrollment_nonce,
            &device.name,
        );
        let signature = sign_device_data(&device, &request).unwrap();
        assert!(verify_signature(&signing_key, &request, &signature));
        let replayed = enrollment_request_data(
            &"ab".repeat(32),
            &device.id,
            &device_secret_hash(&device.secret).unwrap(),
            &signing_key,
            &encryption_key,
            &device.enrollment_request_id,
            &"c3".repeat(32),
            &device.name,
        );
        assert!(!verify_signature(&signing_key, &replayed, &signature));
    }

    #[test]
    fn trusted_device_approval_binds_the_enrollment_and_pairing_fingerprint() {
        let trusted = device();
        let target = device();
        let (trusted_public, _) = device_public_keys(&trusted).unwrap();
        let (target_signing, target_encryption) = device_public_keys(&target).unwrap();
        let request = enrollment_request_data(
            &"ab".repeat(32),
            &target.id,
            &device_secret_hash(&target.secret).unwrap(),
            &target_signing,
            &target_encryption,
            &target.enrollment_request_id,
            &target.enrollment_nonce,
            &target.name,
        );
        let request_signature = sign_device_data(&target, &request).unwrap();
        let approval = enrollment_approval_data(
            &"ab".repeat(32),
            &target.enrollment_request_id,
            &target.id,
            &target_signing,
            &target_encryption,
            &target.enrollment_nonce,
            &request_signature,
        );
        let approval_signature = sign_device_data(&trusted, &approval).unwrap();
        assert!(verify_signature(
            &trusted_public,
            &approval,
            &approval_signature
        ));

        let substituted_key = enrollment_approval_data(
            &"ab".repeat(32),
            &target.enrollment_request_id,
            &target.id,
            &target_signing,
            &"cd".repeat(32),
            &target.enrollment_nonce,
            &request_signature,
        );
        assert!(!verify_signature(
            &trusted_public,
            &substituted_key,
            &approval_signature
        ));
        let replayed_request = enrollment_request_data(
            &"ab".repeat(32),
            &target.id,
            &device_secret_hash(&target.secret).unwrap(),
            &target_signing,
            &target_encryption,
            &target.enrollment_request_id,
            &"ef".repeat(32),
            &target.name,
        );
        assert!(!verify_signature(
            &target_signing,
            &replayed_request,
            &request_signature
        ));
        assert_ne!(
            pairing_code(
                &"ab".repeat(32),
                &target.enrollment_request_id,
                &target.id,
                &target_signing,
                &target_encryption,
                &target.enrollment_nonce
            ),
            pairing_code(
                &"ab".repeat(32),
                &target.enrollment_request_id,
                &target.id,
                &target_signing,
                &"cd".repeat(32),
                &target.enrollment_nonce
            )
        );
    }

    #[test]
    fn history_ciphertext_signature_prevents_substitution_and_replay() {
        let source = device();
        let target = device();
        let (source_signing, _) = device_public_keys(&source).unwrap();
        let (_, target_encryption) = device_public_keys(&target).unwrap();
        let package = encrypt_history(
            &"ab".repeat(32),
            &source,
            &target.id,
            &target_encryption,
            b"history",
        )
        .unwrap();
        assert_eq!(
            decrypt_history(&"ab".repeat(32), &target, &source_signing, &package).unwrap(),
            b"history"
        );

        let mut tampered = package.clone();
        tampered.ciphertext.push_str("00");
        assert!(decrypt_history(&"ab".repeat(32), &target, &source_signing, &tampered).is_err());
        let mut replayed = package.clone();
        replayed.sync_id = "0".repeat(32);
        assert!(decrypt_history(&"ab".repeat(32), &target, &source_signing, &replayed).is_err());
        let wrong_source = device();
        let (wrong_source_signing, _) = device_public_keys(&wrong_source).unwrap();
        assert!(
            decrypt_history(&"ab".repeat(32), &target, &wrong_source_signing, &package).is_err()
        );
    }

    #[test]
    fn history_transfer_rejects_packages_over_the_limit() {
        let source = device();
        let target = device();
        let (_, target_encryption) = device_public_keys(&target).unwrap();
        assert!(
            encrypt_history(
                &"ab".repeat(32),
                &source,
                &target.id,
                &target_encryption,
                &vec![0; MAX_HISTORY_SYNC_BYTES + 1]
            )
            .is_err()
        );
    }
}
