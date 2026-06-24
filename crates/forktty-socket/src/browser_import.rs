use serde_json::{json, Value};
use std::fs;
use std::io::{self, Seek, SeekFrom};
use std::path::Path;

use crate::browser_import_params::{
    browser_import_source_id, BrowserImportPreviewRequest, BrowserImportRunRequest,
    BrowserImportSelection,
};
use crate::browser_profile::profiles_store;
use crate::{DispatchError, SocketAppState};

#[derive(Debug, Clone, Copy, Default)]
struct BrowserImportCounts {
    cookies: usize,
    history: usize,
    bookmarks: usize,
    skipped: usize,
}

impl BrowserImportCounts {
    fn add(&mut self, other: BrowserImportCounts) {
        self.cookies += other.cookies;
        self.history += other.history;
        self.bookmarks += other.bookmarks;
        self.skipped += other.skipped;
    }
}

fn browser_import_profile_json(profile: &forktty_import::SourceProfile) -> Value {
    json!({
        "id": browser_import_source_id(profile),
        "family": profile.family,
        "display_name": profile.display_name,
        "path": profile.path,
        "is_default": profile.is_default,
    })
}

pub(crate) fn browser_import_discover_json() -> Value {
    let browsers = forktty_import::discover();
    let profile_count: usize = browsers.iter().map(|browser| browser.profiles.len()).sum();
    let browsers_json: Vec<Value> = browsers
        .iter()
        .map(|browser| {
            let profiles: Vec<Value> = browser
                .profiles
                .iter()
                .map(browser_import_profile_json)
                .collect();
            json!({
                "family": browser.family,
                "label": browser.family.label(),
                "profiles": profiles,
            })
        })
        .collect();
    json!({
        "browsers": browsers_json,
        "count": profile_count,
    })
}

fn browser_import_counts_from_data(
    data: &forktty_import::ImportedData,
    include: BrowserImportSelection,
) -> BrowserImportCounts {
    BrowserImportCounts {
        cookies: if include.cookies {
            data.result.cookies
        } else {
            0
        },
        history: if include.history {
            data.visits.len()
        } else {
            0
        },
        bookmarks: if include.bookmarks {
            data.bookmarks.len()
        } else {
            0
        },
        skipped: if include.cookies {
            data.result.skipped
        } else {
            0
        },
    }
}

fn browser_import_counts_json(counts: BrowserImportCounts) -> Value {
    json!({
        "cookies": counts.cookies,
        "history": counts.history,
        "bookmarks": counts.bookmarks,
        "skipped": counts.skipped,
    })
}

pub(crate) async fn browser_import_preview(params: &Value) -> Result<Value, DispatchError> {
    let request = BrowserImportPreviewRequest::decode(params)?;
    let include = request.include;
    let mut total = BrowserImportCounts::default();
    let mut source_rows = Vec::new();

    for source in request.sources {
        let data = forktty_import::ImportEngine::read_source_async_with_selection(
            &source,
            include.read_selection(),
        )
        .await
        .map_err(|err| DispatchError::Other(err.to_string()))?;
        let counts = browser_import_counts_from_data(&data, include);
        total.add(counts);
        source_rows.push(json!({
            "source": browser_import_profile_json(&source),
            "counts": browser_import_counts_json(counts),
        }));
    }

    Ok(json!({
        "sources": source_rows,
        "total": browser_import_counts_json(total),
        "cookies_supported": false,
    }))
}

struct BrowserImportSpooledSource {
    source: forktty_import::SourceProfile,
    // Anonymous (unlinked) temp file: no directory entry survives crash/SIGKILL.
    data_file: std::fs::File,
}

pub(crate) fn browser_import_spool_data(
    mut data: forktty_import::ImportedData,
) -> Result<std::fs::File, DispatchError> {
    // Cookie import is reported as unsupported; keep only the counts needed for
    // the report so plaintext/decrypted cookie values never land in temp files.
    data.cookies.clear();
    let mut data_file =
        tempfile::tempfile().map_err(|err| DispatchError::Other(err.to_string()))?;
    serde_json::to_writer(&mut data_file, &data)
        .map_err(|err| DispatchError::Other(err.to_string()))?;
    Ok(data_file)
}

async fn browser_import_read_sources(
    sources: &[forktty_import::SourceProfile],
    include: BrowserImportSelection,
) -> Result<Vec<BrowserImportSpooledSource>, DispatchError> {
    let mut source_data = Vec::with_capacity(sources.len());
    for source in sources {
        let data = forktty_import::ImportEngine::read_source_async_with_selection(
            source,
            include.read_selection(),
        )
        .await
        .map_err(|err| DispatchError::Other(err.to_string()))?;
        let data_file = browser_import_spool_data(data)?;
        source_data.push(BrowserImportSpooledSource {
            source: source.clone(),
            data_file,
        });
    }
    Ok(source_data)
}

struct BrowserImportReadEntry {
    destination: forktty_import::ImportDestination,
    source_data: Vec<BrowserImportSpooledSource>,
}

async fn browser_import_read_plan(
    plan: forktty_import::ImportPlan,
    include: BrowserImportSelection,
) -> Result<(forktty_import::ImportMode, Vec<BrowserImportReadEntry>), DispatchError> {
    let mode = plan.mode;
    let mut entries = Vec::with_capacity(plan.entries.len());
    for entry in plan.entries {
        let source_data = browser_import_read_sources(&entry.sources, include).await?;
        entries.push(BrowserImportReadEntry {
            destination: entry.destination,
            source_data,
        });
    }
    Ok((mode, entries))
}

fn browser_import_profile_meta_json(
    id: forktty_core::ProfileId,
    display_name: &str,
    created: bool,
) -> Value {
    json!({
        "id": id.to_string(),
        "display_name": display_name,
        "created": created,
    })
}

fn browser_import_prepare_destination(
    store: &mut forktty_core::ProfileStore,
    destination: &forktty_import::ImportDestination,
) -> Result<(forktty_core::ProfileId, String, bool), DispatchError> {
    match destination {
        forktty_import::ImportDestination::Existing(id) => {
            let Some(meta) = store.list().iter().find(|profile| profile.id == *id) else {
                return Err(DispatchError::NotFound("profile".to_string()));
            };
            Ok((meta.id, meta.display_name.clone(), false))
        }
        forktty_import::ImportDestination::Create(display_name) => {
            let meta = store.create(display_name).map_err(|err| match err {
                forktty_core::ProfileError::InvalidInput(message) => {
                    DispatchError::InvalidParam(message)
                }
                other => DispatchError::Other(other.to_string()),
            })?;
            Ok((meta.id, meta.display_name, true))
        }
    }
}

fn rollback_browser_import_created_profile(
    state: &SocketAppState,
    profile_id: forktty_core::ProfileId,
) -> Result<(), String> {
    {
        let _profile_store_guard = state
            .profile_store_lock
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let mut store = profiles_store().map_err(|err| err.to_string())?;
        store.delete(&profile_id).map_err(|err| err.to_string())?;
    }

    if let Some(profile_dir) = forktty_core::browser_history::history_path(profile_id)
        .and_then(|path| path.parent().map(Path::to_path_buf))
    {
        match fs::remove_dir_all(&profile_dir) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(format!(
                    "profile data cleanup failed for {}: {err}",
                    profile_dir.display()
                ));
            }
        }
    }
    Ok(())
}

pub(crate) async fn browser_import_run(
    state: &SocketAppState,
    params: &Value,
) -> Result<Value, DispatchError> {
    let request = BrowserImportRunRequest::decode(params)?;
    let plan = {
        let _profile_store_guard = state
            .profile_store_lock
            .lock()
            .map_err(|_| "Lock poisoned".to_string())?;
        let store = profiles_store()?;
        request.plan(params, &store)?
    };
    let include = request.include;
    let (mode, read_entries) = browser_import_read_plan(plan, include).await?;

    let mut total_read = BrowserImportCounts::default();
    let mut total_written = BrowserImportCounts::default();
    let mut total_unsupported_cookies = 0usize;
    let mut entries_json = Vec::new();

    for entry in read_entries {
        let (profile_id, display_name, created) = {
            let _profile_store_guard = state
                .profile_store_lock
                .lock()
                .map_err(|_| "Lock poisoned".to_string())?;
            let mut store = profiles_store()?;
            browser_import_prepare_destination(&mut store, &entry.destination)?
        };
        let entry_result: Result<
            (Value, BrowserImportCounts, BrowserImportCounts, usize),
            DispatchError,
        > = async {
            let history_store = if include.history {
                Some(
                    forktty_core::HistoryStore::for_profile(profile_id)
                        .map_err(|err| DispatchError::Other(err.to_string()))?,
                )
            } else {
                None
            };
            let mut bookmark_store = if include.bookmarks {
                Some(
                    forktty_core::BookmarkStore::for_profile(profile_id)
                        .map_err(|err| DispatchError::Other(err.to_string()))?,
                )
            } else {
                None
            };

            let mut entry_read = BrowserImportCounts::default();
            let mut entry_written = BrowserImportCounts::default();
            let mut entry_unsupported_cookies = 0usize;
            let mut entry_sources = Vec::new();

            for mut spooled in entry.source_data {
                spooled
                    .data_file
                    .seek(SeekFrom::Start(0))
                    .map_err(|err| DispatchError::Other(err.to_string()))?;
                let data: forktty_import::ImportedData =
                    serde_json::from_reader(&mut spooled.data_file)
                        .map_err(|err| DispatchError::Other(err.to_string()))?;
                let read_counts = browser_import_counts_from_data(&data, include);
                entry_read.add(read_counts);
                entry_sources.push(browser_import_profile_json(&spooled.source));

                if let Some(history_store) = &history_store {
                    for visit in &data.visits {
                        if history_store
                            .import_visit(&visit.url, &visit.title, visit.visit_count)
                            .map_err(|err| DispatchError::Other(err.to_string()))?
                        {
                            entry_written.history += 1;
                        }
                    }
                }

                if let Some(bookmark_store) = bookmark_store.as_mut() {
                    for bookmark in &data.bookmarks {
                        bookmark_store
                            .add(&bookmark.url, &bookmark.title)
                            .map_err(|err| DispatchError::Other(err.to_string()))?;
                        entry_written.bookmarks += 1;
                    }
                }

                if include.cookies {
                    entry_unsupported_cookies += data.result.cookies;
                }
            }

            let entry_json = json!({
                "destination": browser_import_profile_meta_json(profile_id, &display_name, created),
                "sources": entry_sources,
                "read": browser_import_counts_json(entry_read),
                "written": browser_import_counts_json(entry_written),
                "cookies": {
                    "read": entry_read.cookies,
                    "written": 0,
                    "unsupported": entry_unsupported_cookies,
                    "skipped": entry_read.skipped,
                },
            });
            Ok((
                entry_json,
                entry_read,
                entry_written,
                entry_unsupported_cookies,
            ))
        }
        .await;

        let (entry_json, entry_read, entry_written, entry_unsupported_cookies) = match entry_result
        {
            Ok(result) => result,
            Err(err) => {
                if created {
                    if let Err(cleanup_err) =
                        rollback_browser_import_created_profile(state, profile_id)
                    {
                        return Err(DispatchError::Other(format!(
                            "{err}; created profile cleanup failed: {cleanup_err}"
                        )));
                    }
                }
                return Err(err);
            }
        };

        total_read.add(entry_read);
        total_written.add(entry_written);
        total_unsupported_cookies += entry_unsupported_cookies;
        entries_json.push(entry_json);
    }

    Ok(json!({
        "mode": mode,
        "entries": entries_json,
        "total": {
            "read": browser_import_counts_json(total_read),
            "written": browser_import_counts_json(total_written),
            "cookies": {
                "read": total_read.cookies,
                "written": 0,
                "unsupported": total_unsupported_cookies,
                "skipped": total_read.skipped,
            },
        },
        "cookies_supported": false,
    }))
}
