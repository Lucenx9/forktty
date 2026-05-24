//! Read history and bookmarks from source browsers.
//! Firefox uses `places.sqlite`; Chromium family uses `History` (sqlite) + `Bookmarks` (JSON).

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::model::{ImportedBookmark, ImportedVisit};

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
    let mut stmt =
        conn.prepare("SELECT url, title, visit_count FROM moz_places WHERE url LIKE 'http%'")?;
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
         WHERE b.type = 1 AND p.url LIKE 'http%'",
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
    let mut stmt =
        conn.prepare("SELECT url, title, visit_count FROM urls WHERE url LIKE 'http%'")?;
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
/// Recursively walks `roots.*.children`, collecting `{type:"url"}` nodes.
/// Folders are recursed. Only `http`/`https` URLs are returned.
///
/// Malformed or missing JSON content returns `Ok(vec![])` — only true IO errors
/// propagate as `Err`.
pub fn read_chromium_bookmarks(path: &Path) -> std::io::Result<Vec<ImportedBookmark>> {
    let text = std::fs::read_to_string(path)?;
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return Ok(vec![]),
    };
    let mut out = Vec::new();
    if let Some(roots) = value.get("roots").and_then(|r| r.as_object()) {
        for root_node in roots.values() {
            collect_chromium_bookmarks(root_node, &mut out);
        }
    }
    Ok(out)
}

fn collect_chromium_bookmarks(node: &serde_json::Value, out: &mut Vec<ImportedBookmark>) {
    match node.get("type").and_then(|t| t.as_str()) {
        Some("url") => {
            let url = node
                .get("url")
                .and_then(|u| u.as_str())
                .unwrap_or("")
                .to_string();
            if url.starts_with("http://") || url.starts_with("https://") {
                let title = node
                    .get("name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                out.push(ImportedBookmark { url, title });
            }
        }
        Some("folder") => {
            if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
                for child in children {
                    collect_chromium_bookmarks(child, out);
                }
            }
        }
        _ => {
            // Unknown node type or missing — check for children anyway (defensive).
            if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
                for child in children {
                    collect_chromium_bookmarks(child, out);
                }
            }
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
             INSERT INTO moz_places (url,title,visit_count) VALUES ('place:internal','x',1);",
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
             INSERT INTO moz_bookmarks (fk,title,type) VALUES (1,'B mark',1);",
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
             INSERT INTO urls VALUES ('https://c.test/','C',5);",
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
}
