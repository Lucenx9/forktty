# Browser pane SP3 — profiles, persistence, history, import

Date: 2026-05-24
Epic: full cmux browser-feature parity for the forktty browser pane.
Builds on SP1 (`SurfaceKind::Browser`, WebKitGTK6 embed, `browser.open`/`navigate`)
and SP2 (scriptable verbs snapshot/click/fill/eval, socket→GTK command channel),
both merged to `main`.

Current implementation status on `main`: P1 persistence and P2 profiles are
implemented; P3 has the pure core history/bookmark stores plus socket verbs;
P3 CLI mirrors/GTK wiring and P4 import are still backlog. WebKit/GTK browser code
is behind the existing `browser` cargo feature, while the pure core profile and
history/bookmark stores compile unconditionally.

## Goal

Bring the browser pane to parity with cmux's browser feature:

1. **Persistence** — cookies, localStorage, and other website data survive forktty
   restarts. Before P1, `WebView::new()` used an ephemeral default session, so
   every relaunch logged out.
2. **Profiles** — multiple isolated browsing identities (cmux's `BrowserProfileStore`),
   each pane bound to one profile, selectable per pane.
3. **History + bookmarks** — per-profile visited-URL history and bookmarks, queryable
   over the socket and surfaced as address-bar completion (WebKit keeps no on-disk
   history of its own).
4. **Import wizard** — pull cookies, history, and bookmarks from real installed
   browsers (Firefox + Chromium family) into a forktty profile, via a GTK wizard that
   mirrors cmux's source→destination plan resolver.

This is a large epic. It is split into **four phases**, each independently shippable
as its own PR. The phase boundaries are the unit-of-review; the spec below is one
coherent design, but implementation proceeds P1 → P2 → P3 → P4.

Non-goals (SP3): multiple tabs per pane; devtools UI; download manager beyond what
SP1/SP2 already do; syncing profiles across machines; exporting forktty data back to
other browsers; importing saved passwords (cookies/history/bookmarks only).

## Phase overview

| Phase | Delivers | Core change? | New deps |
|-------|----------|--------------|----------|
| P1 Persistence | per-profile persistent `NetworkSession`; one fixed Default profile | no | none |
| P2 Profiles | `ProfileId` on `Browser` surface; profile CRUD + per-pane binding | yes | `uuid` (if not present) |
| P3 History+bookmarks | per-profile history/bookmark stores and socket verbs are implemented; CLI mirrors and address completion remain pending | no | `rusqlite` |
| P4 Import wizard | read+decrypt Firefox/Chromium data; plan resolver; GTK wizard | no | `soup`, `rusqlite`, `aes`/`cbc`, `pbkdf2`/`sha2`, `secret-service` |

Each phase's "Out of scope until next phase" is implicit in the table: e.g. P1 ships a
single Default profile with no UI to create others; P2 adds the profile system on top.

## Constraints discovered

- `BrowserPaneWidget::new` (`browser_pane.rs:68`) constructs the WebView with
  `WebView::new()`, which binds the process-default ephemeral network session — no
  disk persistence. webkit6 0.5 exposes:
  - `NetworkSession::new(data_directory: Option<&str>, cache_directory: Option<&str>)`
    → a persistent session rooted at those dirs (`new_ephemeral()` is the opt-out).
  - `WebView::builder().network_session(&session).build()` to bind a session at
    construction (the `network-session` property is construct-only).
  - `session.cookie_manager() -> Option<CookieManager>`, with
    `set_persistent_storage(filename, CookiePersistentStorage::Sqlite)` and
    `add_cookie(soup::Cookie, cb)` / `all_cookies(cb)`.
- Core `SurfaceKind::Browser` now carries `{ url, profile }`. Session save/restore
  serializes it, with serde compatibility for older saved browser surfaces.
- `WorkspaceModel::open_browser(workspace_id, url, profile, axis) -> Option<Surface>` is the
  single model entry point for creating a browser pane (used by socket `browser.open`
  and the UI globe button).
- The socket runs on its own OS thread with a tokio runtime; the WebView lives on the
  GTK main thread. SP2's `async_channel` + `oneshot` command bridge (`BrowserCommand`)
  is the established pattern for socket→GTK calls that need a WebView; profile/history
  verbs that only touch on-disk stores or model metadata do NOT need the bridge.
- cmux reference (`/home/simone/git-misc/cmux`, macOS/WKWebView): `BrowserProfileStore`
  keys profiles by UUID, one `WKWebsiteDataStore(forIdentifier:)` each; history is its
  own `browser_history.json` per profile dir; import uses `SQLite3` + a
  `BrowserImportPlanResolver` (modes `.singleDestination` / `.separateProfiles`, reuses
  same-named destination profiles). Our P4 ports the resolver logic; the storage and
  decrypt layers are Linux-specific (cmux relies on macOS Keychain).

## Storage layout

```text
$XDG_DATA_HOME/forktty/browser_profiles/        (default ~/.local/share/forktty/…)
  profiles.json                  # [{ id, display_name, created_at, is_default }]
  <uuid>/
    data/                        # NetworkSession data_directory (localStorage, IndexedDB, …)
    cache/                       # NetworkSession cache_directory
    cookies.sqlite               # CookieManager persistent storage
    history.sqlite               # P3: visits (url, title, visit_count, last_visit_us)
    bookmarks.json               # P3: [{ url, title, added_at }]
```

The Default profile uses a fixed well-known UUID constant so P1 (which predates the
profile system) writes to the same directory P2 later manages.

---

## P1 — Persistence

### Architecture

```text
forktty-ui-gtk (feature = "browser")
  browser_session.rs  (new)
    DEFAULT_PROFILE_ID: fixed Uuid const
    profiles_root() -> PathBuf            # XDG data dir + browser_profiles
    fn session_for(profile_id) -> NetworkSession
        # NetworkSession::new(Some(data_dir), Some(cache_dir));
        # cookie_manager().set_persistent_storage(cookies.sqlite, Sqlite);
        # cache the NetworkSession per id (one per process; reused across panes)

  browser_pane.rs
    BrowserPaneWidget::new takes the profile id (P1: always DEFAULT_PROFILE_ID),
    builds the WebView via WebView::builder().network_session(&session).build()
```

### Components

- **`browser_session.rs`** — owns a process-wide `RefCell<HashMap<Uuid, NetworkSession>>`
  (GTK main-thread only, so no locking). `session_for(id)` lazily creates the session,
  ensures its directories exist, wires persistent cookie storage, caches and returns a
  clone (WebKit sessions are GObjects; cloning is a refcount bump). Reusing one session
  per profile across panes is required — two persistent sessions over the same dir would
  conflict.
- **`browser_pane.rs`** — `new` gains a `profile_id: Uuid` parameter; replaces
  `WebView::new()` with the builder call binding `session_for(profile_id)`. Everything
  else (driver injection, address bar, SP2 wiring) is unchanged.
- **`gtk_app.rs`** — `browser_pane_widget(...)` passes `DEFAULT_PROFILE_ID` for P1.

### Error handling

- Data dir not creatable (permissions): log a warning, fall back to an ephemeral
  session so the pane still works (no persistence that run); do not crash.
- `cookie_manager()` returning `None` (shouldn't happen for a non-ephemeral session):
  log, continue without persistent cookies.

### Testing

- Manual (`--features browser`, display): log into a site, restart forktty, reopen a
  browser pane → still logged in; `cookies.sqlite` exists and is non-empty.
- `--features gtk-vte` (no browser) still builds; non-browser path untouched.
- Automated: `profiles_root()` / path-construction unit tests (no GTK). Session
  creation itself needs a display, so it stays manual (consistent with existing
  browser-pane test posture).

---

## P2 — Profiles

### Core change

```text
forktty-core
  SurfaceKind::Browser { url: String, #[serde(default)] profile: ProfileId }
  struct ProfileId(Uuid);  // Default == the well-known DEFAULT_PROFILE_ID
  WorkspaceModel::open_browser(workspace_id, url, profile, axis)
```

`#[serde(default)]` makes old saved sessions (no `profile` key) deserialize to the
Default profile — backward compatible. `ProfileId` lives in core so save/restore and
the model share one type; it wraps `uuid::Uuid`.

A separate **`ProfileStore`** (core, pure) owns `profiles.json`: list/create/delete,
each entry `{ id, display_name, created_at, is_default }`. The built-in Default entry
is synthesized if the file is absent. Deleting a profile is metadata-only in P2 (its
on-disk data dir is removed by the GTK layer, which owns the filesystem session dirs —
see below); the Default profile cannot be deleted.

### Socket / CLI

New verbs (always dispatch; return "browser automation unavailable" when the build
lacks `browser`, matching SP2's gating):

- `browser.profile.list` → `[{ id, display_name, is_default }]`
- `browser.profile.create { display_name }` → `{ id }`
- `browser.profile.delete { id }` → ok / error if Default or in-use
- `browser.open` gains optional `profile` (id or display_name; default = Default)

CLI: `forktty browser profile list|create <name>|delete <id>`, and
`forktty browser open --profile <id|name> <url>`.

`profile.delete` of an in-use profile (a live pane bound to it) is refused with a clear
error; the caller closes those panes first. Removing the on-disk data dir happens in
the GTK layer after the model confirms no pane references it.

### GTK

- The globe button's open path (UI) uses Default. A per-pane profile **switcher** in
  the browser-pane chrome is **out of scope for P2** (deferred); P2's profile selection
  is socket/CLI-driven plus whatever the import wizard (P4) sets. (Rationale: keeps P2
  reviewable; the chrome dropdown is additive and can land with P4's UI work.)
- When a `Browser { profile }` surface is built, `browser_pane_widget` passes that
  profile id to `BrowserPaneWidget::new`, so each pane gets its profile's session.

### Error handling

- Unknown profile id/name on `browser.open`: error, no pane created.
- Session restore referencing a profile whose `profiles.json` entry was removed
  out-of-band: re-synthesize a minimal entry (id + generated name) so the pane still
  opens against its existing data dir, rather than silently dropping the pane.

### Testing

- Core unit tests: `ProfileStore` create/list/delete, Default-undeletable, in-use
  refusal; `SurfaceKind::Browser` serde round-trip incl. legacy (no-`profile`) JSON.
- Socket tests: profile verbs over the headless harness (no WebView needed —
  metadata only); `browser.open --profile` resolves name→id.
- Manual: two panes on two profiles are cookie-isolated (log into the same site as
  different users).

---

## P3 — History + bookmarks

### Architecture

```text
forktty-ui-gtk (feature = "browser")
  browser_history.rs (new)
    open(profile_id) -> rusqlite::Connection  # history.sqlite, schema-migrated
    record_visit(conn, url, title)            # upsert visit_count++, last_visit
    query(conn, prefix|substring, limit)
    bookmarks.json read/write (serde)

  browser_pane.rs
    on WebView load-changed (Committed, top frame) + notify::title:
      record_visit into the pane's profile history (skip about:blank, errors)

  socket bridge / direct:
    history/bookmark verbs read the on-disk store directly on the socket thread
    (rusqlite is sync; no WebView needed) — NO command-channel round-trip.
```

History writes happen on the GTK thread (signal handlers); history/bookmark *reads*
for the socket happen on the socket thread opening its own read connection to the same
sqlite file (WAL mode, so concurrent reader + writer is fine). Each side opens its own
connection; sqlite handles the locking.

### Socket / CLI

- `browser.history.list { profile?, limit? }`, `browser.history.search { query, … }`,
  `browser.history.clear { profile? }`
- `browser.bookmark.add { url, title?, profile? }`, `browser.bookmark.list`,
  `browser.bookmark.remove { url }`
- CLI mirrors: `forktty browser history list|search <q>|clear`,
  `forktty browser bookmark add <url>|list|remove <url>`.

### GTK

- Address-bar `gtk::Entry` gains a `gtk::EntryCompletion` (or list model) populated from
  the profile's history prefix-match, so typing offers visited URLs. Selecting one
  navigates. Bookmarks surface in the same completion, flagged.

### Error handling

- sqlite open/migrate failure: log, disable history for that pane (navigation still
  works); verbs return an "history unavailable" error rather than crashing.
- Malformed `bookmarks.json`: back it up and start fresh (don't lose navigation).

### Testing

- Unit (no GTK/display): schema migration, `record_visit` upsert semantics, search
  prefix/substring + limit, bookmark add/dedupe/remove on a temp sqlite + temp json.
- Socket tests: history/bookmark verbs against a seeded temp profile dir.
- Manual: visit pages → `history list` shows them with counts; completion offers them.

---

## P4 — Import wizard

### Architecture

```text
forktty-import (new crate, NO GTK / NO webkit)
  sources.rs    discover installed browsers + their profiles
  cookies.rs    read source cookie sqlite; decrypt values
  history.rs    read source history/bookmarks
  plan.rs       ImportPlanResolver (ported from cmux)
  model.rs      SourceProfile, SourceBrowser, ImportPlan, ImportEntry, ImportResult

forktty-ui-gtk (feature = "browser")
  import_wizard.rs   Adw wizard UI driving forktty-import + ProfileStore + sessions
  socket verbs       browser.import.sources / browser.import.run (scriptable path)
```

The import engine is a **GTK-free crate** so the plan resolver and decrypt logic are
unit-testable headless (cmux tests `BrowserImportPlanResolver` the same way). The GTK
wizard and the socket verbs are two front-ends over the same engine.

### Source discovery (`sources.rs`)

Probe known roots; each existing one yields a `SourceBrowser` with its profiles:

| Family | Config root | Profile dirs | Cookie file | Safe-Storage label |
|--------|-------------|--------------|-------------|--------------------|
| Firefox | `~/.mozilla/firefox` | dirs w/ `cookies.sqlite` (read `profiles.ini`) | `cookies.sqlite` | — (plaintext) |
| Chrome | `~/.config/google-chrome` | `Default`, `Profile *` (read `Local State`) | `Cookies` | "Chrome Safe Storage" |
| Chromium | `~/.config/chromium` | same | `Cookies` | "Chromium Safe Storage" |
| Brave | `~/.config/BraveSoftware/Brave-Browser` | same | `Cookies` | "Brave Safe Storage" |
| Edge | `~/.config/microsoft-edge` | same | `Cookies` | "Microsoft Edge Safe Storage" |
| Vivaldi | `~/.config/vivaldi` | same | `Cookies` | "Vivaldi Safe Storage" |

Chromium profile display names come from `Local State` → `profile.info_cache`.

### Cookie decrypt (`cookies.rs`)

- **Firefox**: `cookies.sqlite` `moz_cookies` table; values are plaintext. Map to
  `soup::Cookie` (name, value, host, path, expiry, secure, http_only).
- **Chromium family**: `Cookies` sqlite `cookies` table; `encrypted_value` blob. On
  Linux:
  - `v10` prefix → key = PBKDF2-HMAC-SHA1(`"peanuts"`, salt `b"saltysalt"`, 1 iter, 16B).
  - `v11` prefix → key = PBKDF2 of the Secret Service secret stored under the family's
    "… Safe Storage" label (via the `secret-service` crate / org.freedesktop.secrets).
    Fallback to `v10`/`peanuts` if the keyring is locked or absent.
  - Decrypt: AES-128-CBC, IV = 16 spaces, strip PKCS#7 padding; recent Chrome prefixes
    the plaintext with a 32-bit length/hash header — strip the documented prefix bytes.
  Decode to `soup::Cookie`. Cookies that fail to decrypt are skipped and counted in the
  result (never abort the whole import).
- **Security note:** importing reads another application's stored credentials from the
  user's own machine, with the user's own keyring unlock — a local, user-initiated,
  user-authorized operation. forktty stores the resulting cookies only inside the
  target profile's own `cookies.sqlite`. No secret leaves the machine. The keyring
  prompt is the OS's own consent gate; we surface what we're reading in the wizard.

### History/bookmarks import (`history.rs`)

- Firefox `places.sqlite` (`moz_places`, `moz_bookmarks`); Chromium `History`
  (`urls`), `Bookmarks` (JSON). Map into forktty's P3 history/bookmark stores.

### Plan resolver (`plan.rs`, ported from cmux)

```text
ImportPlan { mode: SingleDestination | SeparateProfiles, entries: [ImportEntry] }
ImportEntry { sources: [SourceProfile], destination: Existing(ProfileId) | Create(name) }

defaultPlan(selected_sources, destinations, preferred_single_dest):
  one source        -> SingleDestination (into preferred/Default)
  many sources      -> SeparateProfiles (one dest per source)
separateProfilesPlan: reuse a destination profile whose display_name matches the
  source (trimmed, case-insensitive); otherwise Create with a stable de-duped name.
```

Mirrors cmux's `BrowserImportPlanResolver` cases (single vs separate, same-named reuse,
stable create-names on display-name collisions); covered by ported unit tests.

### Execution

For each `ImportEntry`: resolve/create the destination profile (via `ProfileStore`),
then for each source profile read cookies/history/bookmarks and write into the
destination — cookies via that profile's `CookieManager::add_cookie` (GTK-side, over
the SP2-style command bridge since it needs the live session), history/bookmarks via
the P3 sqlite/json store directly. Returns `ImportResult { cookies, history, bookmarks,
skipped }` counts.

### GTK wizard (`import_wizard.rs`)

`Adw`-based multi-step window, opened from an "Import…" entry in the browser-pane
chrome (feature-gated; absent without `browser`):

1. **Source select** — discovered browsers/profiles as a checkbox list (multiselect).
2. **Destination map** — show `defaultPlan`; let the user switch single↔separate and
   edit destination names / pick existing profiles.
3. **Run + progress** — execute, show per-entry counts and skip totals; on finish offer
   "Open a pane in <profile>".

### Socket / CLI (scriptable path over the same engine)

- `browser.import.sources` → discovered sources/profiles.
- `browser.import.run { sources, mode?, mappings? }` → `ImportResult`.
- CLI: `forktty browser import list`, `forktty browser import run --from <family>
  [--src-profile …] [--dest-profile …] [--separate]`.

### Error handling

- No source browsers found: wizard shows an empty-state; `import.sources` returns `[]`.
- Locked keyring / Secret Service unavailable: fall back to `v10`; if that also fails,
  skip the affected cookies and report the count + a hint, don't abort.
- Source DB locked (browser running): copy the sqlite file to a temp path and read the
  copy (standard approach), so a running Chrome doesn't block import.
- Partial failure: per-item skip counts in `ImportResult`; the import never leaves a
  half-created profile (create destination first, then fill; on hard failure the empty
  profile is harmless and reusable).

### Testing

- `forktty-import` unit tests (headless): plan resolver (ported cmux cases); cookie
  decrypt against synthetic `v10` blobs (known key → known plaintext) and a synthetic
  Firefox `cookies.sqlite`; source discovery against a temp fake-profile tree.
- Socket tests: `import.sources` / `import.run` against fake source trees writing into
  a temp forktty profile.
- Manual (display + a real installed browser): run the wizard end-to-end; verify the
  target profile is logged into a site the source browser was logged into.

---

## Cross-cutting

### Feature gating

WebKit/GTK SP3 code is under the `browser` feature, consistent with SP1/SP2.
Pure stores (`ProfileStore`, `HistoryStore`, `BookmarkStore`) live in
`forktty-core` and compile unconditionally so socket/CLI code can share them.
SP3 P2 profile verbs and P3 history/bookmark socket verbs dispatch regardless
of the GUI feature; scripting verbs still return "browser automation unavailable"
when no browser command channel is wired. `forktty-import` remains planned and
would be depended on by `forktty-ui-gtk` under `browser`.

### Dependencies added

- `uuid` (P2) — if not already in the tree; `ProfileId` wrapper.
- `rusqlite` (P3, P4) — bundled sqlite, read history/cookies; WAL for concurrent R/W.
- `soup` (P4) — construct `soup::Cookie` for `CookieManager::add_cookie` (webkit6 pulls
  soup transitively; add as a direct dep).
- `aes` + `cbc` + `pbkdf2` + `sha1`/`sha2` (P4) — Chromium cookie decrypt.
- `secret-service` (P4) — read the "… Safe Storage" key from the Linux keyring.

### Build sequence (per phase, each a PR)

1. **P1** persistence: `browser_session.rs`; switch `WebView::new()` → builder + session.
2. **P2** profiles: core `ProfileId`/`Browser.profile`/`ProfileStore`; socket+CLI verbs;
   thread profile through `open_browser` and pane construction.
3. **P3** history+bookmarks: `browser_history.rs` and socket verbs are implemented;
   visit recording, CLI mirrors, and address-bar completion remain pending.
4. **P4** import: `forktty-import` crate (discovery/decrypt/plan, headless-tested);
   socket+CLI; `import_wizard.rs` GTK UI.

Each phase: `cargo build` + `clippy` + `fmt` clean on both `--features browser` and
`--features gtk-vte`; existing tests green; new tests per the phase's Testing section.

## Out of scope (SP3 epic)

- Multiple tabs per pane, devtools, a separate download manager.
- Profile sync across machines; exporting forktty data to other browsers.
- Importing saved passwords / autofill / extensions.
- A per-pane profile-switcher dropdown is deferred (lands with P4's chrome work or a
  follow-up), to keep P2 reviewable.
