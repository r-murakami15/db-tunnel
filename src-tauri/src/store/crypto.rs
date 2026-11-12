//! 設定ファイルの暗号化・復号（AES-256-GCM）。
//!
//! ファイルの構造: マジック `RDSC`(4) | 形式バージョン 0x01(1) | nonce(12) | 暗号文 + 認証タグ(16)
//! 追加認証データ（AAD）にはマジックとバージョンの5バイトを使う。

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::rngs::OsRng;
use rand::RngCore;

pub const MAGIC: &[u8; 4] = b"RDSC";
pub const FORMAT_VERSION: u8 = 0x01;
pub const NONCE_LEN: usize = 12;
pub const TAG_LEN: usize = 16;
pub const HEADER_LEN: usize = MAGIC.len() + 1;

/// 復号に失敗した理由（利用者向けの日本語）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecryptError {
    TooShort,
    BadMagic,
    UnsupportedVersion(u8),
    /// 認証タグの検証に失敗（改ざん・鍵違い）
    AuthFailed,
}

impl std::fmt::Display for DecryptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecryptError::TooShort => write!(f, "ファイルが短すぎます"),
            DecryptError::BadMagic => write!(f, "設定ファイルの形式ではありません"),
            DecryptError::UnsupportedVersion(v) => {
                write!(f, "未対応の形式バージョンです（{v}）")
            }
            DecryptError::AuthFailed => {
                write!(
                    f,
                    "復号できません（鍵が違うか、ファイルが改ざんされています）"
                )
            }
        }
    }
}

fn header() -> [u8; HEADER_LEN] {
    let mut h = [0u8; HEADER_LEN];
    h[..4].copy_from_slice(MAGIC);
    h[4] = FORMAT_VERSION;
    h
}

/// 新しい32バイトの鍵を生成する
pub fn generate_key() -> zeroize::Zeroizing<[u8; 32]> {
    let mut key = zeroize::Zeroizing::new([0u8; 32]);
    OsRng.fill_bytes(key.as_mut());
    key
}

/// 平文を暗号化してファイルの中身（ヘッダ付き）を返す。nonce は毎回ランダム
pub fn encrypt(key: &[u8; 32], plaintext: &[u8]) -> Result<Vec<u8>, String> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| e.to_string())?;
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let aad = header();
    let ct = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|_| "暗号化に失敗しました".to_string())?;
    let mut out = Vec::with_capacity(HEADER_LEN + NONCE_LEN + ct.len());
    out.extend_from_slice(&aad);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(out)
}

/// ファイルの中身を復号して平文を返す
pub fn decrypt(key: &[u8; 32], data: &[u8]) -> Result<zeroize::Zeroizing<Vec<u8>>, DecryptError> {
    if data.len() < HEADER_LEN {
        return Err(DecryptError::TooShort);
    }
    if &data[..4] != MAGIC {
        return Err(DecryptError::BadMagic);
    }
    if data[4] != FORMAT_VERSION {
        return Err(DecryptError::UnsupportedVersion(data[4]));
    }
    if data.len() < HEADER_LEN + NONCE_LEN + TAG_LEN {
        return Err(DecryptError::TooShort);
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| DecryptError::AuthFailed)?;
    let nonce = &data[HEADER_LEN..HEADER_LEN + NONCE_LEN];
    let ct = &data[HEADER_LEN + NONCE_LEN..];
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ct,
                aad: &data[..HEADER_LEN],
            },
        )
        .map(zeroize::Zeroizing::new)
        .map_err(|_| DecryptError::AuthFailed)
}

/// 鍵を16進文字列にする（キーチェーン保存用）
pub fn hex_encode(bytes: &[u8]) -> zeroize::Zeroizing<String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    zeroize::Zeroizing::new(s)
}

/// 16進文字列（64文字）を32バイトの鍵に戻す。形式が違えば None
pub fn hex_decode_key(s: &str) -> Option<zeroize::Zeroizing<[u8; 32]>> {
    fn nibble(c: u8) -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    }
    let s = s.trim().as_bytes();
    if s.len() != 64 {
        return None;
    }
    let mut key = zeroize::Zeroizing::new([0u8; 32]);
    for (i, pair) in s.chunks_exact(2).enumerate() {
        key[i] = (nibble(pair[0])? << 4) | nibble(pair[1])?;
    }
    Some(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let key = generate_key();
        let enc = encrypt(&key, b"hello").unwrap();
        assert_eq!(&enc[..4], b"RDSC");
        assert_eq!(enc[4], 1);
        assert_eq!(enc.len(), HEADER_LEN + NONCE_LEN + 5 + TAG_LEN);
        assert_eq!(&decrypt(&key, &enc).unwrap()[..], b"hello");
        // nonce は毎回変わる
        let enc2 = encrypt(&key, b"hello").unwrap();
        assert_ne!(enc, enc2);
    }

    #[test]
    fn tamper_and_wrong_key() {
        let key = generate_key();
        let enc = encrypt(&key, b"hello world").unwrap();
        for i in 0..enc.len() {
            let mut t = enc.clone();
            t[i] ^= 0x01;
            assert!(decrypt(&key, &t).is_err(), "byte {i}");
        }
        let other = generate_key();
        assert_eq!(decrypt(&other, &enc), Err(DecryptError::AuthFailed));
        let mut bad = enc.clone();
        bad[0] = b'X';
        assert_eq!(decrypt(&key, &bad), Err(DecryptError::BadMagic));
        bad = enc.clone();
        bad[4] = 2;
        assert_eq!(
            decrypt(&key, &bad),
            Err(DecryptError::UnsupportedVersion(2))
        );
        assert_eq!(decrypt(&key, b"RD"), Err(DecryptError::TooShort));
    }

    #[test]
    fn hex() {
        let key = generate_key();
        let h = hex_encode(&key[..]);
        assert_eq!(h.len(), 64);
        assert_eq!(*hex_decode_key(&h).unwrap(), *key);
        assert!(hex_decode_key("abc").is_none());
        assert!(hex_decode_key(&"zz".repeat(32)).is_none());
    }
}
