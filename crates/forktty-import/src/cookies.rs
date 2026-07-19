//! Read + decrypt cookies from source browsers. Firefox cookies are plaintext;
//! Chromium-family `encrypted_value` blobs are AES-128-CBC (Linux v10/v11).

use std::path::Path;

use aes::cipher::{block_padding::Pkcs7, BlockModeDecrypt, KeyIvInit};
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use sha2::{Digest, Sha256};

use crate::model::{BrowserFamily, ImportedCookie};

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

/// Keys used by Linux Chromium-family cookie encryption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromiumCookieKeys {
    pub v10: [u8; 16],
    pub v11: Option<[u8; 16]>,
}

impl ChromiumCookieKeys {
    pub fn v10_only() -> Self {
        Self {
            v10: chromium_v10_key(),
            v11: None,
        }
    }

    pub async fn for_family(family: BrowserFamily) -> Self {
        let v11 = match family.safe_storage_label() {
            Some(label) => chromium_v11_key(label).await,
            None => None,
        };
        Self {
            v10: chromium_v10_key(),
            v11,
        }
    }
}

/// Derive the Chromium Linux AES key from a Safe Storage password.
pub fn chromium_key_from_password(password: &[u8]) -> [u8; 16] {
    let mut key = [0u8; 16];
    pbkdf2::pbkdf2::<hmac::Hmac<sha1::Sha1>>(password, b"saltysalt", 1, &mut key)
        .expect("pbkdf2 into a 16-byte buffer never fails");
    key
}

/// The Linux Chromium `v10` key: PBKDF2-HMAC-SHA1("peanuts", "saltysalt", 1, 16B).
pub fn chromium_v10_key() -> [u8; 16] {
    chromium_key_from_password(b"peanuts")
}

fn decrypt_chromium_value_bytes(blob: &[u8], key: &[u8; 16]) -> Option<Vec<u8>> {
    if blob.len() < 3 {
        return None;
    }
    let prefix = &blob[0..3];
    if prefix != b"v10" && prefix != b"v11" {
        return None;
    }
    let ct = &blob[3..];
    if ct.is_empty() || !ct.len().is_multiple_of(16) {
        return None;
    }
    let iv = [0x20u8; 16];
    let mut buf = ct.to_vec();
    let pt = Aes128CbcDec::new(key.into(), &iv.into())
        .decrypt_padded::<Pkcs7>(&mut buf)
        .ok()?;
    Some(pt.to_vec())
}

/// Decrypt a Chromium `encrypted_value` blob with `key`. Returns `None` (caller skips
/// + counts) on any structural or decode failure — never panics.
pub fn decrypt_chromium_value(blob: &[u8], key: &[u8; 16]) -> Option<String> {
    String::from_utf8(decrypt_chromium_value_bytes(blob, key)?).ok()
}

fn decrypt_chromium_cookie_value(
    blob: &[u8],
    keys: &ChromiumCookieKeys,
    db_version: Option<i64>,
    host_key: &str,
) -> Option<String> {
    let prefix = blob.get(0..3)?;
    let key = match prefix {
        b"v10" => &keys.v10,
        b"v11" => keys.v11.as_ref()?,
        _ => return None,
    };
    let pt = decrypt_chromium_value_bytes(blob, key)?;

    // Chromium cookie DB version 24+ prepends SHA256(host_key) to the encrypted
    // plaintext. The digest is an integrity check; reject the cookie if the
    // plaintext is too short or if the digest is not bound to this row's host.
    if db_version.is_some_and(|version| version >= 24) {
        let (host_digest, value) = pt.split_at_checked(32)?;
        if host_digest != Sha256::digest(host_key.as_bytes()).as_slice() {
            return None;
        }
        return String::from_utf8(value.to_vec()).ok();
    }

    String::from_utf8(pt).ok()
}

/// The Linux Chromium `v11` key: read password from Secret Service under `label`,
/// then PBKDF2-HMAC-SHA1(password, "saltysalt", 1, 16B).
///
/// Requires D-Bus + unlocked keyring at runtime. Returns `None` on any error
/// (missing D-Bus, locked keyring, item not found) so callers fall back to v10.
#[cfg(feature = "keyring")]
pub async fn chromium_v11_key(label: &str) -> Option<[u8; 16]> {
    chromium_safe_storage_secret(label)
        .await
        .ok()
        .map(|password| chromium_key_from_password(&password))
}

#[cfg(not(feature = "keyring"))]
pub async fn chromium_v11_key(_label: &str) -> Option<[u8; 16]> {
    None
}

#[cfg(feature = "keyring")]
async fn chromium_safe_storage_secret(label: &str) -> Result<Vec<u8>, secret_service::Error> {
    use std::collections::HashMap;

    use secret_service::{EncryptionType, SecretService};

    let service = SecretService::connect(EncryptionType::Dh).await?;
    let items = service.search_items(HashMap::new()).await?;

    for item in &items.unlocked {
        if item.get_label().await? == label {
            return item.get_secret().await;
        }
    }

    for item in &items.locked {
        if item.get_label().await? == label {
            item.unlock().await?;
            return item.get_secret().await;
        }
    }

    Err(secret_service::Error::NoResult)
}

fn open_ro(path: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// Read Firefox plaintext cookies from `cookies.sqlite`.
pub fn read_firefox_cookies(db: &Path) -> rusqlite::Result<Vec<ImportedCookie>> {
    let conn = open_ro(db)?;
    let mut stmt = conn
        .prepare("SELECT name, value, host, path, expiry, isSecure, isHttpOnly FROM moz_cookies")?;
    let rows = stmt
        .query_map([], |row| {
            let expiry: i64 = row.get(4)?;
            Ok(ImportedCookie {
                name: row.get(0)?,
                value: row.get(1)?,
                host: row.get(2)?,
                path: row.get(3)?,
                expires: if expiry == 0 { None } else { Some(expiry) },
                secure: row.get::<_, i64>(5)? != 0,
                http_only: row.get::<_, i64>(6)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Read Chromium-family cookies, decrypting `encrypted_value` with `key`. Returns
/// `(cookies, skipped)` where `skipped` counts rows that failed to decrypt.
pub fn read_chromium_cookies(
    db: &Path,
    key: &[u8; 16],
) -> rusqlite::Result<(Vec<ImportedCookie>, usize)> {
    read_chromium_cookies_with_keys(
        db,
        &ChromiumCookieKeys {
            v10: *key,
            v11: None,
        },
    )
}

/// Read Chromium-family cookies with prefix-aware v10/v11 keys.
pub fn read_chromium_cookies_with_keys(
    db: &Path,
    keys: &ChromiumCookieKeys,
) -> rusqlite::Result<(Vec<ImportedCookie>, usize)> {
    let conn = open_ro(db)?;
    let db_version = chromium_cookie_db_version(&conn)?;
    let mut stmt = conn.prepare(
        "SELECT host_key, name, value, encrypted_value, path, expires_utc, is_secure, is_httponly FROM cookies",
    )?;
    let mut cookies = Vec::new();
    let mut skipped = 0usize;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let host: String = row.get(0)?;
        let name: String = row.get(1)?;
        let plain: String = row.get(2)?;
        let enc: Vec<u8> = row.get(3)?;
        let path: String = row.get(4)?;
        let expires_utc: i64 = row.get(5)?;
        let secure: bool = row.get::<_, i64>(6)? != 0;
        let http_only: bool = row.get::<_, i64>(7)? != 0;

        let value = if enc.is_empty() {
            plain
        } else {
            match decrypt_chromium_cookie_value(&enc, keys, db_version, &host) {
                Some(v) => v,
                None => {
                    skipped += 1;
                    continue;
                }
            }
        };
        // Chromium epoch is 1601-01-01 in microseconds.
        let expires = if expires_utc == 0 {
            None
        } else {
            let unix = expires_utc / 1_000_000 - 11_644_473_600;
            if unix < 0 {
                None
            } else {
                Some(unix)
            }
        };
        cookies.push(ImportedCookie {
            name,
            value,
            host,
            path,
            expires,
            secure,
            http_only,
        });
    }
    Ok((cookies, skipped))
}

fn chromium_cookie_db_version(conn: &Connection) -> rusqlite::Result<Option<i64>> {
    let mut stmt = match conn.prepare("SELECT value FROM meta WHERE key = 'version'") {
        Ok(stmt) => stmt,
        Err(e) if e.to_string().contains("no such table") => return Ok(None),
        Err(e) => return Err(e),
    };
    match stmt.query_row([], |row| match row.get_ref(0)? {
        ValueRef::Integer(version) => Ok(Some(version)),
        ValueRef::Text(bytes) => Ok(std::str::from_utf8(bytes)
            .ok()
            .and_then(|text| text.parse::<i64>().ok())),
        _ => Ok(None),
    }) {
        Ok(version) => Ok(version),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // A Chromium blob = version || AES-128-CBC(key, IV=16 spaces, PKCS7(plaintext)).
    fn make_chromium_blob(version: &[u8; 3], key: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
        use aes::cipher::{block_padding::Pkcs7, BlockModeEncrypt, KeyIvInit};
        type Enc = cbc::Encryptor<aes::Aes128>;
        let iv = [0x20u8; 16];
        let mut buf = plaintext.to_vec();
        let pad_len = 16 - (buf.len() % 16);
        buf.extend(std::iter::repeat_n(0u8, pad_len)); // room for padding
        let ct = Enc::new(key.into(), &iv.into())
            .encrypt_padded::<Pkcs7>(&mut buf, plaintext.len())
            .unwrap()
            .to_vec();
        let mut out = version.to_vec();
        out.extend_from_slice(&ct);
        out
    }

    fn make_v10_blob(key: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
        make_chromium_blob(b"v10", key, plaintext)
    }

    #[test]
    fn test_chromium_cookie_keys_v10_only() {
        let keys = ChromiumCookieKeys::v10_only();
        assert_eq!(keys.v10, chromium_v10_key());
        assert_eq!(keys.v11, None);
    }

    #[test]
    fn decrypt_chromium_value_test_pure_deterministic() {
        let key = chromium_v10_key();
        let plaintext = b"pure_deterministic_test";
        let blob = make_v10_blob(&key, plaintext);
        let decoded = decrypt_chromium_value(&blob, &key).unwrap();
        assert_eq!(decoded, "pure_deterministic_test");

        // Edge case: empty blob
        assert!(decrypt_chromium_value(b"", &key).is_none());

        // Edge case: malformed version prefix
        let mut malformed_prefix = blob.clone();
        malformed_prefix[0..3].copy_from_slice(b"v99");
        assert!(decrypt_chromium_value(&malformed_prefix, &key).is_none());

        // Edge case: valid prefix but invalid length (not a multiple of block size 16)
        let invalid_length = &blob[0..blob.len() - 1];
        assert!(decrypt_chromium_value(invalid_length, &key).is_none());

        // Edge case: valid length but invalid padding/decryption
        let mut invalid_cipher = blob.clone();
        invalid_cipher[5] ^= 0xFF; // corrupt ciphertext
        assert!(decrypt_chromium_value(&invalid_cipher, &key).is_none());

        // Edge case: decryption succeeds but bytes are invalid UTF-8
        let invalid_utf8_plaintext = b"\xFF\xFF\xFF";
        let invalid_utf8_blob = make_v10_blob(&key, invalid_utf8_plaintext);
        assert!(decrypt_chromium_value(&invalid_utf8_blob, &key).is_none());
    }

    #[test]
    fn v10_key_is_pbkdf2_peanuts() {
        // Known vector: PBKDF2-HMAC-SHA1("peanuts", "saltysalt", 1, 16).
        let key = chromium_v10_key();
        assert_eq!(key.len(), 16);
        assert_eq!(
            key,
            [
                0xfd, 0x62, 0x1f, 0xe5, 0xa2, 0xb4, 0x02, 0x53, 0x9d, 0xfa, 0x14, 0x7c, 0xa9, 0x27,
                0x27, 0x78
            ]
        );
    }

    #[test]
    fn decrypt_v10_roundtrip() {
        let key = chromium_v10_key();
        let blob = make_v10_blob(&key, b"session=abc123");
        let value = decrypt_chromium_value(&blob, &key).unwrap();
        assert_eq!(value, "session=abc123");
    }

    #[test]
    fn decrypt_rejects_unknown_prefix() {
        let key = chromium_v10_key();
        assert!(decrypt_chromium_value(b"v99garbage", &key).is_none());
        assert!(decrypt_chromium_value(b"", &key).is_none());
    }

    #[test]
    fn read_firefox_cookies_from_synthetic_sqlite() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("cookies.sqlite");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE moz_cookies (
                 name TEXT, value TEXT, host TEXT, path TEXT,
                 expiry INTEGER, isSecure INTEGER, isHttpOnly INTEGER);
             INSERT INTO moz_cookies VALUES ('sid','xyz','.a.test','/',1893456000,1,1);",
        )
        .unwrap();
        drop(conn);
        let cookies = read_firefox_cookies(&db).unwrap();
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "sid");
        assert_eq!(cookies[0].value, "xyz");
        assert_eq!(cookies[0].host, ".a.test");
        assert!(cookies[0].secure);
        assert!(cookies[0].http_only);
        assert_eq!(cookies[0].expires, Some(1893456000));
    }

    #[test]
    fn read_chromium_cookies_decrypts_with_key() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("Cookies");
        let key = chromium_v10_key();
        let blob = make_v10_blob(&key, b"tok");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE cookies (
                 host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB,
                 path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cookies VALUES ('.a.test','sid','',?1,'/',13300000000000000,1,0)",
            rusqlite::params![blob],
        )
        .unwrap();
        drop(conn);
        let (cookies, skipped) = read_chromium_cookies(&db, &key).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].name, "sid");
        assert_eq!(cookies[0].value, "tok");
        assert_eq!(cookies[0].host, ".a.test");
        assert!(cookies[0].secure);
        assert!(!cookies[0].http_only);
    }

    #[test]
    fn read_chromium_cookies_strips_valid_modern_host_hash_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("Cookies");
        let key = chromium_v10_key();
        let mut plaintext = Sha256::digest(b".a.test").to_vec();
        plaintext.extend_from_slice(b"tok");
        let blob = make_v10_blob(&key, &plaintext);
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
             INSERT INTO meta VALUES ('version','24');
             CREATE TABLE cookies (
                 host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB,
                 path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cookies VALUES ('.a.test','sid','',?1,'/',0,0,0)",
            rusqlite::params![blob],
        )
        .unwrap();
        drop(conn);
        let (cookies, skipped) = read_chromium_cookies(&db, &key).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].value, "tok");
    }

    #[test]
    fn read_chromium_cookies_rejects_invalid_modern_host_hash_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("Cookies");
        let key = chromium_v10_key();
        let mut plaintext = vec![0xff; 32];
        plaintext.extend_from_slice(b"tok");
        let blob = make_v10_blob(&key, &plaintext);
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT);
             INSERT INTO meta VALUES ('version','24');
             CREATE TABLE cookies (
                 host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB,
                 path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cookies VALUES ('.a.test','sid','',?1,'/',0,0,0)",
            rusqlite::params![blob],
        )
        .unwrap();
        drop(conn);

        let (cookies, skipped) = read_chromium_cookies(&db, &key).unwrap();

        assert!(cookies.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn read_chromium_cookies_decrypts_v11_with_safe_storage_key() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("Cookies");
        let v11_key = chromium_key_from_password(b"safe-storage-secret");
        let blob = make_chromium_blob(b"v11", &v11_key, b"modern-token");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE cookies (
                 host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB,
                 path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cookies VALUES ('.a.test','sid','',?1,'/',0,0,0)",
            rusqlite::params![blob],
        )
        .unwrap();
        drop(conn);

        let keys = ChromiumCookieKeys {
            v10: chromium_v10_key(),
            v11: Some(v11_key),
        };
        let (cookies, skipped) = read_chromium_cookies_with_keys(&db, &keys).unwrap();

        assert_eq!(skipped, 0);
        assert_eq!(cookies.len(), 1);
        assert_eq!(cookies[0].value, "modern-token");
    }

    #[test]
    fn read_chromium_cookies_skips_v11_when_keyring_key_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("Cookies");
        let v11_key = chromium_key_from_password(b"safe-storage-secret");
        let blob = make_chromium_blob(b"v11", &v11_key, b"modern-token");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE cookies (
                 host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB,
                 path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cookies VALUES ('.a.test','sid','',?1,'/',0,0,0)",
            rusqlite::params![blob],
        )
        .unwrap();
        drop(conn);

        let (cookies, skipped) =
            read_chromium_cookies_with_keys(&db, &ChromiumCookieKeys::v10_only()).unwrap();

        assert!(cookies.is_empty());
        assert_eq!(skipped, 1);
    }

    #[test]
    fn read_chromium_cookies_counts_skips_on_bad_blob() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("Cookies");
        let key = chromium_v10_key();
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE cookies (
                 host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB,
                 path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO cookies VALUES ('.a.test','bad','',?1,'/',0,0,0)",
            rusqlite::params![b"v99nonsense".to_vec()],
        )
        .unwrap();
        drop(conn);
        let (cookies, skipped) = read_chromium_cookies(&db, &key).unwrap();
        assert!(cookies.is_empty());
        assert_eq!(skipped, 1);
    }
}
