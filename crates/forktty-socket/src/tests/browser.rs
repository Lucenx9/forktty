//! Browser socket method regression tests.

use super::*;

#[tokio::test]
async fn browser_open_waits_for_surface_set_guard_before_model_commit() {
    let (state, _backend) = test_state();
    let workspace_id = state.model.lock().unwrap().active_workspace().unwrap().id;
    let guard = state.surface_set_guard().await;
    let open_state = state.clone();
    let mut open = tokio::spawn(async move {
        dispatch(
            &open_state,
            "browser.open",
            json!({"workspace_id": workspace_id, "url": "https://example.com"}),
        )
        .await
    });

    assert!(
        tokio::time::timeout(Duration::from_millis(100), &mut open)
            .await
            .is_err(),
        "browser topology creation must wait for the surface transaction"
    );
    assert_eq!(state.model.lock().unwrap().list_surfaces(None).len(), 1);

    drop(guard);
    open.await.unwrap().unwrap();
    assert_eq!(state.model.lock().unwrap().list_surfaces(None).len(), 2);
}

#[tokio::test]
async fn browser_open_creates_browser_surface_and_navigate_updates_url() {
    let (state, _backend) = test_state();
    let created = dispatch(&state, "workspace.create", json!({"name": "w"}))
        .await
        .unwrap();
    let ws_id = created["id"].as_str().unwrap().to_string();

    let opened = dispatch(
        &state,
        "browser.open",
        json!({"workspace_id": ws_id, "url": "example.com"}),
    )
    .await
    .unwrap();
    let surface_id = opened["id"].as_str().unwrap().to_string();
    // Bare domain gets https:// prepended. Kind now carries the profile id too.
    assert_eq!(opened["kind"]["type"], json!("browser"));
    assert_eq!(opened["kind"]["url"], json!("https://example.com"));

    let navigated = dispatch(
        &state,
        "browser.navigate",
        json!({"surface_id": surface_id, "url": "https://other.com"}),
    )
    .await
    .unwrap();
    assert_eq!(navigated["navigated"], json!(true));

    let same_url = dispatch(
        &state,
        "browser.navigate",
        json!({"surface_id": surface_id, "url": "https://other.com"}),
    )
    .await
    .unwrap();
    assert_eq!(same_url["navigated"], json!(true));

    // navigate on a non-browser surface errors.
    let term = created["focused_surface_id"].as_str().unwrap();
    let err = dispatch(
        &state,
        "browser.navigate",
        json!({"surface_id": term, "url": "https://x.com"}),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, DispatchError::NotFound(_)));
}

#[tokio::test]
async fn browser_url_limit_applies_after_default_scheme() {
    let (state, _backend) = test_state();
    let created = dispatch(&state, "workspace.create", json!({"name": "w"}))
        .await
        .unwrap();
    let ws_id = created["id"].as_str().unwrap();
    let bare_url = "a".repeat(MAX_BROWSER_URL_BYTES - "https://".len() + 1);

    let err = dispatch(
        &state,
        "browser.open",
        json!({"workspace_id": ws_id, "url": bare_url}),
    )
    .await
    .unwrap_err();

    assert_eq!(err.code(), "payload_too_large");
}

// --- SP2 browser scripting verbs ---------------------------------------

fn state_with_browser_channel() -> (
    SocketAppState,
    async_channel::Receiver<forktty_core::BrowserCommand>,
) {
    let (state, _backend) = test_state();
    let (tx, rx) = async_channel::unbounded();
    (state.with_browser_cmd(tx), rx)
}

async fn open_browser_surface(state: &SocketAppState) -> String {
    let ws = dispatch(state, "workspace.create", json!({"name": "w"}))
        .await
        .unwrap();
    let workspace_id = ws.get("id").unwrap().as_str().unwrap().to_string();
    let surface = dispatch(
        state,
        "browser.open",
        json!({"workspace_id": workspace_id, "url": "https://example.com"}),
    )
    .await
    .unwrap();
    surface.get("id").unwrap().as_str().unwrap().to_string()
}

#[tokio::test]
async fn browser_snapshot_unavailable_without_channel() {
    let (state, _backend) = test_state();
    let sid = open_browser_surface(&state).await;
    let err = dispatch(&state, "browser.snapshot", json!({"surface_id": sid}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("browser automation unavailable"));
}

#[tokio::test]
async fn browser_snapshot_returns_stub_json() {
    let (state, rx) = state_with_browser_channel();
    let sid = open_browser_surface(&state).await;
    let responder = tokio::spawn(async move {
        let cmd = rx.recv().await.unwrap();
        assert_eq!(cmd.op, forktty_core::BrowserOp::Snapshot);
        cmd.reply
            .send(forktty_core::CmdResult::Json("{\"role\":\"root\"}".into()))
            .unwrap();
    });
    let result = dispatch(&state, "browser.snapshot", json!({"surface_id": sid}))
        .await
        .unwrap();
    assert_eq!(result, json!({"role": "root"}));
    responder.await.unwrap();
}

#[tokio::test]
async fn browser_back_returns_ok() {
    let (state, rx) = state_with_browser_channel();
    let sid = open_browser_surface(&state).await;
    let responder = tokio::spawn(async move {
        let cmd = rx.recv().await.unwrap();
        assert_eq!(cmd.op, forktty_core::BrowserOp::Back);
        cmd.reply.send(forktty_core::CmdResult::Ok).unwrap();
    });
    let result = dispatch(&state, "browser.back", json!({"surface_id": sid}))
        .await
        .unwrap();
    assert_eq!(result, json!({"ok": true}));
    responder.await.unwrap();
}

#[tokio::test]
async fn browser_eval_is_not_exposed() {
    let (state, _rx) = state_with_browser_channel();
    let sid = open_browser_surface(&state).await;
    let err = dispatch(
        &state,
        "browser.eval",
        json!({"surface_id": sid, "script": "document.title"}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "method_not_found");
}

#[tokio::test]
async fn browser_click_on_terminal_surface_is_not_found() {
    let (state, _rx) = state_with_browser_channel();
    let ws = dispatch(&state, "workspace.create", json!({"name": "w"}))
        .await
        .unwrap();
    let workspace_id = ws.get("id").unwrap().as_str().unwrap();
    let surfaces = dispatch(
        &state,
        "surface.list",
        json!({"workspace_id": workspace_id}),
    )
    .await
    .unwrap();
    let term_id = surfaces.as_array().unwrap()[0]
        .get("id")
        .unwrap()
        .as_str()
        .unwrap()
        .to_string();
    let err = dispatch(
        &state,
        "browser.click",
        json!({"surface_id": term_id, "ref": "e1"}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "not_found");
}

#[tokio::test]
async fn browser_click_maps_ref_not_found_reply() {
    let (state, rx) = state_with_browser_channel();
    let sid = open_browser_surface(&state).await;
    let responder = tokio::spawn(async move {
        let cmd = rx.recv().await.unwrap();
        cmd.reply
            .send(forktty_core::CmdResult::Err(
                forktty_core::BrowserCmdError::RefNotFound,
            ))
            .unwrap();
    });
    let err = dispatch(
        &state,
        "browser.click",
        json!({"surface_id": sid, "ref": "e1"}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "not_found");
    responder.await.unwrap();
}

#[tokio::test]
async fn browser_fill_maps_not_interactable_reply() {
    let (state, rx) = state_with_browser_channel();
    let sid = open_browser_surface(&state).await;
    let responder = tokio::spawn(async move {
        let cmd = rx.recv().await.unwrap();
        assert_eq!(
            cmd.op,
            forktty_core::BrowserOp::Fill {
                reference: "e1".to_string(),
                value: "hello".to_string(),
            }
        );
        cmd.reply
            .send(forktty_core::CmdResult::Err(
                forktty_core::BrowserCmdError::ElementNotInteractable,
            ))
            .unwrap();
    });
    let err = dispatch(
        &state,
        "browser.fill",
        json!({"surface_id": sid, "ref": "e1", "value": "hello"}),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "invalid_param");
    assert_eq!(err.to_string(), "element is not interactable");
    responder.await.unwrap();
}

// --- SP3 P2 browser.profile verbs ----------------------------------------

#[tokio::test]
#[serial_test::serial]
async fn browser_profile_create_list_then_open_with_profile() {
    // Isolate profiles.json from the real user data dir.
    // XDG_DATA_HOME is process-global; serialize with capabilities test via
    // #[serial_test::serial] and restore on any exit path via EnvGuard.
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvGuard::set("XDG_DATA_HOME", dir.path().to_str().unwrap());

    let (state, _backend) = test_state();

    // Create a workspace so we have a workspace_id for browser.open.
    let ws = dispatch(&state, "workspace.create", json!({"name": "w"}))
        .await
        .unwrap();
    let workspace_id = ws.get("id").unwrap().as_str().unwrap().to_string();

    // browser.profile.create
    let created = dispatch(
        &state,
        "browser.profile.create",
        json!({ "display_name": "Work" }),
    )
    .await
    .unwrap();
    let new_id = created
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap()
        .to_string();
    assert_eq!(created["display_name"], json!("Work"));

    // browser.profile.list — should have Default (is_default=true) + Work
    let listed = dispatch(&state, "browser.profile.list", json!({}))
        .await
        .unwrap();
    let arr = listed.as_array().unwrap();
    assert!(
        arr.iter().any(|p| p["is_default"] == json!(true)),
        "list must contain the default profile"
    );
    assert!(
        arr.iter().any(|p| p["display_name"] == json!("Work")),
        "list must contain Work profile"
    );

    // browser.open with profile name — resolves "Work" to its id
    let opened = dispatch(
        &state,
        "browser.open",
        json!({
            "workspace_id": workspace_id,
            "url": "https://example.com",
            "profile": "Work"
        }),
    )
    .await
    .unwrap();
    assert!(opened.get("id").is_some(), "opened surface must have an id");
    // Surface kind should be a browser
    assert_eq!(opened["kind"]["type"], json!("browser"));

    // browser.profile.delete while a pane is open in that profile must be refused
    let del_err = dispatch(&state, "browser.profile.delete", json!({ "id": new_id }))
        .await
        .unwrap_err();
    assert!(
        del_err.to_string().contains("in use"),
        "expected in-use error, got: {del_err}"
    );

    // _env (EnvGuard) and dir (TempDir) are dropped here, restoring the
    // environment and removing temporary files on any exit path.
}

#[tokio::test]
#[serial_test::serial]
async fn browser_open_rejects_non_string_profile_param() {
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvGuard::set("XDG_DATA_HOME", dir.path().to_str().unwrap());

    let (state, _backend) = test_state();
    let ws = dispatch(&state, "workspace.create", json!({"name": "w"}))
        .await
        .unwrap();
    let workspace_id = ws.get("id").unwrap().as_str().unwrap().to_string();

    let err = dispatch(
        &state,
        "browser.open",
        json!({
            "workspace_id": workspace_id,
            "url": "https://example.com",
            "profile": 123
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "invalid_param");
    assert_eq!(
        err.to_string(),
        "Invalid parameter profile: expected string"
    );

    let surfaces = dispatch(
        &state,
        "surface.list",
        json!({"workspace_id": ws.get("id").unwrap().as_str().unwrap()}),
    )
    .await
    .unwrap();
    assert_eq!(surfaces.as_array().unwrap().len(), 1);
}

#[test]
#[serial_test::serial]
fn browser_profile_create_serializes_store_writes() {
    let dir = tempfile::tempdir().unwrap();
    let _env = EnvGuard::set("XDG_DATA_HOME", dir.path().to_str().unwrap());

    let (state, _backend) = test_state();
    let task_count = 24;
    let barrier = Arc::new(Barrier::new(task_count));
    let mut handles = Vec::new();
    for index in 0..task_count {
        let state = state.clone();
        let barrier = barrier.clone();
        handles.push(std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            barrier.wait();
            runtime
                .block_on(dispatch(
                    &state,
                    "browser.profile.create",
                    json!({ "display_name": format!("Profile {index}") }),
                ))
                .unwrap();
        }));
    }

    for handle in handles {
        handle.join().unwrap();
    }

    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let listed = runtime
        .block_on(dispatch(&state, "browser.profile.list", json!({})))
        .unwrap();
    let profiles = listed.as_array().unwrap();
    assert_eq!(profiles.len(), task_count + 1);
    for index in 0..task_count {
        assert!(
            profiles
                .iter()
                .any(|profile| profile["display_name"] == json!(format!("Profile {index}"))),
            "missing Profile {index}"
        );
    }
}

// --- SP3 P3 browser.history + browser.bookmark verbs ---------------------

#[test]
fn browser_history_limit_defaults_and_caps() {
    assert_eq!(
        browser_profile::history_limit_from_params(&json!({})).unwrap(),
        100
    );
    assert_eq!(
        browser_profile::history_limit_from_params(&json!({"limit": null})).unwrap(),
        100
    );
    assert_eq!(
        browser_profile::history_limit_from_params(&json!({"limit": 5})).unwrap(),
        5
    );
    assert_eq!(
        browser_profile::history_limit_from_params(&json!({"limit": u64::MAX})).unwrap(),
        10_000
    );
    assert!(matches!(
        browser_profile::history_limit_from_params(&json!({"limit": "5"})),
        Err(DispatchError::InvalidParam(_))
    ));
}

#[tokio::test]
#[serial_test::serial]
async fn browser_history_list_and_clear() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvGuard::set("XDG_DATA_HOME", tmp.path().to_str().unwrap());
    let (state, _backend) = test_state();

    // history list on a fresh profile is empty
    let hist = dispatch(&state, "browser.history.list", json!({}))
        .await
        .unwrap();
    assert!(
        hist.as_array().unwrap().is_empty(),
        "fresh history must be empty"
    );

    // clear is a no-op success on empty history
    let cleared = dispatch(&state, "browser.history.clear", json!({}))
        .await
        .unwrap();
    assert_eq!(cleared["cleared"], json!(true));
}

#[tokio::test]
#[serial_test::serial]
async fn browser_bookmark_add_list_remove_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvGuard::set("XDG_DATA_HOME", tmp.path().to_str().unwrap());
    let (state, _backend) = test_state();

    // add
    let added = dispatch(
        &state,
        "browser.bookmark.add",
        json!({"url": "https://a.test/", "title": "A"}),
    )
    .await
    .unwrap();
    assert_eq!(added["added"], json!(true));

    // list
    let listed = dispatch(&state, "browser.bookmark.list", json!({}))
        .await
        .unwrap();
    let arr = listed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["url"], json!("https://a.test/"));
    assert_eq!(arr[0]["title"], json!("A"));

    // remove
    let removed = dispatch(
        &state,
        "browser.bookmark.remove",
        json!({"url": "https://a.test/"}),
    )
    .await
    .unwrap();
    assert_eq!(removed["removed"], json!(true));

    // list is now empty
    let listed2 = dispatch(&state, "browser.bookmark.list", json!({}))
        .await
        .unwrap();
    assert!(listed2.as_array().unwrap().is_empty());
}

#[test]
fn browser_import_spool_data_strips_cookie_values_but_keeps_counts() {
    let data = forktty_import::ImportedData {
        cookies: vec![forktty_import::ImportedCookie {
            name: "sid".to_string(),
            value: "secret-cookie-value".to_string(),
            host: ".example.test".to_string(),
            path: "/".to_string(),
            expires: None,
            secure: false,
            http_only: true,
        }],
        visits: vec![forktty_import::ImportedVisit {
            url: "https://example.test/".to_string(),
            title: "Example".to_string(),
            visit_count: 2,
        }],
        bookmarks: vec![forktty_import::ImportedBookmark {
            url: "https://example.test/".to_string(),
            title: "Example Bookmark".to_string(),
        }],
        result: forktty_import::ImportResult {
            cookies: 1,
            history: 1,
            bookmarks: 1,
            skipped: 0,
        },
    };

    let mut data_file = browser_import_spool_data(data).unwrap();
    data_file.seek(SeekFrom::Start(0)).unwrap();
    let mut serialized = String::new();
    std::io::Read::read_to_string(&mut data_file, &mut serialized).unwrap();
    assert!(!serialized.contains("secret-cookie-value"));
    assert!(serialized.contains("https://example.test/"));

    data_file.seek(SeekFrom::Start(0)).unwrap();
    let spooled: forktty_import::ImportedData = serde_json::from_reader(&mut data_file).unwrap();
    assert!(spooled.cookies.is_empty());
    assert_eq!(spooled.result.cookies, 1);
    assert_eq!(spooled.visits.len(), 1);
    assert_eq!(spooled.bookmarks.len(), 1);
}

fn create_firefox_import_source(home: &Path, name: &str) -> forktty_import::SourceProfile {
    let profile_dir = home.join(".mozilla/firefox").join(name);
    fs::create_dir_all(&profile_dir).unwrap();

    let cookies = rusqlite::Connection::open(profile_dir.join("cookies.sqlite")).unwrap();
    cookies
        .execute_batch(
            "CREATE TABLE moz_cookies (
            name TEXT, value TEXT, host TEXT, path TEXT,
            expiry INTEGER, isSecure INTEGER, isHttpOnly INTEGER
         );
         INSERT INTO moz_cookies VALUES ('sid','cookie-value','.example.test','/',0,0,1);",
        )
        .unwrap();
    drop(cookies);

    let places = rusqlite::Connection::open(profile_dir.join("places.sqlite")).unwrap();
    places
        .execute_batch(
            "CREATE TABLE moz_places (
            id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER
         );
         CREATE TABLE moz_bookmarks (
            id INTEGER PRIMARY KEY, fk INTEGER, title TEXT, type INTEGER
         );
         INSERT INTO moz_places (id,url,title,visit_count)
            VALUES (1,'https://example.test/','Example',2);
         INSERT INTO moz_bookmarks (fk,title,type)
            VALUES (1,'Example Bookmark',1);",
        )
        .unwrap();
    drop(places);

    let profile = forktty_import::SourceProfile {
        family: forktty_import::BrowserFamily::Firefox,
        display_name: name.to_string(),
        path: profile_dir.to_string_lossy().into_owned(),
        is_default: false,
    };
    let profiles_ini = format!("[Profile0]\nName={name}\nPath={name}\nDefault=1\n");
    fs::write(home.join(".mozilla/firefox/profiles.ini"), profiles_ini).unwrap();
    profile
}

fn create_corrupt_firefox_import_source(home: &Path, name: &str) -> forktty_import::SourceProfile {
    let profile_dir = home.join(".mozilla/firefox").join(name);
    fs::create_dir_all(&profile_dir).unwrap();
    let cookies = rusqlite::Connection::open(profile_dir.join("cookies.sqlite")).unwrap();
    cookies
        .execute_batch(
            "CREATE TABLE moz_cookies (
            name TEXT, value TEXT, host TEXT, path TEXT,
            expiry INTEGER, isSecure INTEGER, isHttpOnly INTEGER
         );",
        )
        .unwrap();
    drop(cookies);
    fs::write(profile_dir.join("places.sqlite"), b"not a sqlite database").unwrap();
    let profiles_ini = format!("[Profile0]\nName={name}\nPath={name}\nDefault=1\n");
    fs::write(home.join(".mozilla/firefox/profiles.ini"), profiles_ini).unwrap();
    forktty_import::SourceProfile {
        family: forktty_import::BrowserFamily::Firefox,
        display_name: name.to_string(),
        path: profile_dir.to_string_lossy().into_owned(),
        is_default: true,
    }
}

fn create_firefox_import_source_with_corrupt_cookies(
    home: &Path,
    name: &str,
) -> forktty_import::SourceProfile {
    let profile_dir = home.join(".mozilla/firefox").join(name);
    fs::create_dir_all(&profile_dir).unwrap();
    fs::write(profile_dir.join("cookies.sqlite"), b"not a sqlite database").unwrap();
    let places = rusqlite::Connection::open(profile_dir.join("places.sqlite")).unwrap();
    places
        .execute_batch(
            "CREATE TABLE moz_places (
            id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER
         );
         INSERT INTO moz_places (id,url,title,visit_count)
            VALUES (1,'https://history-only.test/','History Only',3);",
        )
        .unwrap();
    drop(places);
    let profiles_ini = format!("[Profile0]\nName={name}\nPath={name}\nDefault=1\n");
    fs::write(home.join(".mozilla/firefox/profiles.ini"), profiles_ini).unwrap();
    forktty_import::SourceProfile {
        family: forktty_import::BrowserFamily::Firefox,
        display_name: name.to_string(),
        path: profile_dir.to_string_lossy().into_owned(),
        is_default: true,
    }
}

fn create_firefox_import_source_with_long_history_url(
    home: &Path,
    name: &str,
) -> forktty_import::SourceProfile {
    let profile_dir = home.join(".mozilla/firefox").join(name);
    fs::create_dir_all(&profile_dir).unwrap();
    let long_url = format!("https://{}.test/", "a".repeat(MAX_BROWSER_URL_BYTES + 1));
    let places = rusqlite::Connection::open(profile_dir.join("places.sqlite")).unwrap();
    places
        .execute(
            "CREATE TABLE moz_places (
                id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER
            );",
            [],
        )
        .unwrap();
    places
        .execute(
            "INSERT INTO moz_places (id,url,title,visit_count)
                VALUES (1,?1,'Too Long',4);",
            [&long_url],
        )
        .unwrap();
    drop(places);
    write_firefox_profiles_ini(home, &[name]);
    forktty_import::SourceProfile {
        family: forktty_import::BrowserFamily::Firefox,
        display_name: name.to_string(),
        path: profile_dir.to_string_lossy().into_owned(),
        is_default: true,
    }
}

fn write_firefox_profiles_ini(home: &Path, names: &[&str]) {
    let mut profiles_ini = String::new();
    for (index, name) in names.iter().enumerate() {
        profiles_ini.push_str(&format!(
            "[Profile{index}]\nName={name}\nPath={name}\nDefault={}\n",
            usize::from(index == 0)
        ));
    }
    fs::write(home.join(".mozilla/firefox/profiles.ini"), profiles_ini).unwrap();
}

#[tokio::test]
#[serial_test::serial]
async fn browser_import_discover_preview_and_run_imports_history_bookmarks() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let _home = EnvGuard::set("HOME", home.to_str().unwrap());
    let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
    let source = create_firefox_import_source(&home, "default-release");
    let source_id = browser_import_source_id(&source);
    let (state, _backend) = test_state();

    let discovered = dispatch(&state, "browser.import.discover", json!({}))
        .await
        .unwrap();
    assert_eq!(discovered["count"], json!(1));
    assert_eq!(
        discovered["browsers"][0]["profiles"][0]["id"],
        json!(source_id)
    );

    let preview = dispatch(
        &state,
        "browser.import.preview",
        json!({"sources": [source_id.clone()]}),
    )
    .await
    .unwrap();
    assert_eq!(preview["total"]["history"], json!(1));
    assert_eq!(preview["total"]["bookmarks"], json!(1));
    assert_eq!(preview["total"]["cookies"], json!(1));

    let imported = dispatch(
        &state,
        "browser.import.run",
        json!({
            "sources": [source_id.clone()],
            "destination": {"kind": "existing", "profile": "Default"}
        }),
    )
    .await
    .unwrap();
    assert_eq!(imported["total"]["written"]["history"], json!(1));
    assert_eq!(imported["total"]["written"]["bookmarks"], json!(1));
    assert_eq!(imported["total"]["cookies"]["written"], json!(0));
    assert_eq!(imported["total"]["cookies"]["unsupported"], json!(1));

    let history = dispatch(
        &state,
        "browser.history.search",
        json!({"query": "example.test", "limit": 10}),
    )
    .await
    .unwrap();
    assert_eq!(history.as_array().unwrap().len(), 1);
    assert_eq!(history[0]["visit_count"], json!(2));
    let bookmarks = dispatch(&state, "browser.bookmark.list", json!({}))
        .await
        .unwrap();
    assert_eq!(bookmarks.as_array().unwrap().len(), 1);

    let imported_again = dispatch(
        &state,
        "browser.import.run",
        json!({
            "sources": [source_id],
            "destination": {"kind": "existing", "profile": "Default"}
        }),
    )
    .await
    .unwrap();
    assert_eq!(imported_again["total"]["written"]["history"], json!(1));
    let history_after = dispatch(
        &state,
        "browser.history.search",
        json!({"query": "example.test", "limit": 10}),
    )
    .await
    .unwrap();
    assert_eq!(history_after.as_array().unwrap().len(), 1);
    assert_eq!(history_after[0]["visit_count"], json!(2));
    let bookmarks_after = dispatch(&state, "browser.bookmark.list", json!({}))
        .await
        .unwrap();
    assert_eq!(bookmarks_after.as_array().unwrap().len(), 1);
}

#[tokio::test]
#[serial_test::serial]
async fn browser_import_reports_skipped_oversized_history_urls() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let _home = EnvGuard::set("HOME", home.to_str().unwrap());
    let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
    let source = create_firefox_import_source_with_long_history_url(&home, "long-url");
    let source_id = browser_import_source_id(&source);
    let (state, _backend) = test_state();

    let imported = dispatch(
        &state,
        "browser.import.run",
        json!({
            "sources": [source_id],
            "destination": {"kind": "existing", "profile": "Default"}
        }),
    )
    .await
    .unwrap();

    assert_eq!(imported["total"]["read"]["history"], json!(1));
    assert_eq!(imported["total"]["written"]["history"], json!(0));
    assert_eq!(imported["entries"][0]["written"]["history"], json!(0));
    let history = dispatch(&state, "browser.history.list", json!({}))
        .await
        .unwrap();
    assert!(history.as_array().unwrap().is_empty());
}

#[tokio::test]
#[serial_test::serial]
async fn browser_import_run_creates_new_profile_from_plan() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let _home = EnvGuard::set("HOME", home.to_str().unwrap());
    let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
    let source = create_firefox_import_source(&home, "work");
    let source_id = browser_import_source_id(&source);
    let (state, _backend) = test_state();

    let imported = dispatch(
        &state,
        "browser.import.run",
        json!({
            "sources": [source_id],
            "destination": {"kind": "create", "display_name": "Imported Work"}
        }),
    )
    .await
    .unwrap();
    assert_eq!(
        imported["entries"][0]["destination"]["created"],
        json!(true)
    );
    let profile_id = imported["entries"][0]["destination"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let profiles = dispatch(&state, "browser.profile.list", json!({}))
        .await
        .unwrap();
    assert!(profiles.as_array().unwrap().iter().any(|profile| {
        profile["id"] == json!(profile_id) && profile["display_name"] == json!("Imported Work")
    }));
}

#[tokio::test]
#[serial_test::serial]
async fn browser_import_skips_unselected_corrupt_cookie_db() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let _home = EnvGuard::set("HOME", home.to_str().unwrap());
    let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
    let source = create_firefox_import_source_with_corrupt_cookies(&home, "history-only");
    let source_id = browser_import_source_id(&source);
    let (state, _backend) = test_state();

    let preview = dispatch(
        &state,
        "browser.import.preview",
        json!({
            "sources": [source_id.clone()],
            "include": {"history": true, "bookmarks": false, "cookies": false}
        }),
    )
    .await
    .unwrap();
    assert_eq!(preview["total"]["history"], json!(1));
    assert_eq!(preview["total"]["cookies"], json!(0));

    let imported = dispatch(
        &state,
        "browser.import.run",
        json!({
            "sources": [source_id],
            "include": {"history": true, "bookmarks": false, "cookies": false},
            "destination": {"kind": "existing", "profile": "Default"}
        }),
    )
    .await
    .unwrap();
    assert_eq!(imported["total"]["written"]["history"], json!(1));
    assert_eq!(imported["total"]["cookies"]["read"], json!(0));
}

#[tokio::test]
#[serial_test::serial]
async fn browser_import_missing_and_corrupt_sources_are_errors_not_crashes() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let _home = EnvGuard::set("HOME", home.to_str().unwrap());
    let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
    let (state, _backend) = test_state();

    let missing = dispatch(
        &state,
        "browser.import.preview",
        json!({"sources": ["firefox:/does/not/exist"]}),
    )
    .await
    .unwrap_err();
    assert_eq!(missing.code(), "not_found");

    let source = create_corrupt_firefox_import_source(&home, "corrupt");
    let corrupt = dispatch(
        &state,
        "browser.import.preview",
        json!({"sources": [browser_import_source_id(&source)]}),
    )
    .await
    .unwrap_err();
    assert_eq!(corrupt.code(), "error");
    assert!(corrupt.to_string().contains("import database error"));
}

#[tokio::test]
#[serial_test::serial]
async fn browser_import_run_does_not_create_profile_for_unreadable_source() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let _home = EnvGuard::set("HOME", home.to_str().unwrap());
    let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
    let source = create_corrupt_firefox_import_source(&home, "corrupt-create");
    let source_id = browser_import_source_id(&source);
    let (state, _backend) = test_state();

    let err = dispatch(
        &state,
        "browser.import.run",
        json!({
            "sources": [source_id],
            "destination": {"kind": "create", "display_name": "Should Roll Back"}
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "error");

    let profiles = dispatch(&state, "browser.profile.list", json!({}))
        .await
        .unwrap();
    assert!(!profiles
        .as_array()
        .unwrap()
        .iter()
        .any(|profile| { profile["display_name"] == json!("Should Roll Back") }));
}

#[tokio::test]
#[serial_test::serial]
async fn browser_import_run_does_not_partially_write_existing_profile_on_later_read_error() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let _home = EnvGuard::set("HOME", home.to_str().unwrap());
    let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
    let valid = create_firefox_import_source(&home, "valid");
    let corrupt = create_corrupt_firefox_import_source(&home, "corrupt");
    write_firefox_profiles_ini(&home, &["valid", "corrupt"]);
    let (state, _backend) = test_state();

    let err = dispatch(
        &state,
        "browser.import.run",
        json!({
            "sources": [
                browser_import_source_id(&valid),
                browser_import_source_id(&corrupt)
            ],
            "destination": {"kind": "existing", "profile": "Default"}
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "error");

    let history = dispatch(&state, "browser.history.list", json!({}))
        .await
        .unwrap();
    assert!(history.as_array().unwrap().is_empty());
    let bookmarks = dispatch(&state, "browser.bookmark.list", json!({}))
        .await
        .unwrap();
    assert!(bookmarks.as_array().unwrap().is_empty());
}

#[tokio::test]
#[serial_test::serial]
async fn browser_import_run_does_not_create_earlier_separate_profile_on_later_read_error() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let _home = EnvGuard::set("HOME", home.to_str().unwrap());
    let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
    let valid = create_firefox_import_source(&home, "valid");
    let corrupt = create_corrupt_firefox_import_source(&home, "corrupt");
    write_firefox_profiles_ini(&home, &["valid", "corrupt"]);
    let (state, _backend) = test_state();

    let err = dispatch(
        &state,
        "browser.import.run",
        json!({
            "sources": [
                browser_import_source_id(&valid),
                browser_import_source_id(&corrupt)
            ],
            "mode": "separate_profiles"
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.code(), "error");

    let profiles = dispatch(&state, "browser.profile.list", json!({}))
        .await
        .unwrap();
    assert!(!profiles.as_array().unwrap().iter().any(|profile| {
        profile["display_name"] == json!("valid") || profile["display_name"] == json!("corrupt")
    }));
}

#[tokio::test]
#[serial_test::serial]
async fn browser_import_rejects_ambiguous_and_invalid_params() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let _home = EnvGuard::set("HOME", home.to_str().unwrap());
    let _data = EnvGuard::set("XDG_DATA_HOME", tmp.path().join("data").to_str().unwrap());
    let (state, _backend) = test_state();

    for (method, params, expected) in [
        (
            "browser.import.preview",
            json!({"all": true, "sources": ["firefox:/tmp/profile"]}),
            "cannot combine all and sources",
        ),
        (
            "browser.import.preview",
            json!({"sources": [" \t "]}),
            "sources must not include empty source ids",
        ),
        (
            "browser.import.run",
            json!({"all": true, "mode": 42}),
            "Invalid parameter mode",
        ),
        (
            "browser.import.run",
            json!({"all": true, "destination": {"kind": 42}}),
            "Invalid parameter destination.kind",
        ),
        (
            "browser.import.run",
            json!({"all": true, "destination": {"kind": "create", "display_name": 42}}),
            "Invalid parameter destination.display_name",
        ),
        (
            "browser.import.preview",
            json!({
                "sources": ["firefox:/tmp/profile"],
                "include": {"history": false, "bookmarks": false, "cookies": false}
            }),
            "select at least one browser data type",
        ),
    ] {
        let err = dispatch(&state, method, params).await.unwrap_err();
        assert_eq!(err.code(), "invalid_param");
        assert!(
            err.to_string().contains(expected),
            "expected {expected:?}, got {err}"
        );
    }
}

#[tokio::test]
#[serial_test::serial]
async fn browser_history_search_requires_query() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvGuard::set("XDG_DATA_HOME", tmp.path().to_str().unwrap());
    let (state, _backend) = test_state();

    let err = dispatch(&state, "browser.history.search", json!({}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "missing_param");

    for query in [json!(""), json!(" \t "), json!(42)] {
        let err = dispatch(&state, "browser.history.search", json!({"query": query}))
            .await
            .unwrap_err();
        assert_eq!(err.code(), "invalid_param");
        assert!(err.to_string().contains("Invalid parameter query"));
    }
}

#[tokio::test]
#[serial_test::serial]
async fn browser_bookmark_add_rejects_empty_url() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvGuard::set("XDG_DATA_HOME", tmp.path().to_str().unwrap());
    let (state, _backend) = test_state();

    let err = dispatch(&state, "browser.bookmark.add", json!({"url": "   "}))
        .await
        .unwrap_err();
    assert_eq!(err.code(), "invalid_param");
}

#[tokio::test]
#[serial_test::serial]
async fn browser_history_rejects_invalid_limit() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvGuard::set("XDG_DATA_HOME", tmp.path().to_str().unwrap());
    let (state, _backend) = test_state();

    for (method, params) in [
        ("browser.history.list", json!({"limit": "5"})),
        ("browser.history.list", json!({"limit": -1})),
        (
            "browser.history.search",
            json!({"query": "example", "limit": 1.5}),
        ),
    ] {
        let err = dispatch(&state, method, params).await.unwrap_err();
        assert_eq!(err.code(), "invalid_param");
        assert!(err.to_string().contains("Invalid parameter limit"));
    }
}

#[tokio::test]
#[serial_test::serial]
async fn browser_bookmark_trims_url_and_title_and_rejects_bad_remove_url() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvGuard::set("XDG_DATA_HOME", tmp.path().to_str().unwrap());
    let (state, _backend) = test_state();

    let added = dispatch(
        &state,
        "browser.bookmark.add",
        json!({"url": " https://trim.test/ ", "title": " Trimmed "}),
    )
    .await
    .unwrap();
    assert_eq!(added["added"], json!(true));

    let listed = dispatch(&state, "browser.bookmark.list", json!({}))
        .await
        .unwrap();
    assert_eq!(listed[0]["url"], json!("https://trim.test/"));
    assert_eq!(listed[0]["title"], json!("Trimmed"));

    let invalid_title = dispatch(
        &state,
        "browser.bookmark.add",
        json!({"url": "https://title.test/", "title": 42}),
    )
    .await
    .unwrap_err();
    assert_eq!(invalid_title.code(), "invalid_param");
    assert!(invalid_title
        .to_string()
        .contains("Invalid parameter title"));

    let empty_remove = dispatch(&state, "browser.bookmark.remove", json!({"url": "  "}))
        .await
        .unwrap_err();
    assert_eq!(empty_remove.code(), "invalid_param");
    assert!(empty_remove.to_string().contains("url must not be empty"));

    let removed = dispatch(
        &state,
        "browser.bookmark.remove",
        json!({"url": " https://trim.test/ "}),
    )
    .await
    .unwrap();
    assert_eq!(removed["removed"], json!(true));
}

#[tokio::test]
#[serial_test::serial]
async fn browser_history_search_returns_results() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvGuard::set("XDG_DATA_HOME", tmp.path().to_str().unwrap());
    let (state, _backend) = test_state();

    // history is empty so search returns empty array (not an error)
    let results = dispatch(
        &state,
        "browser.history.search",
        json!({"query": "example"}),
    )
    .await
    .unwrap();
    assert!(results.as_array().unwrap().is_empty());
}
