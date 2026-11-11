//! Pure browser session state. No UI callbacks or network operations.
use crate::engine::{Command, Event, NavigationId, Security, TabId};
use serde::{Deserialize, Serialize};
use url::Url;

const MAX_TABS: usize = 64;
const MAX_HISTORY: usize = 200;
const MAX_CLOSED: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Entry {
    pub url: String,
    pub title: String,
}
pub(crate) struct Tab {
    pub id: TabId,
    pub draft: String,
    pub document: Option<Entry>,
    pub navigation: NavigationId,
    document_navigation: NavigationId,
    pub loading: bool,
    pub progress: f32,
    pub error: Option<String>,
    pub can_back: bool,
    pub can_forward: bool,
    pub security: Security,
    pub pinned: bool,
    pub group: String,
    pub zoom: u16,
    pub find: Option<(u64, usize, usize)>,
    pub find_pending: bool,
    pub restore_pending: bool,
}
impl Tab {
    fn new(id: TabId) -> Self {
        Self {
            id,
            draft: String::new(),
            document: None,
            navigation: 0,
            document_navigation: 0,
            loading: false,
            progress: 0.,
            error: None,
            can_back: false,
            can_forward: false,
            security: Security::Unknown,
            pinned: false,
            group: "Default".into(),
            zoom: 100,
            find: None,
            find_pending: false,
            restore_pending: false,
        }
    }
    /// Returns the current engine title or a concise native tab placeholder.
    pub fn title(&self) -> &str {
        self.document
            .as_ref()
            .map(|d| d.title.as_str())
            .filter(|s| !s.is_empty())
            .unwrap_or(if self.error.is_some() {
                "Page Unavailable"
            } else {
                "New Tab"
            })
    }
}

/// A transfer offered by the adapter; no filesystem path is trusted from the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DownloadState {
    Offered,
    Active,
    Finished,
    Failed(String),
    Cancelled,
}
#[derive(Clone, Debug)]
pub(crate) struct Download {
    pub id: u64,
    pub url: String,
    pub name: String,
    pub total: Option<u64>,
    pub received: u64,
    pub state: DownloadState,
}
/// Search is opt-in; no provider is silently selected and no suggestions are sent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum SearchProvider {
    #[default]
    None,
    Google,
    DuckDuckGo,
    Bing,
}
impl SearchProvider {
    /// User-visible provider name.
    pub fn label(self) -> &'static str {
        match self {
            Self::None => "Address only",
            Self::Google => "Google",
            Self::DuckDuckGo => "DuckDuckGo",
            Self::Bing => "Bing",
        }
    }
    /// Constructs a properly escaped search URL without making a network request.
    pub fn query(self, input: &str) -> Result<String, String> {
        let (base, key) = match self {
            Self::None => {
                return Err(
                    "Choose a search provider in Browser Settings to search the web.".into(),
                );
            }
            Self::Google => ("https://www.google.com/search", "q"),
            Self::DuckDuckGo => ("https://duckduckgo.com/", "q"),
            Self::Bing => ("https://www.bing.com/search", "q"),
        };
        let mut url = Url::parse(base).map_err(|_| "Invalid search provider")?;
        url.query_pairs_mut().append_pair(key, input);
        Ok(url.into())
    }
}

pub(crate) struct Browser {
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub bookmarks: Vec<Entry>,
    pub history: Vec<Entry>,
    pub reading_list: Vec<Entry>,
    pub groups: Vec<String>,
    pub search: SearchProvider,
    pub restore_session: bool,
    pub downloads: Vec<Download>,
    closed: Vec<String>,
    next_tab: u64,
    next_navigation: u64,
}
impl Browser {
    /// Creates a local session with one blank tab; no engine success is assumed.
    pub fn new() -> Self {
        Self {
            tabs: vec![Tab::new(1)],
            active: 0,
            bookmarks: vec![],
            history: vec![],
            reading_list: vec![],
            groups: vec!["Default".into()],
            search: SearchProvider::None,
            restore_session: true,
            downloads: vec![],
            closed: vec![],
            next_tab: 2,
            next_navigation: 1,
        }
    }
    /// Returns the active tab. Closing the last tab replaces it with a blank one.
    pub fn tab(&self) -> &Tab {
        &self.tabs[self.active]
    }
    /// Creates a fresh stable tab identity; callers must create/activate its engine session.
    pub fn new_tab(&mut self) -> Result<TabId, String> {
        if self.tabs.len() >= MAX_TABS {
            return Err("Close a tab before opening another.".into());
        }
        let id = self.next_tab;
        self.next_tab = self
            .next_tab
            .checked_add(1)
            .ok_or("Tab identities are exhausted.")?;
        let mut tab = Tab::new(id);
        tab.group = self.tab().group.clone();
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        Ok(id)
    }
    /// Closes a tab without accidentally selecting a different index after removal.
    pub fn close(&mut self, id: TabId) -> Result<(), String> {
        let Some(index) = self.tabs.iter().position(|t| t.id == id) else {
            return Ok(());
        };
        if self.tabs.len() == 1 {
            self.new_tab()?;
        }
        let active_id = self.tab().id;
        let removed = self.tabs.remove(index);
        if let Some(url) = removed
            .document
            .map(|d| d.url)
            .or_else(|| normalize_address(&removed.draft).ok())
        {
            self.closed.push(url);
            if self.closed.len() > MAX_CLOSED {
                self.closed.remove(0);
            }
        }
        self.active = self
            .tabs
            .iter()
            .position(|t| t.id == active_id)
            .unwrap_or(index.min(self.tabs.len() - 1));
        Ok(())
    }
    /// Restores the last closed valid address using a fresh session identity.
    pub fn reopen(&mut self) -> Result<Option<String>, String> {
        if self.closed.is_empty() {
            return Ok(None);
        }
        self.new_tab()?;
        Ok(self.closed.pop())
    }
    /// Whether a closed web address is available to reopen.
    pub fn can_reopen(&self) -> bool {
        !self.closed.is_empty()
    }
    /// Resolves explicit web addresses before optionally using the selected search provider.
    pub fn resolve_input(&self, input: &str) -> Result<String, String> {
        let text = input.trim();
        let address_like = text.contains('.')
            || text.contains(':')
            || text.contains('/')
            || text == "localhost"
            || text.starts_with('[');
        if address_like && !text.chars().any(char::is_whitespace) {
            return normalize_address(text);
        }
        if text.is_empty()
            || text.len() > 8192
            || text.chars().any(char::is_control)
            || text.contains("://")
            || text.contains(':')
            || text.starts_with("javascript:")
            || text.starts_with("data:")
            || text.starts_with("file:")
        {
            return Err("Enter a valid HTTP or HTTPS address or search query.".into());
        }
        self.search.query(text)
    }
    /// Returns visible tab indices in the active group, preserving their stable identities.
    pub fn visible_tabs(&self) -> Vec<usize> {
        self.tabs
            .iter()
            .enumerate()
            .filter(|(_, t)| t.group == self.tab().group)
            .map(|(i, _)| i)
            .collect()
    }
    /// Creates a named local tab group with its own blank tab.
    pub fn create_group(&mut self, name: &str) -> Result<TabId, String> {
        let name = name.trim();
        if name.is_empty()
            || name.chars().count() > 64
            || name.chars().any(char::is_control)
            || self.groups.iter().any(|g| g.eq_ignore_ascii_case(name))
            || self.groups.len() >= 16
        {
            return Err(
                "Choose a unique tab group name of 1–64 characters (up to 16 groups).".into(),
            );
        }
        let id = self.new_tab()?;
        self.tabs[self.active].group = name.into();
        self.groups.push(name.into());
        Ok(id)
    }
    /// Duplicates the active address in a fresh uncommitted session.
    pub fn duplicate(&mut self) -> Result<TabId, String> {
        let url = self
            .tab()
            .document
            .as_ref()
            .map(|e| e.url.clone())
            .unwrap_or_else(|| self.tab().draft.clone());
        let zoom = self.tab().zoom;
        let id = self.new_tab()?;
        self.tabs[self.active].draft = url;
        self.tabs[self.active].zoom = zoom;
        Ok(id)
    }
    /// Pins or unpins the active tab and keeps pinned tabs before ordinary tabs.
    pub fn toggle_pin(&mut self) {
        let mut tab = self.tabs.remove(self.active);
        tab.pinned = !tab.pinned;
        let index = self.tabs.iter().take_while(|t| t.pinned).count();
        self.tabs.insert(index, tab);
        self.active = index;
    }
    /// Moves within the same pin category, preserving active session identity.
    pub fn move_tab(&mut self, delta: i8) {
        let visible = self.visible_tabs();
        let current = visible.iter().position(|i| *i == self.active).unwrap_or(0);
        let next = current as isize + delta as isize;
        if next < 0 || next as usize >= visible.len() {
            return;
        }
        let next = visible[next as usize];
        if self.tabs[next].pinned != self.tab().pinned {
            return;
        }
        self.tabs.swap(self.active, next);
        self.active = next;
    }
    /// Returns closable peers, excluding pinned tabs and the active tab.
    pub fn peer_tabs(&self, right_only: bool) -> Vec<TabId> {
        self.tabs
            .iter()
            .enumerate()
            .filter(|(i, t)| {
                *i != self.active
                    && !t.pinned
                    && t.group == self.tab().group
                    && (!right_only || *i > self.active)
            })
            .map(|(_, t)| t.id)
            .collect()
    }
    /// Saves an explicitly supplied web address without pretending it was visited.
    pub fn add_bookmark(&mut self, title: &str, input: &str) -> Result<(), String> {
        let url = normalize_address(input)?;
        let title: String = title
            .chars()
            .filter(|c| !c.is_control())
            .take(512)
            .collect();
        let title = if title.trim().is_empty() {
            url.clone()
        } else {
            title
        };
        if let Some(entry) = self.bookmarks.iter_mut().find(|e| e.url == url) {
            entry.title = title;
        } else {
            if self.bookmarks.len() >= 1000 {
                return Err("The bookmark limit has been reached.".into());
            }
            self.bookmarks.push(Entry { url, title });
        }
        Ok(())
    }
    /// Adds/removes the committed page in the reading list without downloading it.
    pub fn toggle_reading(&mut self) {
        if let Some(entry) = self.tab().document.clone() {
            if let Some(i) = self.reading_list.iter().position(|e| e.url == entry.url) {
                self.reading_list.remove(i);
            } else if self.reading_list.len() < 1000 {
                self.reading_list.push(entry);
            }
        }
    }
    /// Rejects messages from documents that have been replaced or whose tab closed.
    pub fn has_document(&self, tab: TabId, document: NavigationId) -> bool {
        document != 0
            && self
                .tabs
                .iter()
                .any(|t| t.id == tab && t.document_navigation == document)
    }
    /// The current committed document generation; zero means no usable document.
    pub fn document_id(&self) -> NavigationId {
        self.tab().document_navigation
    }
    /// Starts a new generation without committing an unvisited page to history.
    pub fn navigate(&mut self, input: &str) -> Result<Command, String> {
        let url = self.resolve_input(input)?;
        let (tab, navigation) = self.begin()?;
        self.tabs[self.active].draft = url.clone();
        Ok(Command::Navigate {
            tab,
            navigation,
            url,
        })
    }
    fn begin(&mut self) -> Result<(TabId, NavigationId), String> {
        let navigation = self.next_navigation;
        self.next_navigation = self
            .next_navigation
            .checked_add(1)
            .ok_or("Navigation identities are exhausted.")?;
        let t = &mut self.tabs[self.active];
        t.navigation = navigation;
        t.loading = true;
        t.progress = 0.;
        t.error = None;
        t.find = None;
        t.find_pending = false;
        t.restore_pending = false;
        t.security = Security::Unknown;
        Ok((t.id, navigation))
    }
    /// Issues history/reload commands only when a corresponding engine state exists.
    pub fn travel(&mut self, delta: i8) -> Result<Option<Command>, String> {
        if (delta < 0 && !self.tab().can_back) || (delta > 0 && !self.tab().can_forward) {
            return Ok(None);
        }
        if delta == 0 && self.tab().draft.is_empty() {
            return Ok(None);
        }
        let (tab, navigation) = self.begin()?;
        Ok(Some(if delta < 0 {
            Command::Back { tab, navigation }
        } else if delta > 0 {
            Command::Forward { tab, navigation }
        } else {
            Command::Reload { tab, navigation }
        }))
    }
    /// Invalidates late notifications after Stop while retaining the visible document.
    pub fn stop(&mut self) -> Option<Command> {
        let t = &mut self.tabs[self.active];
        if !t.loading {
            return None;
        }
        let command = Command::Stop {
            tab: t.id,
            navigation: t.navigation,
        };
        t.loading = false;
        t.progress = 0.;
        t.navigation = 0;
        Some(command)
    }
    /// Applies only live, current-generation events. No closed tab can be resurrected.
    pub fn apply(&mut self, event: Event) {
        match &event {
            Event::FindResult {
                tab,
                document,
                request,
                current,
                total,
            } => {
                if *current <= *total {
                    if let Some(t) = self.tabs.iter_mut().find(|t| {
                        t.id == *tab
                            && t.document_navigation == *document
                            && *document != 0
                            && t.find.is_some_and(|f| f.0 == *request)
                    }) {
                        t.find = Some((*request, *current, *total));
                        t.find_pending = false;
                    }
                }
                return;
            }
            Event::DownloadOffered {
                id,
                tab,
                document,
                url,
                name,
                total,
            } => {
                if self.downloads.len() < 100
                    && !self.downloads.iter().any(|d| d.id == *id)
                    && self.tabs.iter().any(|t| {
                        t.id == *tab && t.document_navigation == *document && *document != 0
                    })
                    && normalize_address(url).is_ok()
                {
                    let name: String = name
                        .chars()
                        .filter(|c| !c.is_control() && !matches!(c, '/' | '\\'))
                        .take(255)
                        .collect();
                    self.downloads.push(Download {
                        id: *id,
                        url: url.clone(),
                        name: if name.is_empty() {
                            "Download".into()
                        } else {
                            name
                        },
                        total: *total,
                        received: 0,
                        state: DownloadState::Offered,
                    });
                }
                return;
            }
            Event::DownloadProgress { id, received } => {
                if let Some(d) = self
                    .downloads
                    .iter_mut()
                    .find(|d| d.id == *id && d.state == DownloadState::Active)
                {
                    if *received >= d.received && d.total.is_none_or(|total| *received <= total) {
                        d.received = *received;
                    }
                }
                return;
            }
            Event::DownloadFinished { id } => {
                if let Some(d) = self
                    .downloads
                    .iter_mut()
                    .find(|d| d.id == *id && d.state == DownloadState::Active)
                {
                    d.state = DownloadState::Finished;
                }
                return;
            }
            Event::DownloadFailed { id, message } => {
                if let Some(d) = self
                    .downloads
                    .iter_mut()
                    .find(|d| d.id == *id && d.state == DownloadState::Active)
                {
                    d.state = DownloadState::Failed(
                        message
                            .chars()
                            .filter(|c| !c.is_control())
                            .take(2048)
                            .collect(),
                    );
                }
                return;
            }
            _ => {}
        }
        if let Event::TitleChanged {
            tab,
            document,
            title,
        } = &event
        {
            if let Some(t) = self
                .tabs
                .iter_mut()
                .find(|t| t.id == *tab && t.document_navigation == *document && *document != 0)
            {
                if let Some(entry) = &mut t.document {
                    entry.title = title
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(512)
                        .collect();
                }
            }
            return;
        }
        let (id, generation) = match &event {
            Event::Progress {
                tab, navigation, ..
            }
            | Event::Committed {
                tab, navigation, ..
            }
            | Event::Finished { tab, navigation }
            | Event::Failed {
                tab, navigation, ..
            } => (*tab, *navigation),
            Event::TitleChanged { .. } => return,
            _ => return,
        };
        let Some(t) = self
            .tabs
            .iter_mut()
            .find(|t| t.id == id && t.navigation == generation && generation != 0)
        else {
            return;
        };
        match event {
            Event::TitleChanged { .. } => {}
            Event::Progress { fraction, .. } => {
                if t.loading && fraction.is_finite() {
                    t.progress = fraction.clamp(0., 1.)
                }
            }
            Event::Committed {
                url,
                title,
                security,
                can_back,
                can_forward,
                ..
            } => {
                let Ok(url) = normalize_address(&url) else {
                    t.loading = false;
                    t.navigation = 0;
                    t.error = Some("The engine returned an unsupported page address.".into());
                    t.security = Security::Unknown;
                    return;
                };
                let entry = Entry {
                    title: if title.trim().is_empty() {
                        url.clone()
                    } else {
                        title
                            .chars()
                            .filter(|c| !c.is_control())
                            .take(512)
                            .collect()
                    },
                    url,
                };
                t.draft = entry.url.clone();
                t.document = Some(entry.clone());
                t.document_navigation = generation;
                t.security = security;
                t.can_back = can_back;
                t.can_forward = can_forward;
                t.error = None;
                self.history.retain(|e| e.url != entry.url);
                self.history.insert(0, entry);
                self.history.truncate(MAX_HISTORY);
            }
            Event::Finished { .. } => {
                if t.document_navigation != generation {
                    t.error = Some("The engine completed without committing a page.".into());
                    t.security = Security::Unknown;
                }
                t.navigation = 0;
                t.loading = false;
                t.progress = 1.;
            }
            Event::Failed { message, .. } => {
                t.navigation = 0;
                t.loading = false;
                t.progress = 0.;
                t.error = Some(message.chars().take(2048).collect());
                t.security = Security::Unknown;
            }
            _ => {}
        }
    }
    /// Marks a rejected request as failed without fabricating a commit.
    pub fn fail_current(&mut self, message: String) {
        let t = &mut self.tabs[self.active];
        t.error = Some(message.chars().take(2048).collect());
        t.navigation = 0;
        t.loading = false;
        t.security = Security::Unknown;
    }
    /// Toggles a bookmark only for an engine-committed page.
    pub fn bookmark(&mut self) {
        let Some(entry) = self.tab().document.clone() else {
            return;
        };
        if let Some(i) = self.bookmarks.iter().position(|e| e.url == entry.url) {
            self.bookmarks.remove(i);
        } else {
            self.bookmarks.push(entry);
        }
    }
}

/// Normalizes HTTP(S) URLs; never accepts script/file URLs or embedded credentials.
/// Search is deliberately not invented without a configured provider.
pub(crate) fn normalize_address(input: &str) -> Result<String, String> {
    let input = input.trim();
    if input == "terra:///engine-check" {
        return Ok(input.into());
    }
    if input.is_empty()
        || input.len() > 8192
        || input.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err("Enter a website address, such as example.org.".into());
    }
    let candidate = if input.contains("://") {
        input.to_owned()
    } else if input.contains(':') && !input.starts_with("localhost:") && !input.starts_with('[') {
        // Permit host:port; reject non-web schemes instead of interpreting them as hosts.
        if input
            .split_once(':')
            .is_some_and(|(_, p)| p.split('/').next().unwrap_or("").parse::<u16>().is_ok())
        {
            format!("https://{input}")
        } else {
            return Err("Only HTTP and HTTPS website addresses are supported.".into());
        }
    } else {
        format!("https://{input}")
    };
    let url = Url::parse(&candidate).map_err(|_| "Enter a valid website address.".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("Only HTTP and HTTPS website addresses are supported.".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("Remove the username and password from the website address.".into());
    }
    Ok(url.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(b: &mut Browser, url: &str) -> (TabId, NavigationId) {
        let Command::Navigate {
            tab, navigation, ..
        } = b.navigate(url).unwrap()
        else {
            panic!("navigate")
        };
        (tab, navigation)
    }
    fn commit(tab: TabId, navigation: NavigationId, url: &str) -> Event {
        Event::Committed {
            tab,
            navigation,
            url: url.into(),
            title: "Example".into(),
            security: Security::Secure,
            can_back: true,
            can_forward: false,
        }
    }
    #[test]
    fn tab_groups_pins_moves_and_peer_closure_preserve_identity() {
        let mut b = Browser::new();
        let first = b.tab().id;
        b.new_tab().unwrap();
        let second = b.tab().id;
        b.toggle_pin();
        assert_eq!(b.tab().id, second);
        assert!(b.tab().pinned);
        assert_eq!(b.active, 0);
        let work = b.create_group("Work").unwrap();
        b.new_tab().unwrap();
        let work2 = b.tab().id;
        assert_eq!(b.visible_tabs().len(), 2);
        assert!(b.create_group("work").is_err());
        b.move_tab(-1);
        assert_eq!(b.tab().id, work2);
        assert_eq!(b.peer_tabs(false), vec![work]);
        for id in b.peer_tabs(false) {
            b.close(id).unwrap();
        }
        assert_eq!(b.tab().id, work2);
        assert!(b.tabs.iter().any(|t| t.id == first));
        assert!(b.tabs.iter().any(|t| t.id == second));
        b.tabs[b.active].draft = "https://example.org/".into();
        let duplicate = b.duplicate().unwrap();
        assert_ne!(duplicate, work2);
        assert_eq!(b.tab().group, "Work");
        assert_eq!(b.tab().draft, "https://example.org/");
        assert!(b.tab().document.is_none());
    }
    #[test]
    fn searches_are_opt_in_and_escape_queries_instead_of_accepting_unsafe_schemes() {
        let mut b = Browser::new();
        assert!(b.resolve_input("mochi desktop").is_err());
        b.search = SearchProvider::DuckDuckGo;
        let url = Url::parse(&b.resolve_input("mochi & desktop 日本語").unwrap()).unwrap();
        assert_eq!(url.host_str(), Some("duckduckgo.com"));
        assert_eq!(
            url.query_pairs().next().unwrap().1,
            "mochi & desktop 日本語"
        );
        assert_eq!(
            b.resolve_input("example.org").unwrap(),
            "https://example.org/"
        );
        for input in [
            "javascript:alert(1)",
            "file:///etc/passwd",
            "https://user:pass@example.org",
            "data:text/html,a",
        ] {
            assert!(b.resolve_input(input).is_err());
        }
        assert!(
            b.resolve_input("singleword")
                .unwrap()
                .contains("q=singleword")
        );
    }
    #[test]
    fn bookmarks_can_be_added_and_renamed_without_visiting_a_page() {
        let mut b = Browser::new();
        b.add_bookmark("Docs", "example.org").unwrap();
        b.add_bookmark("Reference", "https://example.org/").unwrap();
        assert_eq!(b.bookmarks.len(), 1);
        assert_eq!(b.bookmarks[0].title, "Reference");
        assert!(b.history.is_empty());
        assert!(b.tab().document.is_none());
        assert!(b.add_bookmark("Bad", "file:///x").is_err());
    }
    #[test]
    fn find_results_and_downloads_reject_stale_or_unaccepted_work() {
        let mut b = Browser::new();
        let (tab, document) = request(&mut b, "example.org");
        b.apply(commit(tab, document, "https://example.org/"));
        b.tabs[0].find = Some((7, 0, 0));
        b.tabs[0].find_pending = true;
        b.apply(Event::FindResult {
            tab,
            document,
            request: 6,
            current: 1,
            total: 3,
        });
        assert!(b.tab().find_pending);
        b.apply(Event::FindResult {
            tab,
            document,
            request: 7,
            current: 2,
            total: 3,
        });
        assert_eq!(b.tab().find, Some((7, 2, 3)));
        assert!(!b.tab().find_pending);
        b.apply(Event::DownloadOffered {
            id: 1,
            tab,
            document: document + 1,
            url: "https://example.org/file".into(),
            name: "file".into(),
            total: Some(20),
        });
        assert!(b.downloads.is_empty());
        b.apply(Event::DownloadOffered {
            id: 1,
            tab,
            document,
            url: "https://example.org/file".into(),
            name: "../file".into(),
            total: Some(20),
        });
        assert_eq!(b.downloads.len(), 1);
        assert!(!b.downloads[0].name.contains('/'));
        b.apply(Event::DownloadFinished { id: 1 });
        assert_eq!(b.downloads[0].state, DownloadState::Offered);
        b.downloads[0].state = DownloadState::Active;
        b.apply(Event::DownloadProgress {
            id: 1,
            received: 10,
        });
        b.apply(Event::DownloadProgress { id: 1, received: 5 });
        assert_eq!(b.downloads[0].received, 10);
        b.downloads[0].state = DownloadState::Cancelled;
        b.apply(Event::DownloadFinished { id: 1 });
        b.apply(Event::DownloadProgress {
            id: 1,
            received: 20,
        });
        assert_eq!(b.downloads[0].state, DownloadState::Cancelled);
        request(&mut b, "new.example");
        b.apply(Event::FindResult {
            tab,
            document,
            request: 7,
            current: 3,
            total: 3,
        });
        assert!(b.tab().find.is_none());
    }
    #[test]
    fn addresses_are_normalized_without_accepting_script_files_credentials_or_search_text() {
        assert_eq!(
            normalize_address(" example.org ").unwrap(),
            "https://example.org/"
        );
        assert_eq!(
            normalize_address("localhost:8080/a").unwrap(),
            "https://localhost:8080/a"
        );
        assert!(
            normalize_address("https://例え.jp")
                .unwrap()
                .contains("xn--")
        );
        for s in [
            "",
            "search words",
            "javascript:alert(1)",
            "data:text/html,a",
            "file:///etc/passwd",
            "https://user:pass@example.org",
            "https://example.org:99999",
            "https://example.org\0",
            "https://",
        ] {
            assert!(normalize_address(s).is_err(), "{s}");
        }
    }
    #[test]
    fn failed_uncommitted_navigation_never_creates_history_or_security_claims() {
        let mut b = Browser::new();
        request(&mut b, "example.org");
        assert!(b.history.is_empty());
        assert_eq!(b.tab().security, Security::Unknown);
        b.fail_current("unavailable".into());
        assert!(!b.tab().loading);
        assert!(b.tab().document.is_none());
        assert!(b.history.is_empty());
    }
    #[test]
    fn closed_tabs_and_old_generations_cannot_receive_new_documents() {
        let mut b = Browser::new();
        let (id, old) = request(&mut b, "old.example");
        let (_, new) = request(&mut b, "new.example");
        b.apply(commit(id, old, "https://old.example/"));
        assert!(b.tab().document.is_none());
        b.apply(commit(id, new, "https://new.example/"));
        assert_eq!(
            b.tab().document.as_ref().unwrap().url,
            "https://new.example/"
        );
        b.close(id).unwrap();
        assert_ne!(b.tab().id, id);
        b.apply(commit(id, new, "https://old.example/"));
        assert!(b.tab().document.is_none());
        assert_eq!(b.tabs.len(), 1);
    }
    #[test]
    fn stopping_or_failing_invalidates_late_success_and_finishing_allows_document_title_updates() {
        let mut b = Browser::new();
        let (id, n) = request(&mut b, "example.org");
        b.stop();
        b.apply(commit(id, n, "https://example.org/"));
        assert!(b.tab().document.is_none());
        let (_, n) = request(&mut b, "example.org");
        b.apply(Event::Failed {
            tab: id,
            navigation: n,
            message: "failed".into(),
        });
        b.apply(commit(id, n, "https://example.org/"));
        assert!(b.tab().document.is_none());
        let (_, n) = request(&mut b, "example.org");
        b.apply(commit(id, n, "https://example.org/"));
        b.apply(Event::Finished {
            tab: id,
            navigation: n,
        });
        b.apply(Event::Failed {
            tab: id,
            navigation: n,
            message: "late failure".into(),
        });
        assert!(b.tab().error.is_none());
        b.apply(Event::TitleChanged {
            tab: id,
            document: n,
            title: "Updated title".into(),
        });
        assert_eq!(b.tab().title(), "Updated title");
    }
    #[test]
    fn tab_closing_preserves_selection_and_reopen_uses_a_new_identity() {
        let mut b = Browser::new();
        let (id, n) = request(&mut b, "example.org");
        b.apply(commit(id, n, "https://example.org/"));
        let second = b.new_tab().unwrap();
        let third = b.new_tab().unwrap();
        b.close(second).unwrap();
        assert_eq!(b.tab().id, third);
        b.close(id).unwrap();
        assert!(b.can_reopen());
        let url = b.reopen().unwrap().unwrap();
        assert_eq!(url, "https://example.org/");
        assert_ne!(b.tab().id, id);
        assert!(!b.can_reopen());
    }
    #[test]
    fn bookmark_and_history_use_only_committed_pages_and_progress_is_finite() {
        let mut b = Browser::new();
        b.bookmark();
        assert!(b.bookmarks.is_empty());
        let (id, n) = request(&mut b, "example.org");
        b.apply(Event::Progress {
            tab: id,
            navigation: n,
            fraction: f32::NAN,
        });
        assert_eq!(b.tab().progress, 0.);
        b.apply(Event::Progress {
            tab: id,
            navigation: n,
            fraction: 2.,
        });
        assert_eq!(b.tab().progress, 1.);
        b.apply(commit(id, n, "https://example.org/"));
        b.bookmark();
        assert_eq!(b.bookmarks.len(), 1);
        b.bookmark();
        assert!(b.bookmarks.is_empty());
        b.apply(commit(id, n, "https://example.org/"));
        assert_eq!(b.history.len(), 1);
        assert!(matches!(b.travel(-1).unwrap(), Some(Command::Back { .. })));
        assert!(b.travel(1).unwrap().is_none());
    }
    #[test]
    fn unsupported_engine_commits_and_finish_without_commit_are_failures() {
        let mut b = Browser::new();
        let (id, n) = request(&mut b, "example.org");
        b.apply(commit(id, n, "file:///private"));
        assert!(b.tab().error.is_some());
        assert!(b.history.is_empty());
        let (_, n) = request(&mut b, "example.org");
        b.apply(Event::Finished {
            tab: id,
            navigation: n,
        });
        assert!(b.tab().error.is_some());
        assert!(b.tab().document.is_none());
        assert!(b.history.is_empty());
    }
    #[test]
    fn session_storage_is_bounded() {
        let mut b = Browser::new();
        for _ in 1..MAX_TABS {
            b.new_tab().unwrap();
        }
        assert!(b.new_tab().is_err());
        for i in 0..MAX_HISTORY + 3 {
            let (id, n) = request(&mut b, &format!("https://example.org/{i}"));
            b.apply(commit(id, n, &format!("https://example.org/{i}")));
        }
        assert_eq!(b.history.len(), MAX_HISTORY);
    }
}
