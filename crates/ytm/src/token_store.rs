//! Refresh-token persistence. Native credential-store calls run off the async/UI thread.

use std::sync::{Arc, Mutex};

use keyring::{Entry, Error};

const SERVICE: &str = "lumina.youtube-music";

#[derive(Debug)]
pub(crate) struct StoreError;

trait CredentialStore: Send {
    fn load(&mut self) -> Result<Option<String>, StoreError>;
    fn save(&mut self, token: &str) -> Result<(), StoreError>;
    fn delete(&mut self) -> Result<(), StoreError>;
}

#[derive(Clone)]
pub(crate) struct TokenStore {
    backend: Arc<Mutex<Box<dyn CredentialStore>>>,
}

impl TokenStore {
    pub fn new(client_id: &str) -> Self {
        Self::with_backend(NativeStore {
            client_id: client_id.to_owned(),
        })
    }

    fn with_backend(backend: impl CredentialStore + 'static) -> Self {
        Self {
            backend: Arc::new(Mutex::new(Box::new(backend))),
        }
    }

    pub async fn load(&self) -> Result<Option<String>, StoreError> {
        self.run(|store| store.load()).await
    }

    pub async fn save(&self, token: &str) -> Result<(), StoreError> {
        let token = token.to_owned();
        self.run(move |store| store.save(&token)).await
    }

    pub async fn delete(&self) -> Result<(), StoreError> {
        self.run(|store| store.delete()).await
    }

    async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut dyn CredentialStore) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, StoreError> {
        let backend = self.backend.clone();
        tokio::task::spawn_blocking(move || {
            let mut store = backend.lock().map_err(|_| StoreError)?;
            operation(store.as_mut())
        })
        .await
        .map_err(|_| StoreError)?
    }

    #[cfg(test)]
    pub(crate) fn memory() -> Self {
        Self::with_backend(MemoryStore(None))
    }
}

struct NativeStore {
    client_id: String,
}

impl NativeStore {
    fn entry(&self) -> Result<Entry, StoreError> {
        Entry::new(SERVICE, &self.client_id).map_err(|_| StoreError)
    }
}

impl CredentialStore for NativeStore {
    fn load(&mut self) -> Result<Option<String>, StoreError> {
        match self.entry()?.get_password() {
            Ok(token) => Ok(Some(token)),
            Err(Error::NoEntry) => Ok(None),
            // Keyring errors can contain secret bytes; never propagate their payloads.
            Err(_) => Err(StoreError),
        }
    }

    fn save(&mut self, token: &str) -> Result<(), StoreError> {
        self.entry()?.set_password(token).map_err(|_| StoreError)
    }

    fn delete(&mut self) -> Result<(), StoreError> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(Error::NoEntry) => Ok(()),
            Err(_) => Err(StoreError),
        }
    }
}

#[cfg(test)]
struct MemoryStore(Option<String>);

#[cfg(test)]
impl CredentialStore for MemoryStore {
    fn load(&mut self) -> Result<Option<String>, StoreError> {
        Ok(self.0.clone())
    }

    fn save(&mut self, token: &str) -> Result<(), StoreError> {
        self.0 = Some(token.to_owned());
        Ok(())
    }

    fn delete(&mut self) -> Result<(), StoreError> {
        self.0 = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn saved_token_is_shared_replaced_and_deleted() {
        let store = TokenStore::memory();
        let restored = store.clone();
        assert_eq!(restored.load().await.unwrap(), None);
        store.save("original").await.unwrap();
        assert_eq!(restored.load().await.unwrap().as_deref(), Some("original"));
        store.save("rotated").await.unwrap();
        assert_eq!(restored.load().await.unwrap().as_deref(), Some("rotated"));
        restored.delete().await.unwrap();
        assert_eq!(store.load().await.unwrap(), None);
        store.delete().await.unwrap();
    }
}
