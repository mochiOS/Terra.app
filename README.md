# Terra

Terra's native browser shell uses AppCore and ViewKit. The shipped backend is
explicitly **not connected**: entering an address displays an unavailable state;
it does not fetch data, invent history, or claim a secure connection.

## UI

- Standard closable document tabs, horizontal scrolling, and title truncation.
- A single compact toolbar: tabs normally show page titles as segments. Clicking
  the selected title or pressing Ctrl+L expands an inline address editor; Enter
  navigates and Escape restores the title. More uses the VK Symbols icon menu. There is
  no File/Edit/View menu strip. HTTP(S), hostnames, ports, IPv6 and IDN normalization.
  Script/file URLs and embedded credentials are rejected. No search provider is
  silently selected; free-text search requires a future provider setting.
- Back/Forward/Reload/Stop driven by actual engine notifications.
- Loading progress, engine errors, and engine-reported transport state.
- Bookmarks and history open on the right, beside their trigger. Narrow windows
  show the sidebar on its own.
- Ctrl+L, Ctrl+T, Ctrl+W, Ctrl+Shift+T, Ctrl+Tab, Ctrl+Shift+Tab, Ctrl+R,
  Ctrl+D, Ctrl+B and Ctrl+H. Escape stops an active load.
- Last-tab closure creates a new blank tab. Reopen uses a fresh engine session.

Bookmarks, history, closed tabs and drafts currently live **in this session**.
They are not persisted yet. Downloads, permission dialogs, print, page search,
zoom, account sync and extension UI are not presented as working features.

## Engine integration

The browser-specific boundary is `src/engine.rs`, inside Terra, not AppCore.
Replace `engine::Unavailable` in `TerraApp::new()` with an implementation of
`Engine`. Keep rendering/network sandboxing and all capability checks in the
appropriate engine/platform layers; this change adds no network or filesystem
rights to Terra's manifest.

1. `Create`, `Activate` and `Close` manage retained page sessions. Dispose all
   sessions when the adapter drops; closed tab IDs must never be reused.
2. `send` enqueues navigation/history/reload/stop operations without blocking the
   UI thread. Echo the supplied tab and navigation IDs, even for redirects.
3. Emit `Committed` with the **final URL**, title, actual transport security and
   engine history availability. Emit `Finished` or `Failed` as the terminal event.
   A navigation without a commit never becomes a history entry. `TitleChanged`
   identifies the committed document and remains usable after `Finished`.
4. `page(tab)` returns a retained ViewKit `View`. It receives the actual page
   bounds, pointer/key/scroll events and ordinary focus/edit commands. The adapter
   must implement page rendering, input, IME, clipboard, accessibility and any
   renderer-specific lifetime/resize handling; none is simulated by the shell.
5. Drain notifications non-blockingly with `poll`. `next_poll_at` must supply a
   deadline while notifications/animation require polling, or the adapter must
   arrange a platform wakeup. Idle/disconnected shells schedule no polling.
   At most 64 notifications are consumed per paint to avoid starving input.

The model rejects stale generations, terminal-event late arrivals and events for
closed tabs. Stop invalidates the old generation. UI connection indicators never
infer TLS validation from the spelling of a URL. Policy for popup windows,
permissions, downloads and external protocols still needs explicit engine-side
integration and user-facing decisions before those features can be enabled.

## Validation and Linux preview

`mmake ui` includes Terra in the desktop application catalogue.
`mmake preview-terra` launches it independently; `mmake ui-check` builds the shared
preview workspace. Tests use a controllable in-process engine adapter, not a
fake production web renderer, to verify event routing and state changes.
