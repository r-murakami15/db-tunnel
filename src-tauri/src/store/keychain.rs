//! 暗号化の鍵の保管場所（OSのキーチェーン / テスト用のメモリ）。

use std::sync::{Arc, Mutex};

use zeroize::Zeroizing;

use super::crypto::{hex_decode_key, hex_encode};
use super::StoreError;

const SERVICE: &str = "db-tunnel";
const ACCOUNT: &str = "config-key";

pub trait KeyProvider: Send + Sync {
    /// Returns the existing 32-byte key, or None if not present.
    fn load_key(&self) -> Result<Option<Zeroizing<[u8; 32]>>, StoreError>;
    /// Stores a newly generated key.
    fn store_key(&self, key: &[u8; 32]) -> Result<(), StoreError>;
}

/// `Arc<T>` でも使えるようにする（テストで同じ鍵を複数の Store で共有するため）
impl<T: KeyProvider + ?Sized> KeyProvider for Arc<T> {
    fn load_key(&self) -> Result<Option<Zeroizing<[u8; 32]>>, StoreError> {
        (**self).load_key()
    }
    fn store_key(&self, key: &[u8; 32]) -> Result<(), StoreError> {
        (**self).store_key(key)
    }
}

/// OSのキーチェーン（Mac: ログインキーチェーン、Windows: 資格情報マネージャー）に
/// 鍵を16進文字列で保存する。
pub struct KeychainKeyProvider;

impl KeychainKeyProvider {
    fn entry() -> Result<keyring::Entry, StoreError> {
        keyring::Entry::new(SERVICE, ACCOUNT).map_err(|e| StoreError::Keychain(e.to_string()))
    }
}

impl KeyProvider for KeychainKeyProvider {
    fn load_key(&self) -> Result<Option<Zeroizing<[u8; 32]>>, StoreError> {
        let entry = Self::entry()?;
        match entry.get_password() {
            Ok(s) => {
                let s = Zeroizing::new(s);
                // 形式が壊れた鍵は「鍵がない」と同じ扱い（どのみち復号できないため）
                Ok(hex_decode_key(&s))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(StoreError::Keychain(e.to_string())),
        }
    }

    fn store_key(&self, key: &[u8; 32]) -> Result<(), StoreError> {
        let entry = Self::entry()?;
        let hex = hex_encode(key);
        entry
            .set_password(&hex)
            .map_err(|e| StoreError::Keychain(e.to_string()))
    }
}

/// テスト用。鍵をメモリ上にだけ持つ
#[derive(Default)]
pub struct MemoryKeyProvider {
    key: Mutex<Option<[u8; 32]>>,
}

impl MemoryKeyProvider {
    pub fn new() -> Self {
        Self::default()
    }

    /// 最初から鍵を持った状態で作る
    pub fn with_key(key: [u8; 32]) -> Self {
        MemoryKeyProvider {
            key: Mutex::new(Some(key)),
        }
    }

    /// 保持している鍵を消す（「鍵が見つからない」状況の再現用）
    pub fn clear(&self) {
        if let Ok(mut k) = self.key.lock() {
            if let Some(mut old) = k.take() {
                zeroize::Zeroize::zeroize(&mut old);
            }
        }
    }
}

impl KeyProvider for MemoryKeyProvider {
    fn load_key(&self) -> Result<Option<Zeroizing<[u8; 32]>>, StoreError> {
        let k = self
            .key
            .lock()
            .map_err(|_| StoreError::Keychain("lock poisoned".into()))?;
        Ok(k.map(Zeroizing::new))
    }

    fn store_key(&self, key: &[u8; 32]) -> Result<(), StoreError> {
        let mut k = self
            .key
            .lock()
            .map_err(|_| StoreError::Keychain("lock poisoned".into()))?;
        *k = Some(*key);
        Ok(())
    }
}

impl Drop for MemoryKeyProvider {
    fn drop(&mut self) {
        self.clear();
    }
}
