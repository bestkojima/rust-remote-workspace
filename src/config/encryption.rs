//! Current-user protected envelopes. Only old TermiRs profiles use PBKDF2.
use crate::error::{AppError, Result};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::pbkdf2::{PBKDF2_HMAC_SHA256, derive};
#[cfg(test)]
use ring::rand::SecureRandom;
use std::num::NonZeroU32;

#[cfg(windows)]
#[path = "credential_windows.rs"]
mod platform;
#[cfg(unix)]
#[path = "credential_unix.rs"]
mod platform;

#[cfg(unix)]
pub(super) use platform::StoreKey;

const SALT_LEN: usize = 16;
const KEY_LEN: usize = 32;
const PBKDF2_ITERATIONS: u32 = 100_000;

fn error(message: &str) -> AppError {
    AppError::EncryptionError(message.into())
}

#[derive(Default)]
pub struct PasswordEncryption;

impl PasswordEncryption {
    pub fn new() -> Self {
        Self
    }
    pub fn encrypt_password(&self, password: &str) -> Result<String> {
        let ciphertext = platform::protect(password.as_bytes())?;
        Ok(format!(
            "{}:{}",
            platform::SCHEME,
            STANDARD.encode(ciphertext)
        ))
    }

    pub fn decrypt_password(&self, envelope: &str) -> Result<String> {
        let Some((scheme, encoded)) = envelope.split_once(':') else {
            return self.decrypt_legacy(envelope);
        };
        if scheme != platform::SCHEME {
            return Err(error(
                "Unsupported credential format for this operating system.",
            ));
        }
        let blob = STANDARD
            .decode(encoded)
            .map_err(|_| error("Invalid protected credential encoding."))?;
        String::from_utf8(platform::unprotect(&blob)?)
            .map_err(|_| error("Invalid protected credential text."))
    }
    /// Derive encryption key from system-specific information
    fn derive_key(&self, salt: &[u8]) -> Result<[u8; KEY_LEN]> {
        let mut key = [0u8; KEY_LEN];

        // Use system-specific information for key derivation
        let system_info = self.get_system_info()?;
        let iterations = NonZeroU32::new(PBKDF2_ITERATIONS)
            .ok_or_else(|| AppError::EncryptionError("Invalid iteration count".to_string()))?;

        derive(
            PBKDF2_HMAC_SHA256,
            iterations,
            salt,
            system_info.as_bytes(),
            &mut key,
        );

        Ok(key)
    }

    /// Get system-specific information for key derivation
    fn get_system_info(&self) -> Result<String> {
        // Use a combination of system information for key derivation
        // Preserve the upstream identifiers and fallbacks for legacy profiles only.
        let hostname = std::env::var("HOSTNAME")
            .or_else(|_| std::env::var("COMPUTERNAME"))
            .unwrap_or_else(|_| "default_host".to_string());

        let username = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "default_user".to_string());

        Ok(format!("termirs_{hostname}_{username}"))
    }

    /// Encrypt a password using AES-256-GCM
    #[cfg(test)]
    pub(super) fn encrypt_legacy(&self, password: &str) -> Result<String> {
        // Generate random salt
        let mut salt = [0u8; SALT_LEN];
        ring::rand::SystemRandom::new()
            .fill(&mut salt)
            .map_err(|_| AppError::EncryptionError("Failed to generate salt".to_string()))?;

        // Generate random nonce
        let mut nonce_bytes = [0u8; NONCE_LEN];
        ring::rand::SystemRandom::new()
            .fill(&mut nonce_bytes)
            .map_err(|_| AppError::EncryptionError("Failed to generate nonce".to_string()))?;

        // Derive key from salt
        let key_bytes = self.derive_key(&salt)?;

        // Create encryption key
        let unbound_key = UnboundKey::new(&AES_256_GCM, &key_bytes).map_err(|_| {
            AppError::EncryptionError("Failed to create encryption key".to_string())
        })?;
        let key = LessSafeKey::new(unbound_key);

        // Create nonce
        let nonce = Nonce::assume_unique_for_key(nonce_bytes);

        // Encrypt password
        let mut password_bytes = password.as_bytes().to_vec();
        key.seal_in_place_append_tag(nonce, Aad::empty(), &mut password_bytes)
            .map_err(|_| AppError::EncryptionError("Failed to encrypt password".to_string()))?;

        // Combine salt + nonce + encrypted_data and encode as base64
        let mut result = Vec::new();
        result.extend_from_slice(&salt);
        result.extend_from_slice(&nonce_bytes);
        result.extend_from_slice(&password_bytes);

        use base64::{Engine as _, engine::general_purpose};
        Ok(general_purpose::STANDARD.encode(&result))
    }

    /// Decrypt a password using AES-256-GCM
    fn decrypt_legacy(&self, encrypted_password: &str) -> Result<String> {
        // Decode from base64
        use base64::{Engine as _, engine::general_purpose};
        let encrypted_data = general_purpose::STANDARD
            .decode(encrypted_password)
            .map_err(|_| AppError::EncryptionError("Invalid base64 encoding".to_string()))?;

        // Check minimum length (salt + nonce + at least some encrypted data)
        if encrypted_data.len() < SALT_LEN + NONCE_LEN + 16 {
            return Err(AppError::EncryptionError(
                "Invalid encrypted data length".to_string(),
            ));
        }

        // Extract salt, nonce, and encrypted password
        let salt = &encrypted_data[0..SALT_LEN];
        let nonce_bytes = &encrypted_data[SALT_LEN..SALT_LEN + NONCE_LEN];
        let mut encrypted_password_bytes = encrypted_data[SALT_LEN + NONCE_LEN..].to_vec();

        // Derive key from salt
        let key_bytes = self.derive_key(salt)?;

        // Create decryption key
        let unbound_key = UnboundKey::new(&AES_256_GCM, &key_bytes).map_err(|_| {
            AppError::EncryptionError("Failed to create decryption key".to_string())
        })?;
        let key = LessSafeKey::new(unbound_key);

        // Create nonce
        let nonce_array: [u8; NONCE_LEN] = nonce_bytes
            .try_into()
            .map_err(|_| AppError::EncryptionError("Invalid nonce length".to_string()))?;
        let nonce = Nonce::assume_unique_for_key(nonce_array);

        // Decrypt password
        let decrypted_bytes = key
            .open_in_place(nonce, Aad::empty(), &mut encrypted_password_bytes)
            .map_err(|_| AppError::EncryptionError("Failed to decrypt password".to_string()))?;

        // Convert to string
        String::from_utf8(decrypted_bytes.to_vec()).map_err(|_| {
            AppError::EncryptionError("Invalid UTF-8 in decrypted password".to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn protected_envelopes_roundtrip_reject_tampering_and_unknown_schemes() {
        let enc = PasswordEncryption::new();
        for password in ["", "临时_🔐", " leading and trailing "] {
            let a = enc.encrypt_password(password).unwrap();
            let b = enc.encrypt_password(password).unwrap();
            assert!(a != b);
            assert!(enc.decrypt_password(&a).unwrap() == password);
            let (scheme, base64) = a.split_once(':').unwrap();
            assert_eq!(scheme, platform::SCHEME);
            let mut blob = STANDARD.decode(base64).unwrap();
            *blob.last_mut().unwrap() ^= 0xff;
            assert!(
                enc.decrypt_password(&format!("{scheme}:{}", STANDARD.encode(blob)))
                    .is_err()
            );
            assert!(enc.decrypt_password(&format!("future:{base64}")).is_err());
        }
    }
    #[test]
    fn old_passwords_still_decrypt_and_invalid_legacy_input_fails() {
        let enc = PasswordEncryption::new();
        let sample = "synthetic-legacy-passphrase";
        let old = enc.encrypt_legacy(sample).unwrap();
        assert!(!old.contains(':') && enc.decrypt_password(&old).unwrap() == sample);
        let mut damaged = STANDARD.decode(old).unwrap();
        *damaged.last_mut().unwrap() ^= 1;
        for invalid in [
            "!?".into(),
            STANDARD.encode([0; 4]),
            STANDARD.encode(damaged),
        ] {
            assert!(enc.decrypt_password(&invalid).is_err());
        }
    }
}
