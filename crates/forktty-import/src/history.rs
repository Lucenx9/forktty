//! Read history and bookmarks from source browsers.
//! Firefox uses `places.sqlite`; Chromium family uses `History` (sqlite) + `Bookmarks` (JSON).

use std::{collections::BTreeMap, fmt, path::Path};

use rusqlite::{Connection, OpenFlags};
use serde::{
    de::{self, IgnoredAny, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer,
};

use crate::model::{ImportedBookmark, ImportedVisit};

const MAX_CHROMIUM_BOOKMARKS_BYTES: u64 = 16 * 1024 * 1024;
const MAX_CHROMIUM_BOOKMARK_NODES: usize = 100_000;

#[derive(Deserialize)]
struct ChromiumBookmarkFile {
    #[serde(default)]
    roots: BTreeMap<String, ChromiumBookmarkNode>,
}

#[derive(Deserialize)]
struct ChromiumBookmarkNode {
    #[serde(default, rename = "type")]
    node_type: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default, deserialize_with = "deserialize_chromium_bookmark_children")]
    children: Vec<ChromiumBookmarkNode>,
}

fn deserialize_chromium_bookmark_children<'de, D>(
    deserializer: D,
) -> Result<Vec<ChromiumBookmarkNode>, D::Error>
where
    D: Deserializer<'de>,
{
    deserializer.deserialize_any(ChromiumBookmarkChildrenVisitor)
}

struct ChromiumBookmarkChildrenVisitor;

impl<'de> Visitor<'de> for ChromiumBookmarkChildrenVisitor {
    type Value = Vec<ChromiumBookmarkNode>;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a Chromium bookmark children array")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut children = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(child) = seq.next_element()? {
            children.push(child);
        }
        Ok(children)
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Vec::new())
    }

    fn visit_bool<E>(self, _value: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Vec::new())
    }

    fn visit_i64<E>(self, _value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Vec::new())
    }

    fn visit_u64<E>(self, _value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Vec::new())
    }

    fn visit_f64<E>(self, _value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Vec::new())
    }

    fn visit_str<E>(self, _value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Vec::new())
    }

    fn visit_string<E>(self, _value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(Vec::new())
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        while map.next_entry::<IgnoredAny, IgnoredAny>()?.is_some() {}
        Ok(Vec::new())
    }
}

fn open_ro(path: &Path) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
}

/// Read Firefox visited URLs from `places.sqlite`.
/// Only `http`/`https` URLs are returned; `place:` internal entries are filtered out.
/// NULL title → empty string; NULL visit_count → 0.
pub fn read_firefox_history(places: &Path) -> rusqlite::Result<Vec<ImportedVisit>> {
    let conn = open_ro(places)?;
    let mut stmt = conn.prepare(
        "SELECT url, title, visit_count FROM moz_places
         WHERE url LIKE 'http://%' OR url LIKE 'https://%'",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ImportedVisit {
                url: row.get(0)?,
                title: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                visit_count: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Read Firefox bookmarks from `places.sqlite`.
/// Joins `moz_bookmarks` (type=1) to `moz_places` on `fk = moz_places.id`.
/// Bookmark title falls back to the place title. Only `http`/`https` URLs are returned.
pub fn read_firefox_bookmarks(places: &Path) -> rusqlite::Result<Vec<ImportedBookmark>> {
    let conn = open_ro(places)?;
    let mut stmt = conn.prepare(
        "SELECT p.url, COALESCE(b.title, p.title, '') AS title
         FROM moz_bookmarks b
         JOIN moz_places p ON b.fk = p.id
         WHERE b.type = 1 AND (p.url LIKE 'http://%' OR p.url LIKE 'https://%')",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ImportedBookmark {
                url: row.get(0)?,
                title: row.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Read Chromium-family visited URLs from the `History` sqlite database.
/// Only `http`/`https` URLs are returned.
pub fn read_chromium_history(history: &Path) -> rusqlite::Result<Vec<ImportedVisit>> {
    let conn = open_ro(history)?;
    let mut stmt = conn.prepare(
        "SELECT url, title, visit_count FROM urls
         WHERE url LIKE 'http://%' OR url LIKE 'https://%'",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(ImportedVisit {
                url: row.get(0)?,
                title: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                visit_count: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Read Chromium-family bookmarks from the `Bookmarks` JSON file.
///
/// Walks `roots.*.children`, collecting `{type:"url"}` nodes. Only `http`/`https`
/// URLs are returned.
///
/// Malformed, oversized, or missing JSON content returns `Ok(vec![])` — only
/// true IO errors propagate as `Err`.
pub fn read_chromium_bookmarks(path: &Path) -> std::io::Result<Vec<ImportedBookmark>> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(err) => return Err(err),
    };
    if !metadata.is_file() {
        return Ok(vec![]);
    }
    if metadata.len() > MAX_CHROMIUM_BOOKMARKS_BYTES {
        return Ok(vec![]);
    }
    let text = std::fs::read_to_string(path)?;
    let file: ChromiumBookmarkFile = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Ok(vec![]),
    };
    let mut out = Vec::new();
    // The node cap is a guard against a hostile/oversized Bookmarks file, so
    // it must bound the total walk across every root, not reset per root.
    let mut visited = 0usize;
    for root_node in file.roots.values() {
        collect_chromium_bookmarks(root_node, &mut out, &mut visited);
    }
    Ok(out)
}

fn collect_chromium_bookmarks(
    node: &ChromiumBookmarkNode,
    out: &mut Vec<ImportedBookmark>,
    visited: &mut usize,
) {
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        *visited += 1;
        if *visited > MAX_CHROMIUM_BOOKMARK_NODES {
            break;
        }
        if node.node_type.as_deref() == Some("url") {
            let url = node.url.as_deref().unwrap_or("");
            if url.starts_with("http://") || url.starts_with("https://") {
                let title = node.name.as_deref().unwrap_or("");
                out.push(ImportedBookmark {
                    url: url.to_string(),
                    title: title.to_string(),
                });
            }
        }
        for child in node.children.iter().rev() {
            stack.push(child);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn firefox_history_from_places() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("places.sqlite");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER);
             INSERT INTO moz_places (url,title,visit_count) VALUES ('https://a.test/','A',3);
             INSERT INTO moz_places (url,title,visit_count) VALUES ('place:internal','x',1);
             INSERT INTO moz_places (url,title,visit_count) VALUES ('httpx://bad.test/','bad',1);",
        )
        .unwrap();
        drop(conn);
        let visits = read_firefox_history(&db).unwrap();
        // Only http(s) URLs; place: internal entries dropped.
        assert_eq!(visits.len(), 1);
        assert_eq!(visits[0].url, "https://a.test/");
        assert_eq!(visits[0].visit_count, 3);
    }

    #[test]
    fn firefox_bookmarks_from_places() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("places.sqlite");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url TEXT, title TEXT);
             CREATE TABLE moz_bookmarks (id INTEGER PRIMARY KEY, fk INTEGER, title TEXT, type INTEGER);
             INSERT INTO moz_places (id,url,title) VALUES (1,'https://b.test/','B page');
             INSERT INTO moz_places (id,url,title) VALUES (2,'httpx://bad.test/','Bad page');
             INSERT INTO moz_bookmarks (fk,title,type) VALUES (1,'B mark',1);
             INSERT INTO moz_bookmarks (fk,title,type) VALUES (2,'Bad mark',1);",
        )
        .unwrap();
        drop(conn);
        let bms = read_firefox_bookmarks(&db).unwrap();
        assert_eq!(bms.len(), 1);
        assert_eq!(bms[0].url, "https://b.test/");
        assert_eq!(bms[0].title, "B mark");
    }

    #[test]
    fn chromium_history_from_urls_table() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("History");
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE urls (url TEXT, title TEXT, visit_count INTEGER);
             INSERT INTO urls VALUES ('https://c.test/','C',5);
             INSERT INTO urls VALUES ('httpx://bad.test/','bad',1);",
        )
        .unwrap();
        drop(conn);
        let visits = read_chromium_history(&db).unwrap();
        assert_eq!(visits.len(), 1);
        assert_eq!(visits[0].url, "https://c.test/");
        assert_eq!(visits[0].visit_count, 5);
    }

    #[test]
    fn chromium_bookmarks_from_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Bookmarks");
        fs::write(
            &path,
            r#"{"roots":{"bookmark_bar":{"children":[
                {"type":"url","name":"D","url":"https://d.test/"},
                {"type":"url","name":"Bad","url":"httpx://bad.test/"},
                {"type":"folder","name":"f","children":[
                    {"type":"url","name":"E","url":"https://e.test/"}]}]}}}"#,
        )
        .unwrap();
        let mut bms = read_chromium_bookmarks(&path).unwrap();
        bms.sort_by(|a, b| a.url.cmp(&b.url));
        assert_eq!(bms.len(), 2);
        assert_eq!(bms[0].url, "https://d.test/");
        assert_eq!(bms[1].url, "https://e.test/");
    }

    #[test]
    fn chromium_bookmarks_treat_null_children_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Bookmarks");
        fs::write(
            &path,
            r#"{"roots":{"bookmark_bar":{"children":[
                {"type":"folder","name":"bad-folder","children":null},
                {"type":"url","name":"Valid","url":"https://valid.test/"}]}}}"#,
        )
        .unwrap();

        let bms = read_chromium_bookmarks(&path).unwrap();

        assert_eq!(bms.len(), 1);
        assert_eq!(bms[0].url, "https://valid.test/");
    }

    #[test]
    fn chromium_bookmarks_treat_non_array_children_as_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Bookmarks");
        fs::write(
            &path,
            r#"{"roots":{"bookmark_bar":{"children":[
                {"type":"folder","name":"bad-folder","children":{"oops":"not-array"}},
                {"type":"url","name":"Valid","url":"https://valid.test/"}]}}}"#,
        )
        .unwrap();

        let bms = read_chromium_bookmarks(&path).unwrap();

        assert_eq!(bms.len(), 1);
        assert_eq!(bms[0].url, "https://valid.test/");
    }

    #[test]
    fn chromium_bookmarks_oversized_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Bookmarks");
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_CHROMIUM_BOOKMARKS_BYTES + 1).unwrap();
        let bms = read_chromium_bookmarks(&path).unwrap();
        assert!(bms.is_empty());
    }

    #[test]
    fn chromium_bookmarks_missing_file_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let bms = read_chromium_bookmarks(&dir.path().join("Bookmarks")).unwrap();
        assert!(bms.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn chromium_bookmarks_fifo_returns_empty_without_blocking() {
        use std::sync::mpsc;
        use std::time::Duration;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Bookmarks");
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&path)
                .status()
                .unwrap()
                .success(),
            "mkfifo failed"
        );

        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            sender.send(read_chromium_bookmarks(&path)).ok();
        });

        let result = receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("read_chromium_bookmarks must not block on a FIFO")
            .unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn chromium_bookmark_node_cap_is_global_across_roots() {
        // Two roots, each just over half the cap. With the cap reset per root a
        // hostile file would walk roots*cap nodes; the cap must bound the total.
        let half = MAX_CHROMIUM_BOOKMARK_NODES / 2 + 100;
        let node = r#"{"type":"url","name":"b","url":"https://x.test/"}"#;
        let children = vec![node; half].join(",");
        let root = format!(r#"{{"children":[{children}]}}"#);
        let json = format!(r#"{{"roots":{{"bookmark_bar":{root},"other":{root}}}}}"#);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Bookmarks");
        fs::write(&path, json).unwrap();

        let bms = read_chromium_bookmarks(&path).unwrap();

        assert!(
            bms.len() <= MAX_CHROMIUM_BOOKMARK_NODES,
            "node cap must be global across roots, got {}",
            bms.len()
        );
    }
}
