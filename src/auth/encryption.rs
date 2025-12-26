use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit, OsRng},
    Aes256Gcm, Key,
};
use base64::{engine::general_purpose::STANDARD, Engine as _};

#[derive(Debug)]
pub enum CryptoError {
    InvalidPayload,
    Base64(base64::DecodeError),
    Aes(aes_gcm::Error),
}

impl From<base64::DecodeError> for CryptoError {
    fn from(e: base64::DecodeError) -> Self {
        CryptoError::Base64(e)
    }
}
impl From<aes_gcm::Error> for CryptoError {
    fn from(e: aes_gcm::Error) -> Self {
        CryptoError::Aes(e)
    }
}

pub fn key_from_b64(s: &str) -> Result<[u8; 32], String> {
    let bytes = STANDARD.decode(s).map_err(|e| e.to_string())?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "Key must decode to exactly 32 bytes".to_string())?;
    Ok(arr)
}

pub fn encrypt_key(key_32bytes: &[u8; 32], plaintext: &[u8]) -> Result<String, CryptoError> {
    let key: &Key<Aes256Gcm> = key_32bytes.into();
    let cipher = Aes256Gcm::new(key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng); // 12 bytes :contentReference[oaicite:2]{index=2}
    let ciphertext = cipher.encrypt(&nonce, plaintext)?;
    let nonce_bytes: [u8; 12] = nonce.into();
    let mut out = Vec::with_capacity(12 + ciphertext.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(STANDARD.encode(out))
}

pub fn decrypt_key(key_32bytes: &[u8; 32], payload_b64: &str) -> Result<String, CryptoError> {
    let raw = STANDARD.decode(payload_b64)?;
    if raw.len() < 12 {
        return Err(CryptoError::InvalidPayload);
    }
    let (nonce_part, ciphertext) = raw.split_at(12);
    let nonce_arr: [u8; 12] = nonce_part
        .try_into()
        .map_err(|_| CryptoError::InvalidPayload)?;
    let nonce: aes_gcm::Nonce<aes_gcm::aead::consts::U12> = nonce_arr.into();
    let key: &Key<Aes256Gcm> = key_32bytes.into();
    let cipher = Aes256Gcm::new(key);
    let plaintext_bytes = cipher.decrypt(&nonce, ciphertext)?;
  Ok(String::from_utf8(plaintext_bytes).map_err(|_| CryptoError::InvalidPayload)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes_gcm::aead::{OsRng, rand_core::RngCore};

    fn random_key() -> [u8; 32] {
        let mut key = [0u8; 32];
        OsRng.fill_bytes(&mut key);
        key
    }

    #[test]
    fn roundtrip_encrypt_decrypt() {
        let key = random_key();
        let msg = "hello from rust 🦀 — AES-GCM test";

        let enc = encrypt_key(&key, msg.as_bytes()).expect("encrypt should succeed");
        let dec = decrypt_key(&key, &enc).expect("decrypt should succeed");

        assert_eq!(dec, msg);
    }

    #[test]
    fn decrypt_with_wrong_key_fails() {
        let key_ok = random_key();
        let key_bad = random_key();

        let enc = encrypt_key(&key_ok, b"secret message").expect("encrypt should succeed");
        let err = decrypt_key(&key_bad, &enc).expect_err("wrong key must fail");

        // Usually CryptoError::Aes for authentication failure, but don't overfit to internals
        matches!(err, CryptoError::Aes(_) | CryptoError::InvalidPayload);
    }

    #[test]
    fn tampering_ciphertext_fails() {
        let key = random_key();
        let enc = encrypt_key(&key, b"do not tamper").expect("encrypt should succeed");

        // Decode, flip a bit (tamper), re-encode
        let mut raw = STANDARD.decode(&enc).expect("base64 decode should succeed");
        assert!(raw.len() > 12);
        let raw_len = raw.len();
        raw[raw_len - 1] ^= 0x01;

        let tampered = STANDARD.encode(raw);
        let _ = decrypt_key(&key, &tampered).expect_err("tampered payload must fail");
    }

    #[test]
    fn invalid_base64_fails() {
        let key = random_key();
        let _ = decrypt_key(&key, "not base64!!").expect_err("invalid base64 must fail");
    }

    #[test]
    fn too_short_payload_fails() {
        let key = random_key();

        // base64 of 11 bytes => less than nonce length (12)
        let short = STANDARD.encode([0u8; 11]);
        let _ = decrypt_key(&key, &short).expect_err("payload shorter than nonce must fail");
    }

    #[test]
    fn key_from_b64_accepts_32_bytes_and_rejects_others() {
        let key = random_key();
        let b64 = STANDARD.encode(key);

        let parsed = key_from_b64(&b64).expect("should parse 32-byte key");
        assert_eq!(parsed, key);

        // 31 bytes should fail
        let b64_31 = STANDARD.encode([0u8; 31]);
        assert!(key_from_b64(&b64_31).is_err());

        // 33 bytes should fail
        let b64_33 = STANDARD.encode([0u8; 33]);
        assert!(key_from_b64(&b64_33).is_err());
    }
}
