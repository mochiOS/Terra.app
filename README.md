# Terra

Native browser chrome built with AppCore and ViewKit. **No web engine is shipped
or simulated.** Libraries, tabs and settings work without one; web pages, page
search, zoom, Reader, printing and transfers need a connected adapter.

## Browser features

- Compact title tabs expand into an address editor when selected or with Ctrl+L.
  Local suggestions search bookmarks, history and Reading List. No remote
  autocomplete requests are made. Narrow windows put secondary actions in More.
- Start Page displays saved bookmarks and links to the browser libraries.
- Create, close, reopen, duplicate, pin, move and bulk-close tabs. Pinned tabs
  survive bulk closure. Right-click a tab for its actions. Tabs also has a native
  actions menu and named groups (up to 16 groups / 64 total tabs).
- Bookmarks can be added by address before an engine is attached, edited, searched
  and removed. History records only actual committed navigations; no synthetic
  visits are inserted. Reading List stores links, not offline page contents.
- Bookmarks, history, Reading List, groups, selected tab, pin/zoom preferences and
  search settings persist per user. Previous tabs restore as uncommitted URLs;
  a connected engine loads the selected tab and lazily loads other restored tabs.
  No transport-security assertion or loaded-document state is restored.
- Browser Settings offers address-only mode, Google, DuckDuckGo and Bing. Search
  is opt-in; queries are encoded and submitted only on explicit navigation.
  Restore Previous Tabs can be disabled independently of saved libraries.
- Back, Forward, Reload, Stop, loading progress, failures and connection state
  are driven by actual engine state. HTTP(S), hostnames, ports, IPv6 and IDNs
  are supported; script/file URLs and embedded credentials are rejected.
- Find on Page has native keyboard focus, next/previous results and match counts.
  Late results are scoped to both the committed document and search request.
- Per-tab zoom (50–300%), Reader and Print requests are available only when the
  adapter advertises support. A failed tool request does not destroy the page.
- Downloads has separate offered, active, completed, failed and cancelled states.
  Save requires explicit acceptance. The adapter must obtain its output through
  the system file broker; a page-provided filename never grants write access.
  No file is auto-opened. Completion requires an actual adapter success event.
- User-initiated links can request foreground/background tabs. Requests from
  stale documents and unsolicited popups are blocked.

## Keyboard

Ctrl+L: address; Ctrl+T / Ctrl+W: new / close; Ctrl+Shift+T: reopen;
Ctrl+Tab / Ctrl+Shift+Tab: next / previous in the active group;
Ctrl+1–8 / Ctrl+9: numbered / last tab; Ctrl+R: reload;
Alt+Left / Alt+Right: back / forward; Ctrl+D: bookmark;
Ctrl+B / Ctrl+H / Ctrl+J: bookmarks / history / downloads;
Ctrl+F: find; Ctrl+G / Ctrl+Shift+G: next / previous match;
Ctrl+Plus / Ctrl+Minus / Ctrl+0: zoom; Escape: dismiss editing/find or stop loading.

## Storage

The versioned JSON store is bounded to 2 MiB. Saves validate the model, compare
with the last loaded revision under an exclusive sibling lock, and atomically
replace the file. A second process, damaged data, an unknown schema or an existing
lock produces an explicit error instead of silently discarding saved data.
Writes are coalesced for 500 ms; final state is also saved on shutdown. This is a
small local store, not an account-sync service. Search drafts are not saved as URLs.

Linux previews use `$MOCHIOS_PREVIEW_CONFIG_ROOT/terra/browser-v1.json`, falling
back to `$HOME/.local/share/mochios/terra/browser-v1.json`. mochiOS uses the existing
per-user application data convention under
`/libraries/applications/org.mochios.terra/users/<user>/browser-v1.json`.
The manifest declares ordinary user-filesystem and clipboard capabilities; no
network or privileged storage right is added.

## Engine adapter contract

The browser-specific boundary is `src/engine.rs`, inside Terra, not AppCore.
Replace `engine::Unavailable` in `TerraApp::new()` with an `Engine` implementation.

1. `Create`, `Activate`, `Close` manage retained page sessions. Closed tab IDs
   must never be reused. Dispose all sessions when the adapter drops.
2. `send` enqueues operations without blocking the UI thread. Navigation events
   echo the supplied tab and generation IDs, including redirects. `Committed`
   contains the final URL, title, real transport status and history availability.
   `Finished` / `Failed` terminate a load. No commit means no history visit.
3. `page(tab)` supplies a retained native ViewKit view that implements rendering,
   pointer/key/scroll input, IME, clipboard, accessibility and resizing. Nothing in
   the shell pretends to render HTML.
4. `features()` defaults to no optional support. Advertise `find`, `zoom`,
   `downloads`, `reader` and `printing` only when actually implemented. Page-tool
   commands carry a committed-document ID. `FindResult` also echoes its request ID.
5. `DownloadOffered` must precede transfer or file writing. IDs are unique for the
   engine lifetime. `AcceptDownload` starts the brokered save flow; cancellation
   must stop it. Emit `DownloadFinished` only after successful file output.
6. `OpenTabRequested` identifies the source document and attests a user gesture.
   The shell validates URLs and preserves the foreground tab for background opens.
7. `poll` is nonblocking. `next_poll_at` or a platform wakeup must arrange event
   draining when work arrives. At most 64 events are processed per paint. Idle,
   disconnected shells do not continuously poll.

Engine sandboxing, network/certificate handling, cookies, storage, permissions,
web dialogs and actual download destinations remain the adapter/platform's
responsibility. Cloud sync, extensions, translation, password autofill, website
permissions UI, bookmark folders/import/export and saved offline articles are
not implemented. This is a browser shell with integration contracts, not a claim
of complete Safari parity or working web access without an engine.

## Validation / preview

`mmake ui` includes Terra. `mmake preview-terra` launches it independently;
`mmake ui-check` builds the shared preview workspace. Tests use an explicitly
injected engine adapter to verify routing and failures. Production always uses
the disconnected backend until a real adapter is supplied.
