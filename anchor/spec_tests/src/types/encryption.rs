use base64::prelude::*;
use operator_key::{encrypted::EncryptedKey, public, unencrypted};
use serde::Deserialize;

use crate::{SpecTest, utils::deserializers::deserialize_base64};

/// Spec test for RSA key encryption/decryption roundtrip using Anchor's `operator_key` crate.
///
/// Adapted from Go's `types/spectest/tests/encryption/test.go`. Go tests raw RSA
/// encrypt/decrypt of plaintext, but Anchor uses EIP-2335 key encryption in production
/// (`EncryptedKey`), so we test that code path instead:
/// 1. Parse RSA private key via `unencrypted::from_base64`
/// 2. Derive public key via `public::to_base64`, verify it matches fixture's PKPem
/// 3. Encrypt private key with `EncryptedKey::encrypt` (plaintext as password)
/// 4. Decrypt with `EncryptedKey::decrypt`
/// 5. Assert roundtrip: decrypted key matches original
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct EncryptionSpecTest {
    #[serde(rename = "SKPem", deserialize_with = "deserialize_base64")]
    sk_pem: Vec<u8>,
    #[serde(rename = "PKPem", deserialize_with = "deserialize_base64")]
    pk_pem: Vec<u8>,
    #[serde(deserialize_with = "deserialize_base64")]
    plain_text: Vec<u8>,
}

impl SpecTest for EncryptionSpecTest {
    fn run(&self) -> Result<(), String> {
        // Parse private key using Anchor's unencrypted key module.
        // `from_base64` expects base64-encoded PEM, so re-encode the raw PEM bytes.
        let sk_b64 = BASE64_STANDARD.encode(&self.sk_pem);
        let private_key = unencrypted::from_base64(sk_b64.as_bytes())
            .map_err(|e| format!("Failed to parse private key: {e}"))?;

        // Derive public key and verify it matches fixture's PKPem.
        let derived_pk_b64 = public::to_base64(&private_key)
            .map_err(|e| format!("Failed to derive public key: {e}"))?;
        let expected_pk_b64 = BASE64_STANDARD.encode(&self.pk_pem);
        if derived_pk_b64 != expected_pk_b64 {
            return Err(
                "Public key derived from private key does not match fixture PKPem".to_string(),
            );
        }

        // Use plaintext as password for EIP-2335 key encryption.
        // Fall back to base64 encoding if plaintext isn't valid UTF-8 (e.g. BLS secret key).
        let password = String::from_utf8(self.plain_text.clone())
            .unwrap_or_else(|e| BASE64_STANDARD.encode(e.into_bytes()));

        // Encrypt using Anchor's EncryptedKey (EIP-2335)
        let encrypted = EncryptedKey::encrypt(&private_key, &password)
            .map_err(|e| format!("Encryption failed: {e}"))?;

        // Decrypt and verify roundtrip
        let decrypted = encrypted
            .decrypt(&password)
            .map_err(|e| format!("Decryption failed: {e}"))?;

        if private_key.p() != decrypted.p() || private_key.q() != decrypted.q() {
            return Err("Roundtrip failed: decrypted key does not match original".to_string());
        }

        Ok(())
    }
}
