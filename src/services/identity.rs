use std::num::NonZeroU32;

use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    pbkdf2,
    rand::{SecureRandom, SystemRandom},
    signature::{Ed25519KeyPair, KeyPair},
};
use zeroize::Zeroizing;

use crate::{models::user::User, theme::colors};

const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const PBKDF2_ITERATIONS: u32 = 310_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncryptedIdentity {
    pub id: String,
    pub username: String,
    pub display_name: String,
    pub public_key: Vec<u8>,
    pub salt: Vec<u8>,
    pub nonce: Vec<u8>,
    pub encrypted_private_key: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct IdentityService;

#[derive(Clone)]
pub struct UnlockedIdentity {
    pub user: User,
    private_key: Zeroizing<Vec<u8>>,
}

impl UnlockedIdentity {
    pub fn private_key(&self) -> &[u8] {
        &self.private_key
    }
}

impl IdentityService {
    pub fn new() -> Self {
        Self
    }

    pub fn create(
        &self,
        username: &str,
        display_name: &str,
        password: &str,
    ) -> Result<(UnlockedIdentity, EncryptedIdentity), IdentityError> {
        let username = username.trim().to_ascii_lowercase();
        let display_name = display_name.trim();
        validate_username(&username)?;
        validate_display_name(display_name)?;
        validate_password(password)?;

        let rng = SystemRandom::new();
        let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng)
            .map_err(|_| IdentityError("Could not generate an identity keypair".into()))?;
        let keypair = Ed25519KeyPair::from_pkcs8(pkcs8.as_ref())
            .map_err(|_| IdentityError("Generated identity key is invalid".into()))?;
        let public_key = keypair.public_key().as_ref().to_vec();
        let id = encode_hex(&public_key);

        let mut salt = [0; SALT_LEN];
        let mut nonce = [0; NONCE_LEN];
        rng.fill(&mut salt)
            .map_err(|_| IdentityError("Could not generate encryption salt".into()))?;
        rng.fill(&mut nonce)
            .map_err(|_| IdentityError("Could not generate encryption nonce".into()))?;
        let mut encryption_key = Zeroizing::new([0; 32]);
        derive_key(password, &salt, &mut encryption_key[..]);
        let encrypted_private_key =
            encrypt(&encryption_key[..], &nonce, id.as_bytes(), pkcs8.as_ref())?;

        let record = EncryptedIdentity {
            id,
            username,
            display_name: display_name.to_string(),
            public_key,
            salt: salt.to_vec(),
            nonce: nonce.to_vec(),
            encrypted_private_key,
        };
        let identity = unlocked_from_record(&record, pkcs8.as_ref().to_vec());
        Ok((identity, record))
    }

    pub fn unlock(
        &self,
        record: &EncryptedIdentity,
        password: &str,
    ) -> Result<UnlockedIdentity, IdentityError> {
        validate_password(password)?;
        if record.salt.len() != SALT_LEN || record.nonce.len() != NONCE_LEN {
            return Err(IdentityError("Stored identity data is corrupted".into()));
        }
        if record.public_key.len() != 32 || record.id != encode_hex(&record.public_key) {
            return Err(IdentityError(
                "Stored identity public key is corrupted".into(),
            ));
        }
        let mut encryption_key = Zeroizing::new([0; 32]);
        derive_key(password, &record.salt, &mut encryption_key[..]);
        let private_key = Zeroizing::new(
            decrypt(
                &encryption_key[..],
                &record.nonce,
                record.id.as_bytes(),
                &record.encrypted_private_key,
            )
            .map_err(|_| IdentityError("Incorrect password or corrupted identity data".into()))?,
        );

        let keypair = Ed25519KeyPair::from_pkcs8(&private_key)
            .map_err(|_| IdentityError("Stored identity key is corrupted".into()))?;
        if keypair.public_key().as_ref() != record.public_key {
            return Err(IdentityError(
                "Stored identity does not match its public key".into(),
            ));
        }
        Ok(unlocked_from_record(record, private_key.to_vec()))
    }

    pub fn update_display_name(
        &self,
        identity: &mut UnlockedIdentity,
        display_name: &str,
    ) -> Result<(), IdentityError> {
        let display_name = display_name.trim();
        validate_display_name(display_name)?;
        identity.user.name = display_name.to_string();
        identity.user.initials = initials(display_name);
        Ok(())
    }

    pub fn reencrypt(
        &self,
        identity: &UnlockedIdentity,
        password: &str,
    ) -> Result<EncryptedIdentity, IdentityError> {
        validate_password(password)?;

        let rng = SystemRandom::new();
        let mut salt = [0; SALT_LEN];
        let mut nonce = [0; NONCE_LEN];
        rng.fill(&mut salt)
            .map_err(|_| IdentityError("Could not generate encryption salt".into()))?;
        rng.fill(&mut nonce)
            .map_err(|_| IdentityError("Could not generate encryption nonce".into()))?;
        let mut encryption_key = Zeroizing::new([0; 32]);
        derive_key(password, &salt, &mut encryption_key[..]);
        let encrypted_private_key = encrypt(
            &encryption_key[..],
            &nonce,
            identity.user.id.as_bytes(),
            identity.private_key(),
        )?;

        Ok(EncryptedIdentity {
            id: identity.user.id.clone(),
            username: identity.user.username.clone(),
            display_name: identity.user.name.clone(),
            public_key: decode_hex(&identity.user.id)
                .ok_or_else(|| IdentityError("Stored identity public key is invalid".into()))?,
            salt: salt.to_vec(),
            nonce: nonce.to_vec(),
            encrypted_private_key,
        })
    }
}

fn unlocked_from_record(record: &EncryptedIdentity, private_key: Vec<u8>) -> UnlockedIdentity {
    UnlockedIdentity {
        user: User::new(
            &record.id,
            &record.username,
            &record.display_name,
            &initials(&record.display_name),
            colors::ACCENT,
            "Online",
        ),
        private_key: Zeroizing::new(private_key),
    }
}

fn derive_key(password: &str, salt: &[u8], output: &mut [u8]) {
    pbkdf2::derive(
        pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(PBKDF2_ITERATIONS).expect("iterations are non-zero"),
        salt,
        password.as_bytes(),
        output,
    );
}

fn encrypt(
    key: &[u8],
    nonce: &[u8],
    identity_id: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, IdentityError> {
    let unbound = UnboundKey::new(&aead::AES_256_GCM, key)
        .map_err(|_| IdentityError("Could not initialize identity encryption".into()))?;
    let key = LessSafeKey::new(unbound);
    let nonce = Nonce::try_assume_unique_for_key(nonce)
        .map_err(|_| IdentityError("Invalid identity encryption nonce".into()))?;
    let mut ciphertext = plaintext.to_vec();
    key.seal_in_place_append_tag(nonce, Aad::from(identity_id), &mut ciphertext)
        .map_err(|_| IdentityError("Could not encrypt identity key".into()))?;
    Ok(ciphertext)
}

fn decrypt(
    key: &[u8],
    nonce: &[u8],
    identity_id: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, IdentityError> {
    let unbound = UnboundKey::new(&aead::AES_256_GCM, key)
        .map_err(|_| IdentityError("Could not initialize identity decryption".into()))?;
    let key = LessSafeKey::new(unbound);
    let nonce = Nonce::try_assume_unique_for_key(nonce)
        .map_err(|_| IdentityError("Invalid identity encryption nonce".into()))?;
    let mut plaintext = ciphertext.to_vec();
    let length = key
        .open_in_place(nonce, Aad::from(identity_id), &mut plaintext)
        .map_err(|_| IdentityError("Identity could not be decrypted".into()))?
        .len();
    plaintext.truncate(length);
    Ok(plaintext)
}

fn validate_username(username: &str) -> Result<(), IdentityError> {
    if !(3..=32).contains(&username.len())
        || !username
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(IdentityError(
            "Username must be 3-32 letters, numbers, underscores or hyphens".into(),
        ));
    }
    Ok(())
}

fn validate_display_name(name: &str) -> Result<(), IdentityError> {
    if name.is_empty() || name.chars().count() > 64 || name.chars().any(char::is_control) {
        return Err(IdentityError(
            "Display name must contain 1-64 visible characters".into(),
        ));
    }
    Ok(())
}

fn validate_password(password: &str) -> Result<(), IdentityError> {
    if password.chars().count() < 10 || password.len() > 1024 {
        return Err(IdentityError(
            "Password must contain 10-1024 characters".into(),
        ));
    }
    Ok(())
}

fn initials(name: &str) -> String {
    let mut words = name.split_whitespace();
    let first = words.next().unwrap_or_default();
    let second = words.next();
    let mut result = first.chars().take(1).collect::<String>();
    if let Some(second) = second {
        result.extend(second.chars().take(1));
    }
    result.to_uppercase()
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
    if hex.len() % 2 != 0 {
        return None;
    }
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let pair = std::str::from_utf8(pair).ok()?;
            u8::from_str_radix(pair, 16).ok()
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityError(pub String);

impl std::fmt::Display for IdentityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for IdentityError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_identity_requires_the_registration_password_to_unlock() {
        let service = IdentityService::new();
        let (created, record) = service
            .create("Local_User", "Local User", "correct horse battery")
            .unwrap();
        assert_eq!(created.user.username, "local_user");
        assert!(!created.private_key().is_empty());
        assert!(service.unlock(&record, "wrong password").is_err());

        let unlocked = service.unlock(&record, "correct horse battery").unwrap();
        assert_eq!(unlocked.user.id, created.user.id);
        assert_eq!(unlocked.private_key(), created.private_key());
    }

    #[test]
    fn reencrypting_identity_changes_its_password_without_changing_its_key() {
        let service = IdentityService::new();
        let (identity, record) = service
            .create("Local_User", "Local User", "correct horse battery")
            .unwrap();
        let updated = service
            .reencrypt(&identity, "another correct horse battery")
            .unwrap();

        assert_eq!(updated.id, record.id);
        assert!(service.unlock(&updated, "correct horse battery").is_err());
        assert_eq!(
            service
                .unlock(&updated, "another correct horse battery")
                .unwrap()
                .private_key(),
            identity.private_key()
        );
    }

    #[test]
    fn identities_cannot_be_unlocked_with_another_accounts_password() {
        let service = IdentityService::new();
        let (_, first) = service
            .create("first_account", "First", "first account password")
            .unwrap();
        let (_, second) = service
            .create("second_account", "Second", "second account password")
            .unwrap();

        assert!(service.unlock(&first, "second account password").is_err());
        assert!(service.unlock(&second, "first account password").is_err());
        assert_ne!(first.id, second.id);
    }

    #[test]
    fn registration_rejects_invalid_usernames_and_weak_passwords() {
        let service = IdentityService::new();
        assert!(
            service
                .create("bad username", "Name", "correct horse battery")
                .is_err()
        );
        assert!(service.create("other", "Other", "short").is_err());
    }
}
