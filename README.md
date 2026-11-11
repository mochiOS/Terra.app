# Terra

Native browser chrome built with AppCore and ViewKit. WebKit is a separately
installed library, never embedded in Terra. The optional `webkit-engine` native
build connects the major-1 `libwebkit.so.1` ABI. Default and Linux previews remain
shell-only. The native connection has contract tests; mochiOS process/rendering
validation is still pending, so this is not a claim of working web access yet.

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
`engine::connect()` selects the native client only in a mochiOS build with
`webkit-engine`. `src/engine/native.rs` owns the worker, navigation IDs and ViewKit
page adapter; `abi.rs` mirrors the reviewed C layout. All engine calls occur on
one worker thread. Painting reads retained immutable frames and records the next
viewport size; it never dispatches WebKit or waits for rendering.

The initial adapter supports navigation, history commands, zoom, pointer/scroll,
committed text and basic keyboard input. Find, Reader, printing, downloads, page
accessibility and full IME composition are not advertised. TLS status remains
Unknown until the engine provides validated transport information. No HTTPS
padlock is inferred from an address. Unsupported operations return errors.

`terra:///engine-check` is the sole local test address. Its bundled UTF-8 HTML is
sent through the host-only Load HTML command with an opaque origin; no arbitrary
`file:` URL or filesystem privilege is introduced. It contains a text input, a
JavaScript button and a scrolling document for testing in Terra itself.

The engine is startup-linked, not dlopened after startup: WPE/Epoxy dependencies
use initial-exec TLS and require the normal authenticated initial ELF graph.

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

`mmake ui` includes Terra. `mmake preview-app APP=terra` launches it independently;
`mmake ui-check` builds the shared preview workspace. Tests use an explicitly
injected engine adapter to verify routing and failures. Default builds use the disconnected backend. A native feature build requires the
independent engine package and a compatible shared SDK.

## Independent mochiOS build and distribution

Terra owns its build entry point and engine dependencies. The OS exports a generic
SDK and does not build or include Terra in the base image. Linux `mmake ui` still
discovers Terra for UI development.

Prepare the generic SDK from the OS workspace, then build from this repository:

```sh
mmake app-sdk
cd applications/terra
python3 tools/build.py build --sdk ../../out/newlib-port/sdk/share/app-build.json --shell-only
```

Package that build using the standard MPKG tool:

```sh
python3 tools/build.py package --sdk ../../out/newlib-port/sdk/share/app-build.json --shell-only --mpack ../../out/mmake/host-tools/release/mpack
```

`--binary <ELF>` reuses an existing native build instead of compiling again.
`out/product/` contains the named `Terra.app` artifact, the digest-bound manifest,
MPKG payload and `artifacts.json`. `out/Terra.mpkg` is unsigned unless an explicit
`--msign <tool> --identity <selector>` is supplied. Signing can run separately in
the release environment; no key or workstation identity is needed for building.

WebKit is distributed independently by mochiOS/libs. Terra never embeds or
repackages its engine. `--engine-prefix` is rejected with migration guidance;
use `--shell-only` while the native adapter is unavailable. Only Terra.app is
packaged here, and no shell build claims a working browsing engine.

The built-in App Store owns the initial-application plan. On a session with the
browser missing, Binder opens its declared setup handler, and App Store directs
the user to the normal release/Get flow. Downloads and package installation use
the existing verification and package service. Unavailable catalog entries remain
unavailable; the setup does not claim installation or create a substitute browser.
A published, signed release and working OS network are needed for a real download.

WebKit source acquisition and builds are owned by [mochiOS/libs](https://github.com/mochiOS/libs).
See [vendor/README.md](vendor/README.md). Terra does not invoke the OS build
system or fetch an engine source tree. The native adapter is implemented, but the independently installed engine package,
helper Capability declarations and mochiOS runtime validation are still required.

A native engine build uses an independently installed prefix and shared SDK:

```sh
python3 tools/build.py build --sdk /path/to/rust-sdk/share/app-build.json \
  --webkit-prefix /path/to/verified-engine-prefix \
  --shared-sdk /path/to/shared-cxx-sdk \
  --engine-dependency /path/to/reviewed-engine-identity.toml
```

The identity file contains the five existing `library_dependency` fields:
`package`, `developer`, `subject_key_id`, `provenance`, `abi`. Use the actual
installed engine package identity; no example signing identity is assumed.
The build replaces static final-link flags with the shared SDK driver, while
preserving source path remapping. Packaging inspects the real ELF DT_NEEDED:
a shell cannot be labeled engine-connected, and an engine-linked binary cannot
be packaged as a shell to omit its library authorization. Engine assets are
never copied into Terra. These build options still require native link/runtime
validation before release. No release or signing operation is performed by build.
