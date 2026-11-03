//! Terra-owned engine boundary. No browser-specific API belongs in AppCore.
use appcore::prelude::View;
use std::{rc::Rc, time::Instant};

/// Stable session identity; closed tab identities are never reused.
pub(crate) type TabId = u64;
/// Navigation generation. An adapter must echo this on every navigation event.
pub(crate) type NavigationId = u64;

/// Requests the adapter translates into its engine's native operations.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Command {
    Create(TabId),
    Activate(TabId),
    Close(TabId),
    Navigate {
        tab: TabId,
        navigation: NavigationId,
        url: String,
    },
    Back {
        tab: TabId,
        navigation: NavigationId,
    },
    Forward {
        tab: TabId,
        navigation: NavigationId,
    },
    Reload {
        tab: TabId,
        navigation: NavigationId,
    },
    Stop {
        tab: TabId,
        navigation: NavigationId,
    },
    /// Changes the current document scale, expressed as a percentage (50–300).
    SetZoom {
        tab: TabId,
        document: NavigationId,
        percent: u16,
    },
    /// Searches the current document; echo the request ID in FindResult.
    Find {
        tab: TabId,
        document: NavigationId,
        request: u64,
        text: String,
        backwards: bool,
    },
    /// Removes all page-search highlights for this document.
    ClearFind {
        tab: TabId,
        document: NavigationId,
    },
    /// Accepts an offered download. The adapter must obtain a brokered destination;
    /// filenames and URLs are untrusted and never authorize filesystem access.
    AcceptDownload(u64),
    /// Toggles the adapter's reader view for the committed document.
    ToggleReader {
        tab: TabId,
        document: NavigationId,
    },
    /// Opens the adapter's native print flow; sending this never means printing succeeded.
    Print {
        tab: TabId,
        document: NavigationId,
    },
    /// Cancels an offered or active download; late progress must not restart it.
    CancelDownload(u64),
}

/// Transport state reported by the engine, never inferred from an HTTPS URL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Security {
    /// No current, engine-validated transport information.
    Unknown,
    /// The engine has validated the committed document's secure transport.
    Secure,
    /// The engine reports an insecure connection for the committed document.
    Insecure,
}

/// Top-level navigation notifications; subframe loads must not be reported as
/// commits. Commits include the final redirected URL.
/// Report engine history availability rather than reconstructing it in the UI.
#[derive(Clone, Debug)]
pub(crate) enum Event {
    /// Requests a new tab from a committed document. Unsolicited popups are
    /// blocked by the shell; adapters must attest an actual user gesture.
    OpenTabRequested {
        source: TabId,
        document: NavigationId,
        url: String,
        background: bool,
        user_initiated: bool,
    },
    /// Counts are for the matching document and find request only; zero is valid.
    FindResult {
        tab: TabId,
        document: NavigationId,
        request: u64,
        current: usize,
        total: usize,
    },
    /// Offers a download without starting transfer or writing any file.
    DownloadOffered {
        id: u64,
        tab: TabId,
        document: NavigationId,
        url: String,
        name: String,
        total: Option<u64>,
    },
    DownloadProgress {
        id: u64,
        received: u64,
    },
    /// Emit only after the broker-authorized file was successfully written.
    DownloadFinished {
        id: u64,
    },
    DownloadFailed {
        id: u64,
        message: String,
    },
    /// Updates an already committed document, including after loading finishes.
    TitleChanged {
        tab: TabId,
        document: NavigationId,
        title: String,
    },
    Progress {
        tab: TabId,
        navigation: NavigationId,
        fraction: f32,
    },
    Committed {
        tab: TabId,
        navigation: NavigationId,
        url: String,
        title: String,
        security: Security,
        can_back: bool,
        can_forward: bool,
    },
    Finished {
        tab: TabId,
        navigation: NavigationId,
    },
    Failed {
        tab: TabId,
        navigation: NavigationId,
        message: String,
    },
}

/// One engine instance owns all page sessions. Its views must implement the
/// ordinary ViewKit input, IME, focus, clipboard and accessibility contracts.
/// `send` must enqueue work without blocking the UI thread. Events must retain
/// their original tab/generation, including redirects and late completions.
/// Optional operations must be explicitly advertised by a connected adapter.
#[derive(Clone, Copy, Default)]
pub(crate) struct Features {
    pub find: bool,
    pub zoom: bool,
    pub downloads: bool,
    pub reader: bool,
    pub printing: bool,
}

pub(crate) trait Engine {
    /// Returns only operations actually implemented by this adapter.
    fn features(&self) -> Features {
        Features::default()
    }
    /// Whether an engine has actually been attached to this browser instance.
    fn available(&self) -> bool;
    /// Enqueues an operation, or returns a user-facing failure without success.
    fn send(&mut self, command: Command) -> Result<(), String>;
    /// Returns the next queued notification without waiting.
    fn poll(&mut self) -> Option<Event>;
    /// Returns the retained page view, with no engine work during painting.
    fn page(&self, tab: TabId) -> Option<Rc<dyn View>>;
    /// Schedules event draining only when the adapter needs another wakeup.
    /// A worker-backed adapter must arrange a deadline or platform wakeup when
    /// events become ready; None must not strand pending engine work.
    fn next_poll_at(&self) -> Option<Instant>;
}

/// Honest default backend: it neither fetches data nor fabricates documents.
pub(crate) struct Unavailable;
impl Engine for Unavailable {
    fn available(&self) -> bool {
        false
    }
    fn send(&mut self, _: Command) -> Result<(), String> {
        Err("The browsing engine is not connected. This page has not been loaded.".into())
    }
    fn poll(&mut self) -> Option<Event> {
        None
    }
    fn page(&self, _: TabId) -> Option<Rc<dyn View>> {
        None
    }
    fn next_poll_at(&self) -> Option<Instant> {
        None
    }
}
