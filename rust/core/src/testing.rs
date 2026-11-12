//! Fakes for tests (feature `testing`).

use std::collections::HashMap;
use std::sync::Mutex;

use crate::secret::{Secret, SecretError, SecretRef, SecretStore};

/// A [`SecretStore`] that keeps secrets in memory.
#[derive(Default)]
pub struct MemorySecretStore {
    values: Mutex<HashMap<String, String>>,
}

impl MemorySecretStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, String>>, SecretError> {
        self.values
            .lock()
            .map_err(|_| SecretError::Store("poisoned lock".into()))
    }
}

impl SecretStore for MemorySecretStore {
    fn get(&self, reference: &SecretRef) -> Result<Option<Secret>, SecretError> {
        Ok(self
            .lock()?
            .get(reference.as_str())
            .map(|v| Secret::new(v.clone())))
    }

    fn set(&self, reference: &SecretRef, value: &Secret) -> Result<(), SecretError> {
        self.lock()?
            .insert(reference.as_str().to_owned(), value.expose().to_owned());
        Ok(())
    }

    fn delete(&self, reference: &SecretRef) -> Result<(), SecretError> {
        self.lock()?.remove(reference.as_str());
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_round_trip() {
        let store = MemorySecretStore::new();
        let key = SecretRef::new("HomeLink/router/x").unwrap();
        assert!(store.get(&key).unwrap().is_none());
        store.set(&key, &Secret::new("pw".into())).unwrap();
        assert_eq!(store.get(&key).unwrap().unwrap().expose(), "pw");
        store.delete(&key).unwrap();
        store.delete(&key).unwrap();
        assert!(store.get(&key).unwrap().is_none());
    }
}
