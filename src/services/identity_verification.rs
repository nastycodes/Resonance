use qrcode::{QrCode, render::svg};
use sha2::{Digest, Sha256};

const IDENTITY_QR_PREFIX: &str = "resonance-identity-v1:";
const FINGERPRINT_CONTEXT: &[u8] = b"Resonance contact identity fingerprint v1\0";

pub fn fingerprint(public_key_hex: &str) -> Result<String, String> {
    let public_key = decode_public_key(public_key_hex)?;
    let mut digest = Sha256::new();
    digest.update(FINGERPRINT_CONTEXT);
    digest.update(public_key);
    Ok(group_hex(&digest.finalize()))
}

pub fn qr_payload(public_key_hex: &str) -> Result<String, String> {
    decode_public_key(public_key_hex)?;
    Ok(format!(
        "{IDENTITY_QR_PREFIX}{}",
        public_key_hex.to_ascii_lowercase()
    ))
}

pub fn qr_svg(public_key_hex: &str) -> Result<String, String> {
    let payload = qr_payload(public_key_hex)?;
    let code = QrCode::new(payload.as_bytes())
        .map_err(|error| format!("Could not encode the identity QR code: {error}"))?;
    Ok(code.render::<svg::Color>().min_dimensions(196, 196).build())
}

pub fn matches_presented_identity(presented: &str, public_key_hex: &str) -> bool {
    let normalized = presented.trim();
    if let Some(key) = normalized.strip_prefix(IDENTITY_QR_PREFIX) {
        return decode_public_key(key)
            .is_ok_and(|key| encode_hex(&key) == public_key_hex.to_ascii_lowercase());
    }
    let entered = normalized
        .chars()
        .filter(|character| !matches!(character, '-' | ':' | ' ' | '\t' | '\r' | '\n'))
        .flat_map(char::to_uppercase)
        .collect::<String>();
    fingerprint(public_key_hex).is_ok_and(|expected| {
        entered
            == expected
                .chars()
                .filter(|character| *character != '-')
                .collect::<String>()
    })
}

fn decode_public_key(value: &str) -> Result<Vec<u8>, String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("The identity public key must be 32 bytes encoded as hexadecimal.".into());
    }
    let bytes = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char)
                .to_digit(16)
                .expect("validated hexadecimal");
            let low = (pair[1] as char)
                .to_digit(16)
                .expect("validated hexadecimal");
            ((high << 4) | low) as u8
        })
        .collect::<Vec<_>>();
    Ok(bytes)
}

fn group_hex(bytes: &[u8]) -> String {
    let hex = encode_hex(bytes).to_ascii_uppercase();
    hex.as_bytes()
        .chunks(8)
        .map(|chunk| std::str::from_utf8(chunk).expect("hexadecimal is ASCII"))
        .collect::<Vec<_>>()
        .join("-")
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

    #[test]
    fn fingerprints_are_stable_key_derived_and_formatted_for_comparison() {
        let key = "01".repeat(32);
        let expected = fingerprint(&key).unwrap();
        assert_eq!(expected, fingerprint(&key.to_ascii_uppercase()).unwrap());
        assert_eq!(expected.len(), 71);
        assert_eq!(expected.matches('-').count(), 7);
        assert_ne!(expected, fingerprint(&"02".repeat(32)).unwrap());
    }

    #[test]
    fn manual_and_qr_presentations_match_only_the_exact_identity_key() {
        let key = "ab".repeat(32);
        let fingerprint = fingerprint(&key).unwrap();
        assert!(matches_presented_identity(&fingerprint, &key));
        assert!(matches_presented_identity(
            &fingerprint.to_ascii_lowercase().replace('-', " "),
            &key
        ));
        let payload = qr_payload(&key).unwrap();
        assert!(matches_presented_identity(&payload, &key));
        assert!(!matches_presented_identity(&payload, &"cd".repeat(32)));
        assert!(!matches_presented_identity("not a fingerprint", &key));
    }

    #[test]
    fn qr_image_is_generated_from_the_identity_key() {
        let qr = qr_svg(&"01".repeat(32)).unwrap();
        assert!(qr.starts_with("<?xml"));
        assert!(qr.contains("<svg "));
        assert!(qr.contains("</svg>"));
    }

    #[test]
    fn malformed_identity_keys_are_rejected() {
        assert!(fingerprint("01").is_err());
        assert!(qr_payload(&"xz".repeat(32)).is_err());
    }
}
