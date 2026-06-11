use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use sha2::{Digest, Sha256};

const KEY_PHRASE: &str = "//TODO This needs to be distributed in a different way";
const NONCE_SIZE: usize = 12;

fn cipher() -> Aes256Gcm {
    let key: [u8; 32] = Sha256::digest(KEY_PHRASE.as_bytes()).into();
    Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(&key))
}

pub fn encrypt(plaintext: &str) -> Result<String, String> {
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ct = cipher()
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut out = nonce.to_vec();
    out.extend_from_slice(&ct);
    Ok(format!("*EHE*{}", hex::encode(out)))
}

/// Returns "*EHE*" + plaintext when input starts with "*EHE*"; passes other strings through.
pub fn decrypt_setting(s: &str) -> Result<String, String> {
    let Some(hex_part) = s.strip_prefix("*EHE*") else {
        return Ok(s.to_string());
    };
    let enc = hex::decode(hex_part).map_err(|e| e.to_string())?;
    if enc.len() < NONCE_SIZE {
        return Err("ciphertext too short".to_string());
    }
    let (nonce, ct) = enc.split_at(NONCE_SIZE);
    let pt = cipher()
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|e| e.to_string())?;
    Ok(format!("*EHE*{}", String::from_utf8_lossy(&pt)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decrypts_go_ciphertext_fixture() {
        let val = "*EHE*8f7d64a1f1cefb44fe280d40bfe056ebd3aff457dd551ab8edf5d213cf9c";
        assert_eq!(decrypt_setting(val).unwrap(), "*EHE*42");
    }

    #[test]
    fn round_trip() {
        let c = encrypt("secret").unwrap();
        assert_eq!(decrypt_setting(&c).unwrap(), "*EHE*secret");
    }

    #[test]
    fn passthrough_without_prefix() {
        assert_eq!(decrypt_setting("plain").unwrap(), "plain");
    }
}
