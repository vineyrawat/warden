// OWNER: forge
//! Forge tokens in the macOS keychain via `keyring` (service "dev.warden.app").
//!
//! Tokens are never logged and never leave Warden's process except as the forge auth header
//! (hard rule 1: Claude never gets a forge token).

use std::collections::HashMap;
use std::sync::Mutex;

use crate::error::{Result, WardenError};
use crate::types::ForgeKind;

pub const KEYRING_SERVICE: &str = "dev.warden.app";

/// Keychain account name for a forge host, e.g. `gitlab:https://gitlab.com`.
/// (Convention fixed by scaffold so forge and integration agree.)
pub fn service_key(forge: ForgeKind, base_url: &str) -> String {
    format!("{}:{}", forge.as_str(), base_url.trim_end_matches('/'))
}

pub fn set_token(service_key: &str, token: &str) -> Result<()> {
    KeychainStore.set(service_key, token)
}

/// `Ok(None)` when no token is stored.
pub fn get_token(service_key: &str) -> Result<Option<String>> {
    KeychainStore.get(service_key)
}

/// Deleting a missing token is not an error.
pub fn delete_token(service_key: &str) -> Result<()> {
    KeychainStore.delete(service_key)
}

/// Storage seam so callers (and tests) can swap the keychain for an in-memory store.
pub trait TokenStore: Send + Sync {
    fn set(&self, service_key: &str, token: &str) -> Result<()>;
    fn get(&self, service_key: &str) -> Result<Option<String>>;
    fn delete(&self, service_key: &str) -> Result<()>;
}

/// The real store: macOS keychain (keyring `apple-native`), service [`KEYRING_SERVICE`].
#[derive(Debug, Clone, Copy, Default)]
pub struct KeychainStore;

fn validate_key(service_key: &str) -> Result<()> {
    if service_key.trim().is_empty() {
        return Err(WardenError::Config("empty keychain account name".into()));
    }
    Ok(())
}

fn entry(service_key: &str) -> Result<keyring::Entry> {
    validate_key(service_key)?;
    Ok(keyring::Entry::new(KEYRING_SERVICE, service_key)?)
}

impl TokenStore for KeychainStore {
    fn set(&self, service_key: &str, token: &str) -> Result<()> {
        let token = token.trim();
        if token.is_empty() {
            return Err(WardenError::Config(
                "refusing to store an empty token".into(),
            ));
        }
        entry(service_key)?.set_password(token)?;
        Ok(())
    }

    fn get(&self, service_key: &str) -> Result<Option<String>> {
        match entry(service_key)?.get_password() {
            Ok(t) => Ok(Some(t)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn delete(&self, service_key: &str) -> Result<()> {
        match entry(service_key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

/// Process-local store (tests, `review_local` smoke runs, or when the keychain is unavailable).
#[derive(Debug, Default)]
pub struct MemoryStore {
    inner: Mutex<HashMap<String, String>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }
}

impl TokenStore for MemoryStore {
    fn set(&self, service_key: &str, token: &str) -> Result<()> {
        validate_key(service_key)?;
        let token = token.trim();
        if token.is_empty() {
            return Err(WardenError::Config(
                "refusing to store an empty token".into(),
            ));
        }
        self.inner
            .lock()
            .map_err(|_| WardenError::Other("token store poisoned".into()))?
            .insert(service_key.to_string(), token.to_string());
        Ok(())
    }

    fn get(&self, service_key: &str) -> Result<Option<String>> {
        validate_key(service_key)?;
        Ok(self
            .inner
            .lock()
            .map_err(|_| WardenError::Other("token store poisoned".into()))?
            .get(service_key)
            .cloned())
    }

    fn delete(&self, service_key: &str) -> Result<()> {
        validate_key(service_key)?;
        self.inner
            .lock()
            .map_err(|_| WardenError::Other("token store poisoned".into()))?
            .remove(service_key);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_key_format() {
        assert_eq!(
            service_key(ForgeKind::Gitlab, "https://gitlab.com/"),
            "gitlab:https://gitlab.com"
        );
        assert_eq!(
            service_key(ForgeKind::Github, "https://github.com"),
            "github:https://github.com"
        );
    }

    #[test]
    fn memory_store_roundtrip() {
        let s = MemoryStore::new();
        let k = service_key(ForgeKind::Gitlab, "https://gl.example");
        assert_eq!(s.get(&k).unwrap(), None);
        s.set(&k, " glpat-abc \n").unwrap();
        assert_eq!(s.get(&k).unwrap().as_deref(), Some("glpat-abc"));
        s.set(&k, "glpat-new").unwrap();
        assert_eq!(s.get(&k).unwrap().as_deref(), Some("glpat-new"));
        s.delete(&k).unwrap();
        assert_eq!(s.get(&k).unwrap(), None);
        // deleting a missing token is fine
        s.delete(&k).unwrap();
    }

    #[test]
    fn rejects_empty_values() {
        let s = MemoryStore::new();
        assert!(s.set("gitlab:x", "   ").is_err());
        assert!(s.set("", "tok").is_err());
        assert!(s.get(" ").is_err());
        assert!(KeychainStore.set("gitlab:x", "").is_err());
        assert!(KeychainStore.get("").is_err());
    }

    /// Uses keyring's mock backend so the real macOS keychain is never touched in tests.
    /// (Mock credentials are per-`Entry`, so this checks NoEntry mapping, not persistence.)
    #[test]
    fn keychain_store_maps_no_entry_with_mock_backend() {
        keyring::set_default_credential_builder(keyring::mock::default_credential_builder());
        let k = "gitlab:https://mock.invalid";
        assert_eq!(get_token(k).unwrap(), None);
        delete_token(k).unwrap();
        set_token(k, "tok").unwrap();
    }
}
