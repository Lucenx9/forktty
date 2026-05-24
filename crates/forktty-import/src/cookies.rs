//! Read + decrypt cookies from source browsers. Firefox cookies are plaintext;
//! Chromium-family `encrypted_value` blobs are AES-128-CBC (Linux v10/v11).

use std::path::Path;

use aes::cipher::{block_padding::Pkcs7, BlockDecryptMut, KeyIvInit};
use rusqlite::{Connection, OpenFlags};

use crate::model::ImportedCookie;

type Aes128CbcDec = cbc::Decryptor<aes::Aes128>;

/// The Linux Chromium `v10` key: PBKDF2-HMAC-SHA1("peanuts", "saltysalt", 1, 16B).
pub fn chromium_v10_key() -> [u8; 16] {
    let mut key = [0u8; 16];
    pbkdf2::pbkdf2::<hmac::Hmac<sha1::Sha1>>(b"peanuts", b"saltysalt", 1, &mut key)
        .expect("pbkdf2 into a 16-byte buffer never fails");
    key
}

/// Decrypt a Chromium `encrypted_value` blob with `key`. Returns `None` (caller skips
/// + counts) on any structural or decode failure — never panics.
pub fn decrypt_chromium_value(blob: &[u8], key: &[u8; 16]) -> Option<String> {
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
        .decrypt_padded_mut::<Pkcs7>(&mut buf)
        .ok()?;
    String::from_utf8(pt.to_vec()).ok()
}

/// The Linux Chromium `v11` key: read password from Secret Service under `label`,
/// then PBKDF2-HMAC-SHA1(password, "saltysalt", 1, 16B).
///
/// Requires D-Bus + unlocked keyring at runtime. Returns `None` on any error
/// (missing D-Bus, locked keyring, item not found) so callers fall back to v10.
///
/// # Note
/// secret-service 4.x is async-only (zbus-based). Wiring a full async executor
/// here would impose a tokio dependency on the import crate; instead this is a
/// best-effort stub returning `None` with a TODO. Callers that want v11 support
/// should run the async path themselves using the `secret-service` crate directly.
#[cfg(feature = "keyring")]
pub fn chromium_v11_key(_label: &str) -> Option<[u8; 16]> {
    // TODO: secret-service 4.x is async-only (zbus). Wiring a blocking wrapper
    // requires either a dedicated tokio runtime or an async caller context.
    // Until the import engine has an async boundary, this returns None and the
    // caller falls back to chromium_v10_key(). Track in:
    // https://github.com/Lucenx9/forktty (follow-up after engine integration).
    None
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
    let conn = open_ro(db)?;
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
            match decrypt_chromium_value(&enc, key) {
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

#[cfg(test)]
mod tests {
    use super::*;

    // A v10 Chromium blob = b"v10" || AES-128-CBC(key, IV=16 spaces, PKCS7(plaintext)).
    fn make_v10_blob(key: &[u8; 16], plaintext: &[u8]) -> Vec<u8> {
        use aes::cipher::{block_padding::Pkcs7, BlockEncryptMut, KeyIvInit};
        type Enc = cbc::Encryptor<aes::Aes128>;
        let iv = [0x20u8; 16];
        let mut buf = plaintext.to_vec();
        let pad_len = 16 - (buf.len() % 16);
        buf.extend(std::iter::repeat_n(0u8, pad_len)); // room for padding
        let ct = Enc::new(key.into(), &iv.into())
            .encrypt_padded_mut::<Pkcs7>(&mut buf, plaintext.len())
            .unwrap()
            .to_vec();
        let mut out = b"v10".to_vec();
        out.extend_from_slice(&ct);
        out
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
