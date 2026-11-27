use curve25519_dalek::edwards::CompressedEdwardsY;
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    hkdf,
    rand::{SecureRandom, SystemRandom},
    signature::{self, KeyPair, UnparsedPublicKey},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha512};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

const MAX_BODY_BYTES: usize = 30_000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RelayEnvelope {
    pub message_id: String,
    pub sender_id: String,
    pub recipient_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
    pub sent_at: String,
    pub ephemeral_public_key: String,
    pub nonce: String,
    pub ciphertext: String,
    pub signature: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct RelayDelivery {
    pub recipient_public_key: String,
    pub envelope: RelayEnvelope,
}

pub fn encrypt(
    sender_private_key: &[u8],
    recipient_id: &str,
    recipient_public_key: &str,
    message_id: &str,
    sent_at: &str,
    group_id: Option<&str>,
    body: &str,
) -> Result<RelayEnvelope, String> {
    validate_body(body)?;
    validate_message_metadata(message_id, sent_at)?;
    validate_group_id(group_id)?;
    let sender = signature::Ed25519KeyPair::from_pkcs8(sender_private_key)
        .map_err(|_| "The unlocked identity key cannot sign relay messages.".to_string())?;
    let sender_id = encode_hex(sender.public_key().as_ref());
    let recipient_public = decode_hex(recipient_public_key)
        .filter(|key| key.len() == 32)
        .ok_or_else(|| "The recipient identity key is invalid.".to_string())?;
    let recipient_public = edwards_public_to_x25519(&recipient_public)?;

    let mut ephemeral_bytes = Zeroizing::new([0u8; 32]);
    SystemRandom::new()
        .fill(&mut ephemeral_bytes[..])
        .map_err(|_| "Could not generate an encrypted relay key.".to_string())?;
    let ephemeral_secret = StaticSecret::from(*ephemeral_bytes);
    let ephemeral_public = PublicKey::from(&ephemeral_secret);
    let shared_secret = ephemeral_secret.diffie_hellman(&recipient_public);
    if !shared_secret.was_contributory() {
        return Err("The recipient identity key cannot establish a secure relay.".into());
    }

    let ephemeral_public_key = encode_hex(ephemeral_public.as_bytes());
    let nonce = random_nonce()?;
    let associated_data = associated_data(
        message_id,
        &sender_id,
        recipient_id,
        sent_at,
        group_id,
        &ephemeral_public_key,
    );
    let key = derive_key(shared_secret.as_bytes(), &associated_data)?;
    let ciphertext = seal(&key, &nonce, &associated_data, body.as_bytes())?;
    let mut envelope = RelayEnvelope {
        message_id: message_id.to_string(),
        sender_id,
        recipient_id: recipient_id.to_string(),
        group_id: group_id.map(str::to_string),
        sent_at: sent_at.to_string(),
        ephemeral_public_key,
        nonce: encode_hex(&nonce),
        ciphertext: encode_hex(&ciphertext),
        signature: String::new(),
    };
    envelope.signature = encode_hex(&sender.sign(&signature_content(&envelope)).as_ref());
    Ok(envelope)
}

pub fn decrypt(
    recipient_private_key: &[u8],
    recipient_id: &str,
    sender_public_key: &str,
    envelope: &RelayEnvelope,
) -> Result<String, String> {
    validate_message_metadata(&envelope.message_id, &envelope.sent_at)?;
    if envelope.recipient_id != recipient_id || envelope.sender_id != sender_public_key {
        return Err("The relayed message identity does not match its recipient.".into());
    }
    validate_group_id(envelope.group_id.as_deref())?;
    let sender_public = decode_hex(sender_public_key)
        .filter(|key| key.len() == 32)
        .ok_or_else(|| "The sender identity key is invalid.".to_string())?;
    let signature = decode_hex(&envelope.signature)
        .filter(|bytes| bytes.len() == 64)
        .ok_or_else(|| "The relay message signature is invalid.".to_string())?;
    UnparsedPublicKey::new(&signature::ED25519, sender_public)
        .verify(&signature_content(envelope), &signature)
        .map_err(|_| "The relay message signature could not be verified.".to_string())?;

    let ephemeral_public = decode_hex(&envelope.ephemeral_public_key)
        .filter(|key| key.len() == 32)
        .ok_or_else(|| "The relay encryption key is invalid.".to_string())?;
    let ephemeral_public = PublicKey::from(
        <[u8; 32]>::try_from(ephemeral_public.as_slice())
            .map_err(|_| "The relay encryption key is invalid.".to_string())?,
    );
    let private = x25519_secret(recipient_private_key)?;
    let shared_secret = private.diffie_hellman(&ephemeral_public);
    if !shared_secret.was_contributory() {
        return Err("The relayed message has an invalid encryption key.".into());
    }
    let nonce = decode_hex(&envelope.nonce)
        .filter(|nonce| nonce.len() == 12)
        .ok_or_else(|| "The relay message nonce is invalid.".to_string())?;
    let nonce = <[u8; 12]>::try_from(nonce.as_slice())
        .map_err(|_| "The relay message nonce is invalid.".to_string())?;
    let ciphertext = decode_hex(&envelope.ciphertext)
        .filter(|ciphertext| (17..=MAX_BODY_BYTES + 16).contains(&ciphertext.len()))
        .ok_or_else(|| "The encrypted relay message is invalid.".to_string())?;
    let associated_data = associated_data(
        &envelope.message_id,
        &envelope.sender_id,
        &envelope.recipient_id,
        &envelope.sent_at,
        envelope.group_id.as_deref(),
        &envelope.ephemeral_public_key,
    );
    let key = derive_key(shared_secret.as_bytes(), &associated_data)?;
    let plaintext = open(&key, &nonce, &associated_data, &ciphertext)?;
    let body = String::from_utf8(plaintext)
        .map_err(|_| "The decrypted relay message is not valid text.".to_string())?;
    validate_body(&body)?;
    Ok(body)
}

fn x25519_secret(pkcs8: &[u8]) -> Result<StaticSecret, String> {
    let marker = [0x04, 0x22, 0x04, 0x20];
    let mut seeds = pkcs8
        .windows(marker.len())
        .enumerate()
        .filter_map(|(offset, window)| (window == marker).then_some(offset + marker.len()));
    let Some(seed_offset) = seeds.next() else {
        return Err("The unlocked identity key has an unsupported format.".into());
    };
    if seeds.next().is_some() || seed_offset + 32 > pkcs8.len() {
        return Err("The unlocked identity key has an unsupported format.".into());
    }
    let seed = &pkcs8[seed_offset..seed_offset + 32];
    let digest = Sha512::digest(seed);
    let mut scalar = Zeroizing::new([0u8; 32]);
    scalar.copy_from_slice(&digest[..32]);
    Ok(StaticSecret::from(*scalar))
}

fn edwards_public_to_x25519(public_key: &[u8]) -> Result<PublicKey, String> {
    let encoded: [u8; 32] = public_key
        .try_into()
        .map_err(|_| "The recipient identity key is invalid.".to_string())?;
    let point = CompressedEdwardsY(encoded)
        .decompress()
        .ok_or_else(|| "The recipient identity key is invalid.".to_string())?;
    Ok(PublicKey::from(point.to_montgomery().to_bytes()))
}

fn validate_message_metadata(message_id: &str, sent_at: &str) -> Result<(), String> {
    if message_id.len() != 32 || !message_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("The relay message identifier is invalid.".into());
    }
    if sent_at.is_empty()
        || sent_at.len() > 20
        || !sent_at.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("The relay message timestamp is invalid.".into());
    }
    Ok(())
}

fn validate_group_id(group_id: Option<&str>) -> Result<(), String> {
    if group_id.is_some_and(|id| id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err("The relay group identifier is invalid.".into());
    }
    Ok(())
}

fn validate_body(body: &str) -> Result<(), String> {
    if body.trim().is_empty()
        || body.len() > MAX_BODY_BYTES
        || body
            .chars()
            .any(|character| character.is_control() && !matches!(character, '\n' | '\r'))
    {
        return Err("Message must contain 1-30000 visible bytes.".into());
    }
    Ok(())
}

fn associated_data(
    message_id: &str,
    sender_id: &str,
    recipient_id: &str,
    sent_at: &str,
    group_id: Option<&str>,
    ephemeral_public_key: &str,
) -> Vec<u8> {
    match group_id {
        Some(group_id) => format!(
            "resonance-relay-group-message-v1\0{group_id}\0{message_id}\0{sender_id}\0{recipient_id}\0{sent_at}\0{ephemeral_public_key}"
        ),
        None => format!(
            "resonance-relay-message-v1\0{message_id}\0{sender_id}\0{recipient_id}\0{sent_at}\0{ephemeral_public_key}"
        ),
    }
    .into_bytes()
}

fn signature_content(envelope: &RelayEnvelope) -> Vec<u8> {
    format!(
        "{}\0{}\0{}\0{}\0{}\0{}\0{}",
        String::from_utf8_lossy(&associated_data(
            &envelope.message_id,
            &envelope.sender_id,
            &envelope.recipient_id,
            &envelope.sent_at,
            envelope.group_id.as_deref(),
            &envelope.ephemeral_public_key,
        )),
        envelope.nonce,
        envelope.ciphertext,
        envelope.sender_id,
        envelope.recipient_id,
        envelope.message_id,
        envelope.sent_at,
    )
    .into_bytes()
}

fn derive_key(shared_secret: &[u8], context: &[u8]) -> Result<Zeroizing<[u8; 32]>, String> {
    let salt = hkdf::Salt::new(hkdf::HKDF_SHA256, b"resonance-relay-message-v1");
    let pseudorandom_key = salt.extract(shared_secret);
    let info = [context];
    let output = pseudorandom_key
        .expand(&info, &aead::AES_256_GCM)
        .map_err(|_| "Could not derive the relay encryption key.".to_string())?;
    let mut key = Zeroizing::new([0u8; 32]);
    output
        .fill(&mut key[..])
        .map_err(|_| "Could not derive the relay encryption key.".to_string())?;
    Ok(key)
}

fn random_nonce() -> Result<[u8; 12], String> {
    let mut nonce = [0u8; 12];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| "Could not initialize encrypted relay delivery.".to_string())?;
    Ok(nonce)
}

fn seal(
    key: &[u8; 32],
    nonce: &[u8; 12],
    associated_data: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, String> {
    let unbound = UnboundKey::new(&aead::AES_256_GCM, key)
        .map_err(|_| "Could not initialize relay message encryption.".to_string())?;
    let mut ciphertext = plaintext.to_vec();
    LessSafeKey::new(unbound)
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(*nonce),
            Aad::from(associated_data),
            &mut ciphertext,
        )
        .map_err(|_| "Could not encrypt the relay message.".to_string())?;
    Ok(ciphertext)
}

fn open(
    key: &[u8; 32],
    nonce: &[u8; 12],
    associated_data: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, String> {
    let unbound = UnboundKey::new(&aead::AES_256_GCM, key)
        .map_err(|_| "Could not initialize relay message decryption.".to_string())?;
    let mut plaintext = ciphertext.to_vec();
    let length = LessSafeKey::new(unbound)
        .open_in_place(
            Nonce::assume_unique_for_key(*nonce),
            Aad::from(associated_data),
            &mut plaintext,
        )
        .map_err(|_| "Relay message authentication failed.".to_string())?
        .len();
    plaintext.truncate(length);
    Ok(plaintext)
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
    hex.as_bytes()
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
    fn relay_message_validation_allows_line_breaks_but_rejects_other_controls() {
        assert!(validate_body("first line\nsecond line").is_ok());
        assert!(validate_body("first line\r\nsecond line").is_ok());
        assert!(validate_body("first line\0second line").is_err());
    }

    #[test]
    fn relay_messages_are_encrypted_and_authenticated_end_to_end() {
        let identities = IdentityService::new();
        let (alice, _) = identities
            .create("relay_alice", "Alice", "alice secure password")
            .unwrap();
        let (bob, _) = identities
            .create("relay_bob", "Bob", "bob secure password")
            .unwrap();
        let envelope = encrypt(
            alice.private_key(),
            &bob.user.id,
            &bob.user.id,
            "0123456789abcdef0123456789abcdef",
            "1234567890",
            None,
            "hello\nthrough the relay",
        )
        .unwrap();

        assert!(!envelope.ciphertext.contains("hello\nthrough the relay"));
        assert_eq!(
            decrypt(bob.private_key(), &bob.user.id, &alice.user.id, &envelope).unwrap(),
            "hello\nthrough the relay"
        );
        assert!(
            decrypt(
                alice.private_key(),
                &alice.user.id,
                &alice.user.id,
                &envelope
            )
            .is_err()
        );
        let mut tampered = envelope;
        tampered.ciphertext.push_str("00");
        assert!(decrypt(bob.private_key(), &bob.user.id, &alice.user.id, &tampered).is_err());
    }

    #[test]
    fn group_identifier_is_authenticated_with_the_encrypted_message() {
        let identities = IdentityService::new();
        let (alice, _) = identities
            .create("relay_group_alice", "Alice", "alice secure password")
            .unwrap();
        let (bob, _) = identities
            .create("relay_group_bob", "Bob", "bob secure password")
            .unwrap();
        let group_id = "0123456789abcdef0123456789abcdef";
        let envelope = encrypt(
            alice.private_key(),
            &bob.user.id,
            &bob.user.id,
            "1123456789abcdef0123456789abcdef",
            "1234567890",
            Some(group_id),
            "group message",
        )
        .unwrap();

        assert_eq!(envelope.group_id.as_deref(), Some(group_id));
        assert_eq!(
            decrypt(bob.private_key(), &bob.user.id, &alice.user.id, &envelope).unwrap(),
            "group message"
        );

        let mut tampered = envelope;
        tampered.group_id = Some("1123456789abcdef0123456789abcdef".into());
        assert!(decrypt(bob.private_key(), &bob.user.id, &alice.user.id, &tampered).is_err());
    }
}
