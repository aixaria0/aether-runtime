use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;

pub const SIGNING_ALGORITHM: &str = "ed25519";
pub const RECEIPT_SIGNATURE_DOMAIN: &str = "aether.execution-receipt.v1";

#[derive(Clone)]
pub struct Identity {
    signing_key: SigningKey,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IdentityInfo {
    pub algorithm: &'static str,
    pub public_key_hex: String,
    pub fingerprint_sha256: String,
}

impl Identity {
    pub fn load_or_create(path: impl Into<PathBuf>) -> Result<Self, String> {
        let key_path = path.into();
        if let Some(parent) = key_path.parent() {
            if !parent.as_os_str().is_empty() && parent != Path::new(".") {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("create identity directory: {e}"))?;
            }
        }

        let signing_key = if key_path.exists() {
            read_signing_key(&key_path)?
        } else {
            let mut rng = OsRng;
            let generated = SigningKey::generate(&mut rng);
            if persist_signing_key(&key_path, &generated)? {
                generated
            } else {
                // Another process won the create_new race. Use the persisted winner.
                read_signing_key(&key_path)?
            }
        };

        Ok(Self { signing_key })
    }

    pub fn info(&self) -> IdentityInfo {
        let verifying = self.signing_key.verifying_key();
        let public_key_hex = hex::encode(verifying.to_bytes());
        IdentityInfo {
            algorithm: SIGNING_ALGORITHM,
            fingerprint_sha256: sha256_hex(&verifying.to_bytes()),
            public_key_hex,
        }
    }

    pub fn sign_domain(&self, domain: &str, payload: &[u8]) -> String {
        let message = domain_message(domain, payload);
        let signature = self.signing_key.sign(&message);
        hex::encode(signature.to_bytes())
    }

    pub fn verify_domain(
        public_key_hex: &str,
        signature_hex: &str,
        domain: &str,
        payload: &[u8],
    ) -> bool {
        let public_key = match decode_fixed::<32>(public_key_hex) {
            Some(value) => value,
            None => return false,
        };
        let signature_bytes = match decode_fixed::<64>(signature_hex) {
            Some(value) => value,
            None => return false,
        };
        let verifying_key = match VerifyingKey::from_bytes(&public_key) {
            Ok(value) => value,
            Err(_) => return false,
        };
        let signature = Signature::from_bytes(&signature_bytes);
        verifying_key
            .verify(&domain_message(domain, payload), &signature)
            .is_ok()
    }

    pub fn fingerprint_public_key_hex(public_key_hex: &str) -> Option<String> {
        decode_fixed::<32>(public_key_hex).map(|bytes| sha256_hex(&bytes))
    }
}

fn read_signing_key(path: &Path) -> Result<SigningKey, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(path)
            .map_err(|e| format!("read identity metadata: {e}"))?
            .permissions()
            .mode()
            & 0o777;
        if mode & 0o077 != 0 {
            return Err(format!(
                "identity key permissions must be owner-only (0600); found {mode:04o}"
            ));
        }
    }

    let bytes = fs::read(path).map_err(|e| format!("read identity key: {e}"))?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "identity key must contain exactly 32 bytes".to_string())?;
    Ok(SigningKey::from_bytes(&seed))
}

fn persist_signing_key(path: &Path, key: &SigningKey) -> Result<bool, String> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);

    match options.open(path) {
        Ok(mut file) => {
            file.write_all(&key.to_bytes())
                .map_err(|e| format!("write identity key: {e}"))?;
            file.sync_all()
                .map_err(|e| format!("sync identity key: {e}"))?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(format!("create identity key: {error}")),
    }
}

fn domain_message(domain: &str, payload: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(8 + domain.len() + payload.len());
    message.extend_from_slice(&(domain.len() as u64).to_be_bytes());
    message.extend_from_slice(domain.as_bytes());
    message.extend_from_slice(payload);
    message
}

fn decode_fixed<const N: usize>(value: &str) -> Option<[u8; N]> {
    let bytes = hex::decode(value).ok()?;
    bytes.try_into().ok()
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn path() -> PathBuf {
        std::env::temp_dir().join(format!("aether-identity-test-{}.key", Uuid::new_v4()))
    }

    #[test]
    fn identity_persists_across_reopen() {
        let path = path();
        let first = Identity::load_or_create(&path).unwrap();
        let first_info = first.info();
        drop(first);
        let second = Identity::load_or_create(&path).unwrap();
        assert_eq!(first_info.public_key_hex, second.info().public_key_hex);
        assert_eq!(
            first_info.fingerprint_sha256,
            second.info().fingerprint_sha256
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn signatures_reject_tampering_wrong_domain_and_wrong_keys() {
        let path_a = path();
        let path_b = path();
        let a = Identity::load_or_create(&path_a).unwrap();
        let b = Identity::load_or_create(&path_b).unwrap();
        let signature = a.sign_domain("aether-test-v1", b"payload");
        assert!(Identity::verify_domain(
            &a.info().public_key_hex,
            &signature,
            "aether-test-v1",
            b"payload"
        ));
        assert!(!Identity::verify_domain(
            &a.info().public_key_hex,
            &signature,
            "aether-test-v1",
            b"tampered"
        ));
        assert!(!Identity::verify_domain(
            &a.info().public_key_hex,
            &signature,
            "wrong-domain",
            b"payload"
        ));
        assert!(!Identity::verify_domain(
            &b.info().public_key_hex,
            &signature,
            "aether-test-v1",
            b"payload"
        ));
        let _ = fs::remove_file(path_a);
        let _ = fs::remove_file(path_b);
    }

    #[cfg(unix)]
    #[test]
    fn new_identity_key_is_owner_only_and_insecure_existing_key_is_rejected() {
        use std::os::unix::fs::PermissionsExt;
        let path = path();
        Identity::load_or_create(&path).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Identity::load_or_create(&path).is_err());
        let _ = fs::remove_file(path);
    }
}
