//! Retained native browser chrome; page rendering belongs to the engine adapter.
mod library;
use crate::{
    browser::{Browser, DownloadState, Entry, SearchProvider},
    engine::{Command, Engine, Security, TabId},
};
use appcore::prelude::*;
use appcore::viewkit::{
    event::{EventContext, EventResult, ViewEvent},
    platform::PointerButton,
    view::{Constraints, MeasureContext, PaintContext},
};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, VecDeque},
    rc::Rc,
};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Action {
    New,
    Close(TabId),
    Reopen,
    Navigate,
    EditAddress,
    Open(String),
    Back,
    Forward,
    Reload,
    Stop,
    Bookmark,
    Bookmarks,
    History,
    ClosePanel,
    ClearHistory,
    RemoveBookmark(String),
    Duplicate,
    Pin,
    Move(i8),
    ClosePeers(bool),
    SelectTab(TabId),
    ReadingList,
    ToggleReading,
    RemoveReading(String),
    Tabs,
    Downloads,
    Settings,
    AddBookmark,
    EditBookmark(String),
    SaveBookmark,
    SearchProvider(u8),
    ToggleRestore,
    Find,
    FindNext(bool),
    CloseFind,
    Reader,
    Print,
    Zoom(i16),
    AcceptDownload(u64),
    CancelDownload(u64),
    ClearDownloads,
    NewGroup,
    CreateGroup,
    SelectGroup(String),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Panel {
    None,
    Bookmarks,
    History,
    ReadingList,
    Tabs,
    Downloads,
    Settings,
    BookmarkEditor,
    GroupEditor,
}
type Pending = Rc<RefCell<VecDeque<Action>>>;
fn enqueue(pending: &Pending, action: Action) {
    pending.borrow_mut().push_back(action);
}

struct Core {
    browser: RefCell<Browser>,
    engine: RefCell<Box<dyn Engine>>,
    pending: Pending,
    selected: State<usize>,
    tabs: TabsInteractionState,
    tab_scroll: ScrollState,
    address: Rc<TextField>,
    address_state: TextFieldInteractionState,
    focus_address: Cell<bool>,
    editing_address: Cell<bool>,
    address_bounds: Cell<Option<Rect>>,
    address_ready: Cell<Option<Rect>>,
    buttons: RefCell<BTreeMap<Action, ButtonInteractionState>>,
    icons: RefCell<BTreeMap<Action, IconButtonInteractionState>>,
    rows: RefCell<BTreeMap<String, ButtonInteractionState>>,
    menu: RefCell<Option<(String, Rc<MenuButton>)>>,
    tab_menu: RefCell<Option<(String, Rc<MenuButton>)>>,
    tab_context: RefCell<Option<(u64, TabId, Vec<Option<Action>>)>>,
    last_reveal: Cell<Option<(TabId, u32, usize, bool)>>,
    panel: Cell<Panel>,
    panel_scroll: ScrollState,
    suggestions_scroll: ScrollState,
    panel_toolbar: ActionToolbarState<Action>,
    feedback: RefCell<Option<String>>,
    storage_error: RefCell<Option<String>>,
    library_query: TextFieldInteractionState,
    bookmark_title: TextFieldInteractionState,
    bookmark_url: TextFieldInteractionState,
    bookmark_original: RefCell<Option<String>>,
    find_text: TextFieldInteractionState,
    finding: Cell<bool>,
    next_find: Cell<u64>,
    find_focus: Cell<bool>,
    find_ready: Cell<Option<Rect>>,
    store: RefCell<Option<crate::store::Store>>,
    save_at: Cell<Option<std::time::Instant>>,
}
impl Core {
    fn button(&self, label: &str, symbol: Option<SymbolName>, action: Action) -> Button {
        let interaction = self
            .buttons
            .borrow_mut()
            .entry(action.clone())
            .or_default()
            .clone();
        let pending = self.pending.clone();
        let mut button = Button::with_interaction_and_label(interaction, label)
            .size(ButtonSize::Small)
            .style(ButtonStyle::Ghost)
            .on_click(move || enqueue(&pending, action.clone()));
        if let Some(symbol) = symbol {
            button = button.leading_symbol(symbol);
        }
        button
    }
    fn icon(&self, label: &str, symbol: SymbolName, action: Action) -> Button {
        self.button("", Some(symbol), action)
            .accessibility_label(label)
    }
    fn navigation_icon(&self, label: &str, symbol: SymbolName, action: Action) -> IconButton {
        let interaction = self
            .icons
            .borrow_mut()
            .entry(action.clone())
            .or_default()
            .clone();
        let pending = self.pending.clone();
        IconButton::with_interaction(symbol, interaction)
            .size(ButtonSize::Small)
            .accessibility_label(label)
            .on_click(move || enqueue(&pending, action.clone()))
    }
    fn more(&self) -> Rc<MenuButton> {
        let b = self.browser.borrow();
        let features = self.engine.borrow().features();
        let connected = self.engine.borrow().available();
        let document = b.document_id() != 0;
        let key = format!(
            "{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}:{}",
            b.can_reopen(),
            b.tab().id,
            b.tab().pinned,
            b.tabs.len(),
            b.active,
            b.tab().zoom,
            connected,
            features.find,
            features.zoom,
            document,
            features.reader,
            features.printing
        );
        if let Some((old, menu)) = self.menu.borrow().as_ref() {
            if old == &key {
                return menu.clone();
            }
        }
        let mut menu = MenuButton::new("More", SymbolName::More);
        for (label, action, enabled) in [
            ("New Tab", Action::New, true),
            ("Reopen Closed Tab", Action::Reopen, b.can_reopen()),
            ("Show All Tabs", Action::Tabs, true),
            ("Bookmarks", Action::Bookmarks, true),
            ("Add Bookmark…", Action::AddBookmark, true),
            ("Reading List", Action::ReadingList, true),
            ("History", Action::History, true),
            ("Downloads", Action::Downloads, true),
            (
                "Find on Page…",
                Action::Find,
                connected && features.find && document,
            ),
            (
                "Zoom In",
                Action::Zoom(10),
                connected && features.zoom && document && b.tab().zoom < 300,
            ),
            (
                "Zoom Out",
                Action::Zoom(-10),
                connected && features.zoom && document && b.tab().zoom > 50,
            ),
            (
                "Actual Size",
                Action::Zoom(0),
                connected && features.zoom && document,
            ),
            (
                "Show / Hide Reader",
                Action::Reader,
                connected && features.reader && document,
            ),
            (
                "Print…",
                Action::Print,
                connected && features.printing && document,
            ),
            ("Browser Settings…", Action::Settings, true),
        ] {
            if !enabled {
                menu = menu.disabled_option(label);
            } else {
                let pending = self.pending.clone();
                menu = menu.option(label, move || enqueue(&pending, action.clone()));
            }
        }
        let menu = Rc::new(menu);
        *self.menu.borrow_mut() = Some((key, menu.clone()));
        menu
    }
    fn schedule_save(&self) {
        if self.store.borrow().is_some() && self.save_at.get().is_none() {
            self.save_at.set(Some(
                std::time::Instant::now() + std::time::Duration::from_millis(500),
            ));
        }
    }
    fn persist(&self) {
        self.save_at.set(None);
        if let Some(store) = self.store.borrow_mut().as_mut() {
            match store.save(&self.browser.borrow()) {
                Ok(()) => {
                    self.storage_error.borrow_mut().take();
                }
                Err(error) => {
                    *self.storage_error.borrow_mut() = Some(format!("Changes not saved: {error}"))
                }
            }
        }
    }
    fn sync_address(&self) {
        let b = self.browser.borrow();
        self.selected.set_if_changed(b.active);
        self.address_state.set_value(b.tab().draft.clone());
    }
    fn focus_address(&self) {
        self.editing_address.set(true);
        self.address_bounds.set(None);
        self.focus_address.set(true);
        self.address_ready.set(None);
        self.address_state.set_focused(true);
        self.address_state
            .set_selection(0..self.address_state.value().len());
    }
    fn send(&self, command: Command) {
        if let Err(error) = self.engine.borrow_mut().send(command) {
            self.browser.borrow_mut().fail_current(error);
        }
    }
    fn lifecycle(&self, command: Command) {
        if self.engine.borrow().available() {
            self.send(command);
        }
    }
    fn poll(&self, context: &mut PaintContext<'_>) {
        for n in 0..64 {
            let event = self.engine.borrow_mut().poll();
            let Some(event) = event else { break };
            if let crate::engine::Event::OpenTabRequested {
                source,
                document,
                url,
                background,
                user_initiated,
            } = event
            {
                if self.browser.borrow().has_document(source, document) {
                    if user_initiated {
                        self.open_link_tab(source, &url, background);
                    } else {
                        *self.feedback.borrow_mut() = Some("Pop-up blocked.".into());
                    }
                }
                if n == 63 {
                    context.request_redraw_at(std::time::Instant::now());
                }
                continue;
            }
            let editing = self
                .address_state
                .is_focused()
                .then(|| (self.browser.borrow().tab().id, self.address_state.value()));
            let committed = if let crate::engine::Event::Committed {
                tab, navigation, ..
            } = &event
            {
                Some((*tab, *navigation))
            } else {
                None
            };
            self.browser.borrow_mut().apply(event);
            if let Some((id, document)) = committed {
                let zoom = self
                    .browser
                    .borrow()
                    .tabs
                    .iter()
                    .find(|t| t.id == id && t.document.is_some() && t.navigation == document)
                    .map(|t| t.zoom);
                if let Some(percent) = zoom.filter(|z| *z != 100) {
                    if self.engine.borrow().features().zoom {
                        self.page_command(Command::SetZoom {
                            tab: id,
                            document,
                            percent,
                        });
                    }
                }
            }
            self.schedule_save();
            if let Some((id, draft)) = editing {
                if let Some(tab) = self
                    .browser
                    .borrow_mut()
                    .tabs
                    .iter_mut()
                    .find(|t| t.id == id)
                {
                    tab.draft = draft;
                }
            }
            if !self.address_state.is_focused() {
                self.sync_address();
            }
            if n == 63 {
                context.request_redraw_at(std::time::Instant::now());
            }
        }
        if let Some(deadline) = self.engine.borrow().next_poll_at() {
            context.request_redraw_at(deadline);
        }
    }
    fn perform(&self, action: Action) {
        self.feedback.borrow_mut().take();
        self.schedule_save();
        if self.rows.borrow().len() > 2048 {
            self.rows.borrow_mut().clear();
        }
        if self.buttons.borrow().len() > 2048 {
            self.buttons.borrow_mut().retain(|action, _| {
                !matches!(
                    action,
                    Action::RemoveBookmark(_) | Action::RemoveReading(_) | Action::EditBookmark(_)
                )
            });
        }
        match action {
            Action::New => {
                let result = self.browser.borrow_mut().new_tab();
                match result {
                    Ok(id) => {
                        self.lifecycle(Command::Create(id));
                        self.lifecycle(Command::Activate(id));
                        self.sync_address();
                        self.focus_address();
                    }
                    Err(e) => *self.feedback.borrow_mut() = Some(e),
                }
            }
            Action::Close(id) => {
                let old: Vec<_> = self.browser.borrow().tabs.iter().map(|t| t.id).collect();
                if !old.contains(&id) {
                    return;
                }
                if let Err(e) = self.browser.borrow_mut().close(id) {
                    *self.feedback.borrow_mut() = Some(e);
                    return;
                }
                self.lifecycle(Command::Close(id));
                let active = self.browser.borrow().tab().id;
                if !old.contains(&active) {
                    self.lifecycle(Command::Create(active));
                }
                self.lifecycle(Command::Activate(active));
                self.sync_address();
                if self.browser.borrow().tab().document.is_none() {
                    self.focus_address();
                }
            }
            Action::Reopen => {
                let result = self.browser.borrow_mut().reopen();
                match result {
                    Ok(Some(url)) => {
                        let id = self.browser.borrow().tab().id;
                        self.lifecycle(Command::Create(id));
                        self.lifecycle(Command::Activate(id));
                        self.perform(Action::Open(url));
                    }
                    Ok(None) => {}
                    Err(e) => *self.feedback.borrow_mut() = Some(e),
                }
            }
            Action::Navigate => self.perform(Action::Open(self.address_state.value())),
            Action::EditAddress => self.focus_address(),
            Action::Open(url) => {
                let request = self.browser.borrow_mut().navigate(&url);
                match request {
                    Ok(command) => {
                        self.editing_address.set(false);
                        self.address_bounds.set(None);
                        self.address_state.set_focused(false);
                        self.focus_address.set(false);
                        self.address_ready.set(None);
                        self.send(command);
                        self.sync_address();
                    }
                    Err(e) => *self.feedback.borrow_mut() = Some(e),
                }
            }
            Action::Back | Action::Forward | Action::Reload => {
                if !self.engine.borrow().available() {
                    return;
                }
                let delta = if action == Action::Back {
                    -1
                } else if action == Action::Forward {
                    1
                } else {
                    0
                };
                let request = self.browser.borrow_mut().travel(delta);
                match request {
                    Ok(Some(command)) => self.send(command),
                    Ok(None) => {}
                    Err(e) => *self.feedback.borrow_mut() = Some(e),
                }
            }
            Action::Stop => {
                let command = self.browser.borrow_mut().stop();
                if let Some(command) = command {
                    self.send(command);
                }
            }
            Action::Bookmark => self.browser.borrow_mut().bookmark(),
            Action::Bookmarks | Action::History => {
                let panel = if action == Action::Bookmarks {
                    Panel::Bookmarks
                } else {
                    Panel::History
                };
                self.panel.set(if self.panel.get() == panel {
                    Panel::None
                } else {
                    panel
                });
                self.panel_scroll.reset();
            }
            Action::ClosePanel => self.panel.set(Panel::None),
            Action::ClearHistory => self.browser.borrow_mut().history.clear(),
            Action::RemoveBookmark(url) => {
                self.browser.borrow_mut().bookmarks.retain(|e| e.url != url)
            }
            other => self.library_action(other),
        }
    }
    fn drain(&self) {
        for _ in 0..32 {
            let action = self.pending.borrow_mut().pop_front();
            let Some(action) = action else { break };
            self.perform(action);
        }
    }
    fn select(&self, index: usize) {
        let valid = {
            let b = self.browser.borrow();
            index < b.tabs.len() && index != b.active
        };
        if valid && self.finding.get() {
            self.library_action(Action::CloseFind);
        }
        let id = {
            let mut b = self.browser.borrow_mut();
            if index >= b.tabs.len() || index == b.active {
                return;
            }
            b.active = index;
            b.tab().id
        };
        self.lifecycle(Command::Activate(id));
        self.schedule_save();
        self.editing_address.set(false);
        self.address_bounds.set(None);
        self.address_state.set_focused(false);
        self.focus_address.set(false);
        self.address_ready.set(None);
        self.sync_address();
        let url = {
            let b = self.browser.borrow();
            let t = b.tab();
            if t.restore_pending && t.document.is_none() && !t.loading && t.error.is_none() {
                crate::browser::normalize_address(&t.draft).ok()
            } else {
                None
            }
        };
        if self.engine.borrow().available() {
            if let Some(url) = url {
                self.perform(Action::Open(url));
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct BrowserView {
    core: Rc<Core>,
}
impl BrowserView {
    /// Creates the retained browser UI with an explicitly supplied engine adapter.
    pub fn new(engine: Box<dyn Engine>) -> Self {
        let pending: Pending = Rc::new(RefCell::new(VecDeque::new()));
        let state = TextFieldInteractionState::new();
        let submit = pending.clone();
        let address = Rc::new(
            TextField::with_interaction(state.clone())
                .placeholder("Enter a website address")
                .bordered(false)
                .size(TextFieldSize::Medium)
                .on_submit(move || enqueue(&submit, Action::Navigate)),
        );
        let core = Rc::new(Core {
            browser: RefCell::new(Browser::new()),
            engine: RefCell::new(engine),
            pending,
            selected: State::new(0),
            tabs: TabsInteractionState::new(),
            tab_scroll: ScrollState::new(),
            address,
            address_state: state,
            focus_address: Cell::new(false),
            editing_address: Cell::new(false),
            address_bounds: Cell::new(None),
            address_ready: Cell::new(None),
            buttons: RefCell::new(BTreeMap::new()),
            icons: RefCell::new(BTreeMap::new()),
            rows: RefCell::new(BTreeMap::new()),
            menu: RefCell::new(None),
            tab_menu: RefCell::new(None),
            tab_context: RefCell::new(None),
            last_reveal: Cell::new(None),
            panel: Cell::new(Panel::None),
            panel_scroll: ScrollState::new(),
            suggestions_scroll: ScrollState::new(),
            panel_toolbar: ActionToolbarState::new(),
            feedback: RefCell::new(None),
            storage_error: RefCell::new(None),
            library_query: TextFieldInteractionState::new(),
            bookmark_title: TextFieldInteractionState::new(),
            bookmark_url: TextFieldInteractionState::new(),
            bookmark_original: RefCell::new(None),
            find_text: TextFieldInteractionState::new(),
            finding: Cell::new(false),
            next_find: Cell::new(1),
            find_focus: Cell::new(false),
            find_ready: Cell::new(None),
            store: RefCell::new(None),
            save_at: Cell::new(None),
        });
        core.lifecycle(Command::Create(1));
        core.lifecycle(Command::Activate(1));
        Self { core }
    }
    /// Loads per-user libraries/settings and restores uncommitted tab addresses.
    pub fn persistent(engine: Box<dyn Engine>) -> Self {
        let view = Self::new(engine);
        let result = crate::store::path()
            .and_then(|path| crate::store::Store::load(path, &mut view.core.browser.borrow_mut()));
        match result {
            Ok(store) => *view.core.store.borrow_mut() = Some(store),
            Err(error) => {
                *view.core.storage_error.borrow_mut() = Some(format!(
                    "Changes will not be saved. Browser data unavailable: {error}"
                ))
            }
        }
        let ids: Vec<_> = view
            .core
            .browser
            .borrow()
            .tabs
            .iter()
            .skip(1)
            .map(|t| t.id)
            .collect();
        for id in ids {
            view.core.lifecycle(Command::Create(id));
        }
        view.core
            .lifecycle(Command::Activate(view.core.browser.borrow().tab().id));
        view.core.sync_address();
        let url = view.core.browser.borrow().tab().draft.clone();
        if view.core.engine.borrow().available() && !url.is_empty() {
            view.core.perform(Action::Open(url));
        }
        view
    }
    fn navigation(&self, width: f32, theme: &Theme) -> Toolbar<HStack> {
        let b = self.core.browser.borrow();
        let t = b.tab();
        let available = self.core.engine.borrow().available();
        let reload = if t.loading {
            self.core
                .navigation_icon("Stop Loading", SymbolName::X, Action::Stop)
        } else {
            self.core
                .navigation_icon("Reload", SymbolName::Refresh, Action::Reload)
                .enabled(available && !t.draft.is_empty())
        };
        let bookmarked = t
            .document
            .as_ref()
            .is_some_and(|d| b.bookmarks.iter().any(|e| e.url == d.url));
        Toolbar::new(
            HStack::new()
                .alignment(StackAlignment::Center)
                .gap(StackGap::Small)
                .child(
                    self.core
                        .navigation_icon("Back", SymbolName::ArrowBackward, Action::Back)
                        .enabled(available && t.can_back)
                        .layout()
                        .flex_shrink(0.0),
                )
                .child(
                    self.core
                        .navigation_icon("Forward", SymbolName::ArrowForward, Action::Forward)
                        .enabled(available && t.can_forward)
                        .layout()
                        .flex_shrink(0.0),
                )
                .child(reload.layout().flex_shrink(0.0))
                .child(BrowserTabs(self.core.clone()).layout().flex_grow(1.0))
                .child(
                    self.core
                        .navigation_icon("New Tab (Ctrl+T)", SymbolName::Plus, Action::New)
                        .layout()
                        .flex_shrink(0.0),
                )
                .children(
                    if width
                        >= theme.layout.navigation_sidebar_width
                            + theme.layout.compact_form_control_width
                    {
                        vec![
                            self.core
                                .navigation_icon(
                                    if bookmarked {
                                        "Remove Bookmark"
                                    } else {
                                        "Bookmark Page"
                                    },
                                    if bookmarked {
                                        SymbolName::BookmarkFill
                                    } else {
                                        SymbolName::Bookmark
                                    },
                                    Action::Bookmark,
                                )
                                .enabled(t.document.is_some())
                                .layout()
                                .flex_shrink(0.0),
                            self.core
                                .navigation_icon(
                                    "Downloads",
                                    SymbolName::Download,
                                    Action::Downloads,
                                )
                                .layout()
                                .flex_shrink(0.0),
                        ]
                    } else {
                        vec![]
                    },
                )
                .child(self.core.more().layout().flex_shrink(0.0)),
        )
    }
    fn page(&self) -> Box<dyn View> {
        let b = self.core.browser.borrow();
        let tab = b.tab();
        if tab.error.is_none() && (tab.document.is_some() || tab.loading) {
            if let Some(page) = self.core.engine.borrow().page(tab.id) {
                return Box::new(Surface::pane().content(page));
            }
        }
        if tab.error.is_none() && tab.document.is_none() && !tab.loading {
            drop(b);
            return self.start_page();
        }
        let (title, description) = if let Some(error) = &tab.error {
            ("Unable to Open Page", error.clone())
        } else if tab.document.is_some() || tab.loading {
            (
                "Page View Unavailable",
                "The engine has not supplied a page view.".into(),
            )
        } else {
            ("Terra", "Enter a website address to get started.".into())
        };
        let mut content = VStack::new()
            .alignment(StackAlignment::Stretch)
            .gap(StackGap::Medium)
            .child(Text::styled(title, TextRole::TitleSmall))
            .child(Text::body(description).tone(TextTone::Secondary));
        if !self.core.engine.borrow().available() {
            content = content.child(
                Text::caption("The browsing engine is not connected.").tone(TextTone::Secondary),
            );
        }
        if tab.error.is_some() {
            content = content.child(
                self.core
                    .button("Try Again", None, Action::Navigate)
                    .enabled(self.core.engine.borrow().available()),
            );
        }
        Box::new(Scroll::vertical(ContentArea::pane(content)))
    }
    fn panel(&self) -> VStack {
        self.library_panel()
    }
    fn content(&self, theme: &Theme, width: f32) -> Surface<VStack> {
        let mut layout = VStack::new()
            .alignment(StackAlignment::Stretch)
            .gap(StackGap::None)
            .child(self.navigation(width, theme).layout().flex_shrink(0.0));
        if self.core.finding.get() {
            layout = layout.child(self.find_bar().layout().flex_shrink(0.0));
        }
        if self.core.browser.borrow().tab().loading {
            layout = layout.child(
                ProgressBar::new(self.core.browser.borrow().tab().progress)
                    .accessibility_label("Page loading progress")
                    .height(theme.spacing.extra_small)
                    .flex_shrink(0.0),
            );
        }
        if self.core.panel.get() == Panel::None {
            layout = layout.child(self.page().layout().flex_grow(1.0));
        } else if width
            < theme.layout.navigation_sidebar_width + theme.layout.compact_form_control_width
        {
            layout = layout.child(self.panel().layout().flex_grow(1.0));
        } else {
            layout = layout.child(
                NavigationSplitView::new(self.panel(), self.page())
                    .flexible_sidebar(160., theme.layout.navigation_sidebar_width, 320.)
                    .minimum_detail_width(160.)
                    .sidebar_placement(SidebarPlacement::Trailing)
                    .layout()
                    .flex_grow(1.0),
            );
        }
        let b = self.core.browser.borrow();
        let security = match b.tab().security {
            Security::Secure => "Secure connection",
            Security::Insecure => "Connection is not secure",
            Security::Unknown => "Connection status unavailable",
        };
        let status = self
            .core
            .feedback
            .borrow()
            .clone()
            .or_else(|| self.core.storage_error.borrow().clone())
            .unwrap_or_else(|| {
                if b.tab().zoom == 100 {
                    security.into()
                } else {
                    format!("{security} · {}%", b.tab().zoom)
                }
            });
        layout = layout.child(
            Surface::sidebar()
                .content(
                    Padding::symmetric(theme.spacing.large, 0.)
                        .content(Text::metadata(status).single_line(true)),
                )
                .height(theme.layout.status_bar_height)
                .flex_shrink(0.0),
        );
        Surface::app().content(layout)
    }
}
struct BrowserTabs(Rc<Core>);
impl BrowserTabs {
    fn item_width(&self, available: f32, theme: &Theme) -> f32 {
        let count = self.0.browser.borrow().visible_tabs().len().max(1) as f32;
        if self.0.editing_address.get() {
            available
        } else {
            (available / count).max(theme.layout.tab_width.min(available))
        }
    }
    fn scroll(&self, available: f32, theme: &Theme) -> Scroll {
        let browser = self.0.browser.borrow();
        let mut tabs = Tabs::with_interaction(self.0.selected.binding(), self.0.tabs.clone())
            .compact_style()
            .item_width(self.item_width(available, theme))
            .accessibility_label("Browser tabs");
        if self.0.editing_address.get() {
            tabs = tabs.selected_content(Address(self.0.clone()));
        } else {
            let pending = self.0.pending.clone();
            tabs = tabs.on_selected_activate(move || enqueue(&pending, Action::EditAddress));
        }
        for (index, tab) in browser.tabs.iter().enumerate() {
            if tab.group != browser.tab().group {
                continue;
            }
            if self.0.editing_address.get() && index != browser.active {
                continue;
            }
            let pending = self.0.pending.clone();
            let id = tab.id;
            let label = format!("{}{}", if tab.pinned { "• " } else { "" }, tab.title());
            tabs = tabs.closable_item(index, label, move || enqueue(&pending, Action::Close(id)));
        }
        Scroll::new(self.0.tab_scroll.clone())
            .axis(ScrollAxis::Horizontal)
            .content(tabs)
    }
}
impl View for BrowserTabs {
    fn measure(&self, c: Constraints, m: &mut MeasureContext<'_>) -> Size {
        c.constrain(Size::new(c.maximum.width, m.theme.tabs.height))
    }
    fn paint(&self, b: Rect, c: &mut PaintContext<'_>) {
        if b.size.width <= 0.0 || b.size.height <= 0.0 {
            return;
        }
        let browser = self.0.browser.borrow();
        let key = (
            browser.tab().id,
            b.size.width.to_bits(),
            browser.tabs.len(),
            self.0.editing_address.get(),
        );
        if self.0.last_reveal.get() != Some(key) {
            let width = self.item_width(b.size.width, c.theme);
            let left = if self.0.editing_address.get() {
                0.0
            } else {
                browser
                    .visible_tabs()
                    .iter()
                    .position(|i| *i == browser.active)
                    .unwrap_or(0) as f32
                    * width
            };
            let offset = self.0.tab_scroll.offset_x();
            if left < offset {
                self.0.tab_scroll.set_offset(left, 0.0);
            } else if left + width > offset + b.size.width {
                self.0
                    .tab_scroll
                    .set_offset((left + width - b.size.width).max(0.0), 0.0);
            }
            self.0.last_reveal.set(Some(key));
        }
        drop(browser);
        self.scroll(b.size.width, c.theme).paint(b, c);
    }
    fn handle_event(&self, b: Rect, e: &ViewEvent, c: &mut EventContext<'_>) -> EventResult {
        if let ViewEvent::PointerPressed {
            position,
            button: PointerButton::Secondary,
        } = e
        {
            if b.contains(*position) {
                let visible = self.0.browser.borrow().visible_tabs();
                let slot = ((position.x - b.origin.x + self.0.tab_scroll.offset_x())
                    / self.item_width(b.size.width, c.theme()))
                .floor() as usize;
                let index = if self.0.editing_address.get() {
                    Some(self.0.browser.borrow().active)
                } else {
                    visible.get(slot).copied()
                };
                if let Some(index) = index {
                    self.0.select(index);
                    let mut request =
                        appcore::viewkit::event::ContextMenuRequest::new(*position, vec![]);
                    let request_id = request.request_id;
                    let options = self.0.tab_options();
                    let actions = options
                        .iter()
                        .map(|(_, action, enabled)| enabled.then_some(action.clone()))
                        .collect();
                    *self.0.tab_context.borrow_mut() =
                        Some((request_id, self.0.browser.borrow().tab().id, actions));
                    request.items = options
                        .into_iter()
                        .enumerate()
                        .map(
                            |(i, (label, _, enabled))| appcore::viewkit::event::ContextMenuItem {
                                command_id: i as u32 + 1,
                                label: label.into(),
                                enabled,
                                checked: false,
                                destructive: false,
                                separator: false,
                            },
                        )
                        .collect();
                    c.show_context_menu(request);
                    c.request_redraw();
                    return EventResult::Consumed;
                }
            }
        }
        self.scroll(b.size.width, c.theme()).handle_event(b, e, c)
    }
}

struct Address(Rc<Core>);
impl View for Address {
    fn measure(&self, c: Constraints, m: &mut MeasureContext<'_>) -> Size {
        self.0.address.measure(c, m)
    }
    fn paint(&self, b: Rect, c: &mut PaintContext<'_>) {
        self.0.address_bounds.set(Some(b));
        if self.0.focus_address.get() {
            self.0.address_state.set_focused(true);
            self.0.address_ready.set(Some(b));
        }
        self.0.address.paint(b, c);
    }
    fn handle_event(&self, b: Rect, e: &ViewEvent, c: &mut EventContext<'_>) -> EventResult {
        self.0.address.handle_event(b, e, c)
    }
}
impl View for BrowserView {
    fn measure(&self, c: Constraints, _: &mut MeasureContext<'_>) -> Size {
        c.constrain(c.maximum)
    }
    fn paint(&self, b: Rect, c: &mut PaintContext<'_>) {
        self.core.drain();
        self.core.poll(c);
        if let Some(deadline) = self.core.save_at.get() {
            if std::time::Instant::now() >= deadline {
                self.core.persist();
            } else {
                c.request_redraw_at(deadline);
            }
        }
        self.content(c.theme, b.size.width).paint(b, c);
        let popup = self.suggestions(
            b,
            &mut MeasureContext {
                theme: c.theme,
                typography: c.typography,
                text_measurer: &mut *c.text_measurer,
            },
        );
        if let Some((bounds, popup)) = popup {
            popup.paint(bounds, c);
        }
    }
    fn handle_event(&self, b: Rect, e: &ViewEvent, c: &mut EventContext<'_>) -> EventResult {
        if let ViewEvent::ContextMenuResult {
            request_id,
            command_id,
        } = e
        {
            let matches = self
                .core
                .tab_context
                .borrow()
                .as_ref()
                .is_some_and(|(id, _, _)| id == request_id);
            if matches {
                let (_, tab, actions) = self.core.tab_context.borrow_mut().take().unwrap();
                let index = self
                    .core
                    .browser
                    .borrow()
                    .tabs
                    .iter()
                    .position(|t| t.id == tab);
                if let Some(index) = index {
                    if let Some(action) = command_id
                        .and_then(|id| id.checked_sub(1))
                        .and_then(|id| actions.get(id as usize))
                        .and_then(Clone::clone)
                    {
                        self.core.select(index);
                        self.core.perform(action);
                    }
                }
                c.request_redraw();
                return EventResult::Consumed;
            }
        }
        let popup = self.suggestions(b, &mut c.measure_context());
        let outside = matches!(e, ViewEvent::PointerFocusRequested { position }
            if !self.core.address_bounds.get().is_some_and(|r|r.contains(*position))
                && !popup.as_ref().is_some_and(|(bounds,_)|bounds.contains(*position)));
        if self.core.editing_address.get()
            && (outside || matches!(e, ViewEvent::FocusChanged { focused: false }))
        {
            if let Some(frame) = self.core.address_bounds.get() {
                self.core.address.handle_event(
                    frame,
                    &ViewEvent::KeyboardFocusRequested { bounds: None },
                    c,
                );
            }
            self.core.editing_address.set(false);
            self.core.address_bounds.set(None);
            c.request_redraw();
        }
        if matches!(
            e,
            ViewEvent::PointerFocusRequested { .. } | ViewEvent::FocusChanged { focused: false }
        ) {
            self.core.focus_address.set(false);
            self.core.address_ready.set(None);
        } else if !matches!(e, ViewEvent::KeyboardFocusRequested { .. }) {
            if let Some(target) = self.core.address_ready.take() {
                self.core.focus_address.set(false);
                self.content(c.theme(), b.size.width).handle_event(
                    b,
                    &ViewEvent::KeyboardFocusRequested {
                        bounds: Some(target),
                    },
                    c,
                );
                c.request_keyboard_focus(target);
            }
        }
        if !matches!(e, ViewEvent::KeyboardFocusRequested { .. }) {
            if matches!(
                e,
                ViewEvent::PointerFocusRequested { .. }
                    | ViewEvent::FocusChanged { focused: false }
            ) {
                self.core.find_focus.set(false);
                self.core.find_ready.set(None);
            } else if let Some(target) = self.core.find_ready.take() {
                self.core.find_focus.set(false);
                self.content(c.theme(), b.size.width).handle_event(
                    b,
                    &ViewEvent::KeyboardFocusRequested {
                        bounds: Some(target),
                    },
                    c,
                );
                c.request_keyboard_focus(target);
            }
        }
        if matches!(
            e,
            ViewEvent::KeyPressed {
                key: Key::Escape,
                ..
            }
        ) && self.core.editing_address.get()
        {
            let url = self
                .core
                .browser
                .borrow()
                .tab()
                .document
                .as_ref()
                .map(|d| d.url.clone())
                .unwrap_or_default();
            self.core.address_state.set_value(url.clone());
            self.core.address_state.set_focused(false);
            let mut browser = self.core.browser.borrow_mut();
            let active = browser.active;
            browser.tabs[active].draft = url;
            drop(browser);
            self.core.editing_address.set(false);
            self.core.focus_address.set(false);
            self.core.address_ready.set(None);
            self.core.address_bounds.set(None);
            c.request_redraw();
            return EventResult::Consumed;
        }
        if let ViewEvent::KeyPressed { key, modifiers } = e {
            let action = if modifiers.alt() && !modifiers.shortcut() {
                match key {
                    Key::ArrowLeft => Some(Action::Back),
                    Key::ArrowRight => Some(Action::Forward),
                    _ => None,
                }
            } else if modifiers.shortcut() {
                match key {
                    Key::Character('t' | 'T') => Some(if modifiers.shift() {
                        Action::Reopen
                    } else {
                        Action::New
                    }),
                    Key::Character('w' | 'W') => {
                        Some(Action::Close(self.core.browser.borrow().tab().id))
                    }
                    Key::Character('r' | 'R') => Some(Action::Reload),
                    Key::Character('d' | 'D') => Some(Action::Bookmark),
                    Key::Character('h' | 'H') => Some(Action::History),
                    Key::Character('f' | 'F') => Some(Action::Find),
                    Key::Character('g' | 'G') => Some(Action::FindNext(modifiers.shift())),
                    Key::Character('+' | '=') => Some(Action::Zoom(10)),
                    Key::Character('-') => Some(Action::Zoom(-10)),
                    Key::Character('0') => Some(Action::Zoom(0)),
                    Key::Character('j' | 'J') => Some(Action::Downloads),
                    Key::Character('1'..='9') => {
                        if let Key::Character(digit) = key {
                            let visible = self.core.browser.borrow().visible_tabs();
                            let position = if *digit == '9' {
                                visible.len() - 1
                            } else {
                                (*digit as usize - '1' as usize).min(visible.len() - 1)
                            };
                            self.core.select(visible[position]);
                            c.request_redraw();
                            return EventResult::Consumed;
                        }
                        None
                    }
                    Key::Character('b' | 'B') => Some(Action::Bookmarks),
                    Key::Character('l' | 'L') => {
                        self.content(c.theme(), b.size.width).handle_event(
                            b,
                            &ViewEvent::KeyboardFocusRequested { bounds: None },
                            c,
                        );
                        self.core.focus_address();
                        c.request_redraw();
                        return EventResult::Consumed;
                    }
                    Key::Tab => {
                        let b = self.core.browser.borrow();
                        let visible = b.visible_tabs();
                        let len = visible.len();
                        let position = visible.iter().position(|i| *i == b.active).unwrap_or(0);
                        let index = visible[if modifiers.shift() {
                            (position + len - 1) % len
                        } else {
                            (position + 1) % len
                        }];
                        drop(b);
                        self.core.select(index);
                        c.request_redraw();
                        return EventResult::Consumed;
                    }
                    _ => None,
                }
            } else if *key == Key::Escape && self.core.finding.get() {
                Some(Action::CloseFind)
            } else if *key == Key::Escape && self.core.browser.borrow().tab().loading {
                Some(Action::Stop)
            } else {
                None
            };
            if let Some(action) = action {
                self.core.perform(action);
                c.request_redraw();
                return EventResult::Consumed;
            }
        }
        let overlay_result = popup
            .map(|(bounds, popup)| popup.handle_event(bounds, e, c))
            .unwrap_or(EventResult::Ignored);
        let result = if overlay_result.is_consumed() {
            overlay_result
        } else {
            self.content(c.theme(), b.size.width).handle_event(b, e, c)
        };
        {
            let mut browser = self.core.browser.borrow_mut();
            let active = browser.active;
            browser.tabs[active].draft = self.core.address_state.value();
        }
        self.core.select(self.core.selected.get());
        let editing = self.core.editing_address.get();
        self.core.drain();
        if !editing && self.core.editing_address.get() {
            self.content(c.theme(), b.size.width).handle_event(
                b,
                &ViewEvent::KeyboardFocusRequested { bounds: None },
                c,
            );
            self.core.address_state.set_focused(true);
        }
        if result.is_consumed() {
            c.request_redraw();
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Event, NavigationId};
    use appcore::viewkit::{
        draw_command::{DisplayList, DrawCommand},
        event::EventDispatcher,
        platform::{KeyModifiers, PlatformEvent},
        typography::TextMeasurer,
    };
    #[derive(Clone, Default)]
    struct Harness {
        commands: Rc<RefCell<Vec<Command>>>,
        events: Rc<RefCell<VecDeque<Event>>>,
        reject_tools: Rc<Cell<bool>>,
    }
    impl Engine for Harness {
        fn features(&self) -> crate::engine::Features {
            crate::engine::Features {
                find: true,
                zoom: true,
                downloads: true,
                reader: true,
                printing: true,
            }
        }
        fn available(&self) -> bool {
            true
        }
        fn send(&mut self, c: Command) -> Result<(), String> {
            if self.reject_tools.get() {
                return Err("Page tool unavailable".into());
            }
            self.commands.borrow_mut().push(c);
            Ok(())
        }
        fn poll(&mut self) -> Option<Event> {
            self.events.borrow_mut().pop_front()
        }
        fn page(&self, _: TabId) -> Option<Rc<dyn View>> {
            Some(Rc::new(Text::body("Engine page")))
        }
        fn next_poll_at(&self) -> Option<std::time::Instant> {
            None
        }
    }
    fn snapshot(v: &BrowserView, b: Rect, theme: &Theme) -> (DisplayList, Vec<AccessibilityNode>) {
        let mut list = DisplayList::new();
        let mut nodes = vec![];
        let mut text = TextMeasurer::new();
        v.paint(
            b,
            &mut PaintContext::new(&mut list, theme, &theme.typography, &mut text)
                .with_accessibility_nodes(&mut nodes),
        );
        (list, nodes)
    }
    fn commit(
        v: &BrowserView,
        h: &Harness,
        url: &str,
        security: Security,
    ) -> (TabId, NavigationId) {
        v.core.perform(Action::Open(url.into()));
        let Command::Navigate {
            tab, navigation, ..
        } = h.commands.borrow().last().unwrap().clone()
        else {
            panic!("navigation request")
        };
        h.events.borrow_mut().extend([
            Event::Committed {
                tab,
                navigation,
                url: url.into(),
                title: "Engine title".into(),
                security,
                can_back: true,
                can_forward: false,
            },
            Event::Finished { tab, navigation },
        ]);
        snapshot(v, Rect::new(0., 0., 1000., 700.), &Theme::LIGHT);
        (tab, navigation)
    }
    #[test]
    fn disconnected_engine_is_honest_and_navigation_controls_are_disabled() {
        let v = BrowserView::new(Box::new(crate::engine::Unavailable));
        v.core.perform(Action::Open("example.org".into()));
        let (list, nodes) = snapshot(&v, Rect::new(0., 0., 800., 600.), &Theme::LIGHT);
        assert!(v.core.browser.borrow().tab().error.is_some());
        assert!(v.core.browser.borrow().history.is_empty());
        for label in ["Back", "Forward", "Reload"] {
            assert!(
                !nodes
                    .iter()
                    .find(|n| n.label.as_deref() == Some(label))
                    .unwrap()
                    .enabled
            );
        }
        assert!(list.commands().iter().any(
            |c| matches!(c,DrawCommand::DrawText{command} if command.text=="Unable to Open Page")
        ));
        assert!(!list.commands().iter().any(
            |c| matches!(c,DrawCommand::DrawText{command} if command.text=="Secure connection")
        ));
    }
    #[test]
    fn backend_commits_drive_page_security_history_and_bookmarks() {
        let h = Harness::default();
        let v = BrowserView::new(Box::new(h.clone()));
        commit(&v, &h, "https://example.org/", Security::Unknown);
        assert_eq!(v.core.browser.borrow().history.len(), 1);
        assert_eq!(v.core.browser.borrow().tab().security, Security::Unknown);
        v.core.perform(Action::Bookmark);
        assert_eq!(v.core.browser.borrow().bookmarks.len(), 1);
        let (list, nodes) = snapshot(&v, Rect::new(0., 0., 800., 600.), &Theme::LIGHT);
        assert!(
            nodes
                .iter()
                .find(|n| n.label.as_deref() == Some("Back"))
                .unwrap()
                .enabled
        );
        assert!(
            list.commands()
                .iter()
                .any(|c| matches!(c,DrawCommand::DrawText{command} if command.text=="Engine page"))
        );
        assert!(list.commands().iter().any(|c|matches!(c,DrawCommand::DrawText{command} if command.text=="Connection status unavailable")));
        v.core.perform(Action::ClearHistory);
        assert!(v.core.browser.borrow().history.is_empty());
        assert_eq!(v.core.browser.borrow().bookmarks.len(), 1);
        v.core.perform(Action::Back);
        assert!(matches!(
            h.commands.borrow().last(),
            Some(Command::Back { .. })
        ));
        commit(&v, &h, "http://example.org/", Security::Insecure);
        let (list, _) = snapshot(&v, Rect::new(0., 0., 800., 600.), &Theme::LIGHT);
        assert!(list.commands().iter().any(|c| matches!(c,DrawCommand::DrawText{command} if command.text=="Connection is not secure")));
    }
    #[test]
    fn per_tab_drafts_and_closed_sessions_do_not_leak_into_other_tabs() {
        let h = Harness::default();
        let v = BrowserView::new(Box::new(h.clone()));
        commit(&v, &h, "https://example.org/", Security::Secure);
        let first = v.core.browser.borrow().tab().id;
        v.core.perform(Action::New);
        v.core.browser.borrow_mut().tabs[1].draft = "unsent.example".into();
        v.core.sync_address();
        v.core.select(0);
        assert_eq!(v.core.address_state.value(), "https://example.org/");
        v.core.select(1);
        assert_eq!(v.core.address_state.value(), "unsent.example");
        v.core.perform(Action::Close(first));
        assert_eq!(v.core.browser.borrow().tabs.len(), 1);
        assert_eq!(v.core.address_state.value(), "unsent.example");
        assert!(h.commands.borrow().contains(&Command::Close(first)));
    }
    #[test]
    fn asynchronous_commits_do_not_overwrite_an_address_being_edited() {
        let h = Harness::default();
        let v = BrowserView::new(Box::new(h.clone()));
        v.core.perform(Action::Open("https://example.org/".into()));
        let Command::Navigate {
            tab, navigation, ..
        } = h.commands.borrow().last().unwrap().clone()
        else {
            panic!()
        };
        v.core.address_state.set_focused(true);
        v.core.address_state.set_value("editing.example");
        h.events.borrow_mut().push_back(Event::Committed {
            tab,
            navigation,
            url: "https://example.org/redirect".into(),
            title: "Redirect".into(),
            security: Security::Secure,
            can_back: false,
            can_forward: false,
        });
        snapshot(&v, Rect::new(0., 0., 800., 600.), &Theme::LIGHT);
        assert_eq!(v.core.address_state.value(), "editing.example");
        assert_eq!(v.core.browser.borrow().tab().draft, "editing.example");
        assert_eq!(
            v.core.browser.borrow().tab().document.as_ref().unwrap().url,
            "https://example.org/redirect"
        );
    }
    #[test]
    fn native_chrome_keeps_address_and_actions_separate_and_small_panels_can_close() {
        for theme in [Theme::LIGHT, Theme::DARK] {
            let v = BrowserView::new(Box::new(crate::engine::Unavailable));
            for width in [320., 640., 1100.] {
                let b = Rect::new(0., 0., width, 480.);
                v.core.focus_address();
                let (_, nodes) = snapshot(&v, b, &theme);
                let field = nodes
                    .iter()
                    .find(|n| n.role == AccessibilityRole::TextField)
                    .unwrap()
                    .bounds;
                assert!(field.size.width > 0. && field.size.height > 0.);
                assert!(field.origin.x >= 0. && field.origin.x + field.size.width <= width + 0.1);
                for label in ["Back", "Forward", "Reload", "Bookmark Page", "More"] {
                    let Some(node) = nodes.iter().find(|n| n.label.as_deref() == Some(label))
                    else {
                        assert!(width == 320. && label == "Bookmark Page");
                        continue;
                    };
                    assert!(
                        node.bounds.intersection(field).is_none(),
                        "{label} overlaps address at {width}"
                    );
                }
                v.core.perform(Action::History);
                let (_, nodes) = snapshot(&v, b, &theme);
                assert!(
                    nodes
                        .iter()
                        .any(|n| n.label.as_deref() == Some("Close Sidebar"))
                );
                v.core.perform(Action::ClosePanel);
            }
        }
    }
    #[test]
    fn more_menu_survives_repaint_and_newly_selected_tabs_are_revealed() {
        use appcore::viewkit::platform::PointerButton;
        let v = BrowserView::new(Box::new(crate::engine::Unavailable));
        let b = Rect::new(0., 0., 320., 480.);
        let theme = Theme::LIGHT;
        let (_, nodes) = snapshot(&v, b, &theme);
        let more = nodes
            .iter()
            .find(|n| n.label.as_deref() == Some("More"))
            .unwrap()
            .bounds;
        let p = Point::new(
            more.origin.x + more.size.width / 2.,
            more.origin.y + more.size.height / 2.,
        );
        let mut text = TextMeasurer::new();
        let mut c = EventContext::new(&theme, &theme.typography, &mut text);
        v.handle_event(
            b,
            &ViewEvent::PointerPressed {
                position: p,
                button: PointerButton::Primary,
            },
            &mut c,
        );
        v.handle_event(
            b,
            &ViewEvent::PointerReleased {
                position: p,
                button: PointerButton::Primary,
            },
            &mut c,
        );
        let request = c.take_context_menu_request().expect("symbol command menu");
        assert!(
            !request
                .items
                .iter()
                .find(|i| i.label == "Reopen Closed Tab")
                .unwrap()
                .enabled
        );
        snapshot(&v, b, &theme);
        v.handle_event(
            b,
            &ViewEvent::ContextMenuResult {
                request_id: request.request_id,
                command_id: Some(1),
            },
            &mut c,
        );
        assert_eq!(v.core.browser.borrow().tabs.len(), 2);
        for _ in 0..8 {
            v.core.perform(Action::New);
        }
        v.handle_event(
            b,
            &ViewEvent::KeyPressed {
                key: Key::Escape,
                modifiers: KeyModifiers::default(),
            },
            &mut c,
        );
        snapshot(&v, b, &theme);
        assert!(v.core.tab_scroll.offset_x() > 0.);
        let old = v.core.tab_scroll.offset_x();
        snapshot(&v, b, &theme);
        assert_eq!(v.core.tab_scroll.offset_x(), old);
    }

    #[test]
    fn selected_segment_click_expands_the_address_and_escape_restores_the_title() {
        use appcore::viewkit::platform::{ButtonState, PointerButton};
        let v = BrowserView::new(Box::new(crate::engine::Unavailable));
        let b = Rect::new(0., 0., 800., 480.);
        let theme = Theme::LIGHT;
        let (_, nodes) = snapshot(&v, b, &theme);
        assert!(!nodes.iter().any(|n| n.role == AccessibilityRole::TextField));
        let tab = nodes
            .iter()
            .find(|n| n.role == AccessibilityRole::Tab && n.selected)
            .unwrap()
            .bounds;
        let mut d = EventDispatcher::new();
        d.set_accessibility_nodes(&nodes);
        let mut text = TextMeasurer::new();
        let mut c = EventContext::new(&theme, &theme.typography, &mut text);
        d.dispatch(&v, b, &PlatformEvent::Focused(true), &mut c);
        d.dispatch(
            &v,
            b,
            &PlatformEvent::PointerMoved {
                x: tab.origin.x + tab.size.width * 0.4,
                y: tab.origin.y + tab.size.height / 2.,
            },
            &mut c,
        );
        for state in [ButtonState::Pressed, ButtonState::Released] {
            d.dispatch(
                &v,
                b,
                &PlatformEvent::PointerButton {
                    button: PointerButton::Primary,
                    state,
                },
                &mut c,
            );
        }
        assert!(v.core.editing_address.get());
        let (_, nodes) = snapshot(&v, b, &theme);
        d.set_accessibility_nodes(&nodes);
        assert_eq!(
            nodes
                .iter()
                .filter(|n| n.role == AccessibilityRole::TextField)
                .count(),
            1
        );
        d.dispatch(
            &v,
            b,
            &PlatformEvent::TextInput {
                text: "example.org".into(),
            },
            &mut c,
        );
        assert_eq!(v.core.address_state.value(), "example.org");
        d.dispatch(
            &v,
            b,
            &PlatformEvent::KeyPressed {
                key: Key::Escape,
                modifiers: KeyModifiers::default(),
            },
            &mut c,
        );
        assert!(!v.core.editing_address.get());
        assert!(
            !snapshot(&v, b, &theme)
                .1
                .iter()
                .any(|n| n.role == AccessibilityRole::TextField)
        );
        assert!(v.core.browser.borrow().history.is_empty());
    }
    #[test]
    fn right_hand_actions_open_a_right_hand_sidebar() {
        let v = BrowserView::new(Box::new(crate::engine::Unavailable));
        let b = Rect::new(0., 0., 1100., 700.);
        for action in [Action::History, Action::Bookmarks] {
            v.core.perform(action);
            let (_, nodes) = snapshot(&v, b, &Theme::LIGHT);
            let close = nodes
                .iter()
                .find(|n| n.label.as_deref() == Some("Close Sidebar"))
                .unwrap()
                .bounds;
            assert!(close.origin.x > b.size.width * 0.5);
            v.core.perform(Action::ClosePanel);
        }
    }

    #[test]
    fn navigation_icons_keep_equal_targets_and_horizontal_spacing() {
        let v = BrowserView::new(Box::new(crate::engine::Unavailable));
        for theme in [Theme::LIGHT, Theme::DARK] {
            for width in [320., 800., 1100.] {
                for editing in [false, true] {
                    v.core.editing_address.set(editing);
                    let (_, nodes) = snapshot(&v, Rect::new(0., 0., width, 480.), &theme);
                    let buttons: Vec<_> = [
                        "Back",
                        "Forward",
                        "Reload",
                        "New Tab (Ctrl+T)",
                        "Bookmark Page",
                        "Downloads",
                        "More",
                    ]
                    .into_iter()
                    .filter_map(|label| {
                        nodes
                            .iter()
                            .find(|n| {
                                n.role == AccessibilityRole::Button
                                    && n.label.as_deref() == Some(label)
                            })
                            .map(|n| n.bounds)
                    })
                    .collect();
                    for button in &buttons {
                        assert!(
                            (button.size.width - theme.layout.compact_control_height).abs() < 0.1
                        );
                        assert!((button.size.height - buttons[0].size.height).abs() < 0.1);
                        assert!((button.origin.y - buttons[0].origin.y).abs() < 0.1);
                    }
                    for (a, b) in (0..buttons.len() - 1)
                        .filter(|i| *i != 2)
                        .map(|i| (i, i + 1))
                    {
                        let gap = buttons[b].origin.x - buttons[a].origin.x - buttons[a].size.width;
                        assert!((gap - theme.spacing.small).abs() < 0.1, "gap={gap}");
                    }
                    let left = buttons[0].origin.x;
                    let last = buttons.last().unwrap();
                    let right = width - last.origin.x - last.size.width;
                    assert!((left - right).abs() < 0.1);
                    assert!(left >= theme.spacing.small);
                    let tabs = nodes
                        .iter()
                        .find(|n| n.role == AccessibilityRole::Tab)
                        .unwrap()
                        .bounds;
                    assert!(
                        tabs.origin.x
                            >= buttons[2].origin.x + buttons[2].size.width + theme.spacing.small
                                - 0.1
                    );
                    assert!(
                        tabs.origin.x + tabs.size.width
                            <= buttons[3].origin.x - theme.spacing.small + 0.1
                    );
                }
            }
        }
    }

    #[test]
    fn selected_tab_is_the_only_address_control_and_more_is_symbol_only() {
        let v = BrowserView::new(Box::new(crate::engine::Unavailable));
        v.core.perform(Action::New);
        v.core.perform(Action::New);
        for theme in [Theme::LIGHT, Theme::DARK] {
            for width in [320., 800., 1100.] {
                v.core.focus_address();
                let (list, nodes) = snapshot(&v, Rect::new(0., 0., width, 480.), &theme);
                let fields: Vec<_> = nodes
                    .iter()
                    .filter(|n| n.role == AccessibilityRole::TextField)
                    .collect();
                assert_eq!(fields.len(), 1);
                let tab = nodes
                    .iter()
                    .find(|n| n.role == AccessibilityRole::Tab && n.selected)
                    .unwrap()
                    .bounds;
                let field = fields[0].bounds;
                assert!(
                    field.origin.x >= tab.origin.x
                        && field.origin.x + field.size.width <= tab.origin.x + tab.size.width + 0.1
                );
                assert!(
                    field.origin.y >= tab.origin.y
                        && field.origin.y + field.size.height
                            <= tab.origin.y + tab.size.height + 0.1
                );
                assert!(
                    nodes.iter().any(|n| n.role == AccessibilityRole::Button
                        && n.label.as_deref() == Some("More"))
                );
                assert!(!list.commands().iter().any(|c|matches!(c,DrawCommand::DrawText{command} if matches!(command.text.as_str(),"File"|"Edit"|"View"|"More"))));
            }
        }
    }

    #[test]
    fn ctrl_l_enter_and_tab_shortcuts_work_through_the_real_dispatcher() {
        let v = BrowserView::new(Box::new(crate::engine::Unavailable));
        let b = Rect::new(0., 0., 800., 600.);
        let theme = Theme::LIGHT;
        let mut d = EventDispatcher::new();
        let mut text = TextMeasurer::new();
        let mut c = EventContext::new(&theme, &theme.typography, &mut text);
        d.set_accessibility_nodes(&snapshot(&v, b, &theme).1);
        d.dispatch(&v, b, &PlatformEvent::Focused(true), &mut c);
        let ctrl = KeyModifiers::from_bits(KeyModifiers::CONTROL);
        d.dispatch(
            &v,
            b,
            &PlatformEvent::KeyPressed {
                key: Key::Character('l'),
                modifiers: ctrl,
            },
            &mut c,
        );
        d.set_accessibility_nodes(&snapshot(&v, b, &theme).1);
        d.dispatch(
            &v,
            b,
            &PlatformEvent::TextInput {
                text: "example.org".into(),
            },
            &mut c,
        );
        assert_eq!(v.core.address_state.value(), "example.org");
        assert!(
            v.core.address_state.is_focused(),
            "address focus must survive dispatcher reconciliation"
        );
        d.dispatch(
            &v,
            b,
            &PlatformEvent::KeyPressed {
                key: Key::Enter,
                modifiers: KeyModifiers::default(),
            },
            &mut c,
        );
        assert!(v.core.browser.borrow().tab().error.is_some());
        assert!(v.core.browser.borrow().history.is_empty());
        d.dispatch(
            &v,
            b,
            &PlatformEvent::KeyPressed {
                key: Key::Character('t'),
                modifiers: ctrl,
            },
            &mut c,
        );
        assert_eq!(v.core.browser.borrow().tabs.len(), 2);
        d.dispatch(
            &v,
            b,
            &PlatformEvent::KeyPressed {
                key: Key::Character('w'),
                modifiers: ctrl,
            },
            &mut c,
        );
        assert_eq!(v.core.browser.borrow().tabs.len(), 1);
    }
    #[test]
    fn bookmark_editor_and_tab_groups_work_without_a_renderer() {
        let v = BrowserView::new(Box::new(crate::engine::Unavailable));
        v.core.perform(Action::AddBookmark);
        v.core.bookmark_title.set_value("mochiOS");
        v.core.bookmark_url.set_value("mochios.org");
        v.core.perform(Action::SaveBookmark);
        assert_eq!(
            v.core.browser.borrow().bookmarks[0].url,
            "https://mochios.org/"
        );
        assert_eq!(v.core.panel.get(), Panel::Bookmarks);
        v.core
            .perform(Action::EditBookmark("https://mochios.org/".into()));
        v.core.bookmark_title.set_value("Documentation");
        v.core.bookmark_url.set_value("docs.mochios.org");
        v.core.perform(Action::SaveBookmark);
        assert_eq!(v.core.browser.borrow().bookmarks.len(), 1);
        assert_eq!(v.core.browser.borrow().bookmarks[0].title, "Documentation");
        assert!(v.core.browser.borrow().history.is_empty());
        v.core.perform(Action::NewGroup);
        v.core.bookmark_title.set_value("Research");
        v.core.perform(Action::CreateGroup);
        assert_eq!(v.core.browser.borrow().tab().group, "Research");
        v.core.perform(Action::New);
        v.core.perform(Action::Pin);
        let b = Rect::new(0., 0., 900., 600.);
        let (_, nodes) = snapshot(&v, b, &Theme::LIGHT);
        let visible = v.core.browser.borrow().visible_tabs().len();
        assert_eq!(visible, 2);
        assert_eq!(
            nodes
                .iter()
                .filter(|n| n.role == AccessibilityRole::Tab)
                .count(),
            1
        ); // active inline URL editor
        v.core.editing_address.set(false);
        let (_, nodes) = snapshot(&v, b, &Theme::LIGHT);
        assert_eq!(
            nodes
                .iter()
                .filter(|n| n.role == AccessibilityRole::Tab)
                .count(),
            2
        );
        v.core.perform(Action::SelectGroup("Default".into()));
        assert_eq!(v.core.browser.borrow().visible_tabs().len(), 1);
    }
    #[test]
    fn page_tools_use_document_ids_and_errors_do_not_destroy_the_page() {
        let h = Harness::default();
        let v = BrowserView::new(Box::new(h.clone()));
        let (tab, document) = commit(&v, &h, "example.org", Security::Secure);
        v.core.perform(Action::Zoom(10));
        assert_eq!(v.core.browser.borrow().tab().zoom, 110);
        assert_eq!(
            h.commands.borrow().last(),
            Some(&Command::SetZoom {
                tab,
                document,
                percent: 110
            })
        );
        v.core.perform(Action::Reader);
        assert_eq!(
            h.commands.borrow().last(),
            Some(&Command::ToggleReader { tab, document })
        );
        v.core.perform(Action::Print);
        assert_eq!(
            h.commands.borrow().last(),
            Some(&Command::Print { tab, document })
        );
        v.core.perform(Action::Find);
        v.core.find_text.set_value("mochi");
        v.core.perform(Action::FindNext(false));
        let request = match h.commands.borrow().last().unwrap() {
            Command::Find { request, .. } => *request,
            _ => panic!(),
        };
        h.events.borrow_mut().push_back(Event::FindResult {
            tab,
            document,
            request,
            current: 1,
            total: 4,
        });
        snapshot(&v, Rect::new(0., 0., 900., 600.), &Theme::LIGHT);
        assert_eq!(v.core.browser.borrow().tab().find, Some((request, 1, 4)));
        v.core.perform(Action::CloseFind);
        assert!(!v.core.finding.get());
        assert_eq!(
            h.commands.borrow().last(),
            Some(&Command::ClearFind { tab, document })
        );
        h.reject_tools.set(true);
        v.core.perform(Action::Zoom(10));
        assert_eq!(v.core.browser.borrow().tab().zoom, 110);
        assert!(v.core.browser.borrow().tab().document.is_some());
        assert!(v.core.browser.borrow().tab().error.is_none());
        assert!(
            v.core
                .feedback
                .borrow()
                .as_ref()
                .unwrap()
                .contains("unavailable")
        );
    }
    #[test]
    fn find_keyboard_focus_and_submit_reach_the_engine() {
        let h = Harness::default();
        let v = BrowserView::new(Box::new(h.clone()));
        commit(&v, &h, "example.org", Security::Secure);
        let bounds = Rect::new(0., 0., 900., 600.);
        let theme = Theme::LIGHT;
        let mut d = EventDispatcher::new();
        let mut text = TextMeasurer::new();
        let mut c = EventContext::new(&theme, &theme.typography, &mut text);
        d.set_accessibility_nodes(&snapshot(&v, bounds, &theme).1);
        d.dispatch(&v, bounds, &PlatformEvent::Focused(true), &mut c);
        d.dispatch(
            &v,
            bounds,
            &PlatformEvent::KeyPressed {
                key: Key::Character('f'),
                modifiers: KeyModifiers::from_bits(KeyModifiers::CONTROL),
            },
            &mut c,
        );
        d.set_accessibility_nodes(&snapshot(&v, bounds, &theme).1);
        d.dispatch(
            &v,
            bounds,
            &PlatformEvent::TextInput {
                text: "hello".into(),
            },
            &mut c,
        );
        assert_eq!(v.core.find_text.value(), "hello");
        d.dispatch(
            &v,
            bounds,
            &PlatformEvent::KeyPressed {
                key: Key::Enter,
                modifiers: KeyModifiers::default(),
            },
            &mut c,
        );
        assert!(
            h.commands
                .borrow()
                .iter()
                .any(|command| matches!(command,Command::Find{text,..} if text=="hello"))
        );
        d.dispatch(
            &v,
            bounds,
            &PlatformEvent::KeyPressed {
                key: Key::Escape,
                modifiers: KeyModifiers::default(),
            },
            &mut c,
        );
        assert!(!v.core.finding.get());
    }
    #[test]
    fn download_progress_requires_explicit_acceptance_and_real_completion() {
        let h = Harness::default();
        let v = BrowserView::new(Box::new(h.clone()));
        let (tab, document) = commit(&v, &h, "example.org", Security::Secure);
        h.events.borrow_mut().push_back(Event::DownloadOffered {
            id: 5,
            tab,
            document,
            url: "https://example.org/file".into(),
            name: "report.pdf".into(),
            total: Some(100),
        });
        snapshot(&v, Rect::new(0., 0., 900., 600.), &Theme::LIGHT);
        assert_eq!(
            v.core.browser.borrow().downloads[0].state,
            DownloadState::Offered
        );
        assert!(!h.commands.borrow().contains(&Command::AcceptDownload(5)));
        v.core.perform(Action::AcceptDownload(5));
        assert_eq!(
            h.commands.borrow().last(),
            Some(&Command::AcceptDownload(5))
        );
        h.events.borrow_mut().extend([
            Event::DownloadProgress {
                id: 5,
                received: 25,
            },
            Event::DownloadFailed {
                id: 5,
                message: "Disk full".into(),
            },
            Event::DownloadFinished { id: 5 },
        ]);
        snapshot(&v, Rect::new(0., 0., 900., 600.), &Theme::LIGHT);
        assert_eq!(
            v.core.browser.borrow().downloads[0].state,
            DownloadState::Failed("Disk full".into())
        );
        v.core.perform(Action::ClearDownloads);
        assert!(v.core.browser.borrow().downloads.is_empty());
    }
    #[test]
    fn link_requests_preserve_foreground_and_block_unsolicited_or_stale_popups() {
        let h = Harness::default();
        let v = BrowserView::new(Box::new(h.clone()));
        let (source, document) = commit(&v, &h, "example.org", Security::Secure);
        let request = |generation, user_initiated| Event::OpenTabRequested {
            source,
            document: generation,
            url: "https://second.example/".into(),
            background: true,
            user_initiated,
        };
        h.events
            .borrow_mut()
            .extend([request(document, false), request(document + 1, true)]);
        snapshot(&v, Rect::new(0., 0., 900., 600.), &Theme::LIGHT);
        assert_eq!(v.core.browser.borrow().tabs.len(), 1);
        h.events.borrow_mut().push_back(request(document, true));
        snapshot(&v, Rect::new(0., 0., 900., 600.), &Theme::LIGHT);
        let b = v.core.browser.borrow();
        assert_eq!(b.tabs.len(), 2);
        assert_eq!(b.tab().id, source);
        assert_eq!(b.history.len(), 1);
        assert!(b.tabs[1].document.is_none());
        assert!(h.commands.borrow().iter().any(|command|matches!(command,Command::Navigate{tab,url,..} if *tab!=source && url=="https://second.example/")));
    }
    #[test]
    fn new_panels_use_native_controls_and_keep_close_reachable_at_small_sizes() {
        for theme in [Theme::LIGHT, Theme::DARK] {
            for width in [320., 640., 1100.] {
                let v = BrowserView::new(Box::new(crate::engine::Unavailable));
                for action in [
                    Action::Tabs,
                    Action::ReadingList,
                    Action::Downloads,
                    Action::Settings,
                    Action::AddBookmark,
                    Action::NewGroup,
                ] {
                    v.core.perform(Action::ClosePanel);
                    v.core.perform(action);
                    let (_, nodes) = snapshot(&v, Rect::new(0., 0., width, 480.), &theme);
                    let close = nodes
                        .iter()
                        .find(|n| n.label.as_deref() == Some("Close Sidebar"))
                        .unwrap();
                    assert!(
                        close.bounds.origin.x >= 0.
                            && close.bounds.origin.x + close.bounds.size.width <= width + 0.1
                    );
                }
            }
        }
    }
    #[test]
    fn local_address_suggestions_survive_pointer_focus_and_open_the_chosen_url() {
        use appcore::viewkit::platform::ButtonState;
        let h = Harness::default();
        let v = BrowserView::new(Box::new(h.clone()));
        v.core
            .browser
            .borrow_mut()
            .add_bookmark("Saved Docs", "docs.example.org")
            .unwrap();
        v.core.focus_address();
        v.core.address_state.set_value("docs");
        let bounds = Rect::new(0., 0., 900., 600.);
        let theme = Theme::LIGHT;
        let (_, nodes) = snapshot(&v, bounds, &theme);
        let row = nodes
            .iter()
            .rev()
            .find(|n| n.label.as_deref() == Some("Saved Docs"))
            .expect("local suggestion")
            .bounds;
        let mut dispatcher = EventDispatcher::new();
        let mut text = TextMeasurer::new();
        let mut context = EventContext::new(&theme, &theme.typography, &mut text);
        dispatcher.set_accessibility_nodes(&nodes);
        dispatcher.dispatch(&v, bounds, &PlatformEvent::Focused(true), &mut context);
        dispatcher.dispatch(
            &v,
            bounds,
            &PlatformEvent::PointerMoved {
                x: row.origin.x + 12.,
                y: row.origin.y + row.size.height * 0.5,
            },
            &mut context,
        );
        for state in [ButtonState::Pressed, ButtonState::Released] {
            dispatcher.dispatch(
                &v,
                bounds,
                &PlatformEvent::PointerButton {
                    button: PointerButton::Primary,
                    state,
                },
                &mut context,
            );
        }
        assert!(
            h.commands
                .borrow()
                .iter()
                .any(|c| matches!(c,Command::Navigate{url,..} if url=="https://docs.example.org/"))
        );
        assert!(!v.core.editing_address.get());
        assert!(v.core.browser.borrow().history.is_empty());
    }
    #[test]
    fn tab_context_menu_uses_shared_request_ids_and_keeps_its_original_target() {
        let v = BrowserView::new(Box::new(crate::engine::Unavailable));
        v.core.browser.borrow_mut().tabs[0].draft = "https://first.example/".into();
        v.core.perform(Action::New);
        v.core.browser.borrow_mut().tabs[1].draft = "https://second.example/".into();
        v.core.editing_address.set(false);
        let bounds = Rect::new(0., 0., 900., 600.);
        let theme = Theme::LIGHT;
        let (_, nodes) = snapshot(&v, bounds, &theme);
        let first = nodes
            .iter()
            .find(|n| n.role == AccessibilityRole::Tab)
            .unwrap()
            .bounds;
        let more = nodes
            .iter()
            .find(|n| n.label.as_deref() == Some("More"))
            .unwrap()
            .bounds;
        let mut text = TextMeasurer::new();
        let mut context = EventContext::new(&theme, &theme.typography, &mut text);
        let more_point = Point::new(
            more.origin.x + more.size.width * 0.5,
            more.origin.y + more.size.height * 0.5,
        );
        v.handle_event(
            bounds,
            &ViewEvent::PointerPressed {
                position: more_point,
                button: PointerButton::Primary,
            },
            &mut context,
        );
        v.handle_event(
            bounds,
            &ViewEvent::PointerReleased {
                position: more_point,
                button: PointerButton::Primary,
            },
            &mut context,
        );
        let menu = context.take_context_menu_request().unwrap();
        let point = Point::new(
            first.origin.x + 8.,
            first.origin.y + first.size.height * 0.5,
        );
        v.handle_event(
            bounds,
            &ViewEvent::PointerPressed {
                position: point,
                button: PointerButton::Secondary,
            },
            &mut context,
        );
        let tab_menu = context.take_context_menu_request().unwrap();
        assert_ne!(menu.request_id, tab_menu.request_id);
        v.core.select(1);
        v.handle_event(
            bounds,
            &ViewEvent::ContextMenuResult {
                request_id: tab_menu.request_id,
                command_id: Some(1),
            },
            &mut context,
        );
        assert_eq!(v.core.browser.borrow().tabs.len(), 3);
        assert_eq!(
            v.core.browser.borrow().tab().draft,
            "https://first.example/"
        );
        v.handle_event(
            bounds,
            &ViewEvent::ContextMenuResult {
                request_id: tab_menu.request_id,
                command_id: Some(1),
            },
            &mut context,
        );
        assert_eq!(v.core.browser.borrow().tabs.len(), 3);
    }
    #[test]
    fn storage_errors_remain_visible_after_unrelated_browser_actions() {
        let v = BrowserView::new(Box::new(crate::engine::Unavailable));
        *v.core.storage_error.borrow_mut() =
            Some("Changes not saved: browser data is in use.".into());
        v.core.perform(Action::Settings);
        let (list, _) = snapshot(&v, Rect::new(0., 0., 900., 600.), &Theme::LIGHT);
        assert!(list.commands().iter().any(|c|matches!(c,DrawCommand::DrawText{command} if command.text.starts_with("Changes not saved:"))));
        v.core.perform(Action::New);
        let (list, _) = snapshot(&v, Rect::new(0., 0., 900., 600.), &Theme::LIGHT);
        assert!(list.commands().iter().any(|c|matches!(c,DrawCommand::DrawText{command} if command.text.starts_with("Changes not saved:"))));
    }
}
