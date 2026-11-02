//! Retained native browser chrome; page rendering belongs to the engine adapter.
use crate::{
    browser::{Browser, Entry},
    engine::{Command, Engine, Security, TabId},
};
use appcore::prelude::*;
use appcore::viewkit::{
    event::{EventContext, EventResult, ViewEvent},
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
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Panel {
    None,
    Bookmarks,
    History,
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
    address_ready: Cell<Option<Rect>>,
    buttons: RefCell<BTreeMap<Action, ButtonInteractionState>>,
    rows: RefCell<BTreeMap<String, ButtonInteractionState>>,
    menu: RefCell<Option<(bool, Rc<Picker>)>>,
    last_reveal: Cell<Option<(TabId, u32)>>,
    panel: Cell<Panel>,
    panel_scroll: ScrollState,
    panel_toolbar: ActionToolbarState<Action>,
    feedback: RefCell<Option<String>>,
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
    fn more(&self) -> Rc<Picker> {
        let reopen = self.browser.borrow().can_reopen();
        if let Some((old, menu)) = self.menu.borrow().as_ref() {
            if *old == reopen {
                return menu.clone();
            }
        }
        let mut menu = Picker::new("More").size(ControlSize::Small);
        for (label, action) in [
            ("New Tab", Action::New),
            ("Reopen Closed Tab", Action::Reopen),
            ("Bookmarks", Action::Bookmarks),
            ("History", Action::History),
        ] {
            if action == Action::Reopen && !reopen {
                menu = menu.disabled_option(label);
            } else {
                let pending = self.pending.clone();
                menu = menu.option(label, move || enqueue(&pending, action.clone()));
            }
        }
        let menu = Rc::new(menu);
        *self.menu.borrow_mut() = Some((reopen, menu.clone()));
        menu
    }
    fn sync_address(&self) {
        let b = self.browser.borrow();
        self.selected.set_if_changed(b.active);
        self.address_state.set_value(b.tab().draft.clone());
    }
    fn focus_address(&self) {
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
            let editing = self
                .address_state
                .is_focused()
                .then(|| (self.browser.borrow().tab().id, self.address_state.value()));
            self.browser.borrow_mut().apply(event);
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
            Action::Open(url) => {
                let request = self.browser.borrow_mut().navigate(&url);
                match request {
                    Ok(command) => {
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
        let id = {
            let mut b = self.browser.borrow_mut();
            if index >= b.tabs.len() || index == b.active {
                return;
            }
            b.active = index;
            b.tab().id
        };
        self.lifecycle(Command::Activate(id));
        self.address_state.set_focused(false);
        self.focus_address.set(false);
        self.address_ready.set(None);
        self.sync_address();
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
                .size(TextFieldSize::Small)
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
            address_ready: Cell::new(None),
            buttons: RefCell::new(BTreeMap::new()),
            rows: RefCell::new(BTreeMap::new()),
            menu: RefCell::new(None),
            last_reveal: Cell::new(None),
            panel: Cell::new(Panel::None),
            panel_scroll: ScrollState::new(),
            panel_toolbar: ActionToolbarState::new(),
            feedback: RefCell::new(None),
        });
        core.lifecycle(Command::Create(1));
        core.lifecycle(Command::Activate(1));
        Self { core }
    }
    /// Registers ordinary application menus through AppCore.
    pub fn with_menus(&self) -> ApplicationMenuBar<Self> {
        let item = |label: &str, action: Action| {
            let pending = self.core.pending.clone();
            ApplicationMenuItem::new(label, move || enqueue(&pending, action.clone()))
        };
        let close_pending = self.core.pending.clone();
        let weak = Rc::downgrade(&self.core);
        ApplicationMenuBar::new(self.clone())
            .menu(
                ApplicationMenu::new("File")
                    .item(
                        item("New Tab", Action::New).shortcut(MenuShortcut::command('t', "Ctrl+T")),
                    )
                    .item(
                        ApplicationMenuItem::new("Close Tab", move || {
                            if let Some(core) = weak.upgrade() {
                                enqueue(
                                    &close_pending,
                                    Action::Close(core.browser.borrow().tab().id),
                                );
                            }
                        })
                        .shortcut(MenuShortcut::command('w', "Ctrl+W")),
                    )
                    .item(
                        item("Reopen Closed Tab", Action::Reopen)
                            .shortcut(MenuShortcut::command('t', "Ctrl+Shift+T").shift()),
                    )
                    .separator()
                    .item(ApplicationMenuItem::new(
                        "Close Window",
                        request_close_key_window,
                    )),
            )
            .menu(ApplicationMenu::standard_edit())
            .menu(
                ApplicationMenu::new("View")
                    .item(item("Reload", Action::Reload))
                    .item(item("Bookmarks", Action::Bookmarks))
                    .item(item("History", Action::History)),
            )
    }
    fn tabstrip(&self, theme: &Theme) -> HStack {
        let b = self.core.browser.borrow();
        let mut tabs = Tabs::with_interaction(self.core.selected.binding(), self.core.tabs.clone())
            .document_style()
            .item_width(theme.layout.tab_width)
            .accessibility_label("Browser tabs");
        for (index, tab) in b.tabs.iter().enumerate() {
            let pending = self.core.pending.clone();
            let id = tab.id;
            tabs = tabs.closable_item(index, tab.title(), move || {
                enqueue(&pending, Action::Close(id))
            });
        }
        HStack::new()
            .alignment(StackAlignment::Center)
            .gap(StackGap::ExtraSmall)
            .child(
                Scroll::new(self.core.tab_scroll.clone())
                    .axis(ScrollAxis::Horizontal)
                    .content(tabs)
                    .layout()
                    .flex_grow(1.0),
            )
            .child(
                self.core
                    .icon("New Tab (Ctrl+T)", SymbolName::Plus, Action::New)
                    .layout()
                    .flex_shrink(0.0),
            )
    }
    fn navigation(&self) -> Toolbar<HStack> {
        let b = self.core.browser.borrow();
        let t = b.tab();
        let available = self.core.engine.borrow().available();
        let reload = if t.loading {
            self.core.icon("Stop Loading", SymbolName::X, Action::Stop)
        } else {
            self.core
                .icon("Reload", SymbolName::Refresh, Action::Reload)
                .enabled(available && !t.draft.is_empty())
        };
        let bookmarked = t
            .document
            .as_ref()
            .is_some_and(|d| b.bookmarks.iter().any(|e| e.url == d.url));
        Toolbar::new(
            HStack::new()
                .alignment(StackAlignment::Center)
                .gap(StackGap::ExtraSmall)
                .child(
                    self.core
                        .icon("Back", SymbolName::ArrowBackward, Action::Back)
                        .enabled(available && t.can_back),
                )
                .child(
                    self.core
                        .icon("Forward", SymbolName::ArrowForward, Action::Forward)
                        .enabled(available && t.can_forward),
                )
                .child(reload)
                .child(Address(self.core.clone()).layout().flex_grow(1.0))
                .child(
                    self.core
                        .icon(
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
                        .enabled(t.document.is_some()),
                )
                .child(self.core.more()),
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
        let bookmarks = self.core.panel.get() == Panel::Bookmarks;
        let entries: Vec<Entry> = {
            let b = self.core.browser.borrow();
            if bookmarks {
                b.bookmarks.clone()
            } else {
                b.history.clone()
            }
        };
        self.core
            .rows
            .borrow_mut()
            .retain(|url, _| entries.iter().any(|e| &e.url == url));
        self.core
            .buttons
            .borrow_mut()
            .retain(|action, _| match action {
                Action::RemoveBookmark(url) => bookmarks && entries.iter().any(|e| &e.url == url),
                _ => true,
            });
        let mut toolbar = ActionToolbar::new(self.core.panel_toolbar.clone())
            .title(if bookmarks { "Bookmarks" } else { "History" })
            .trailing(
                Action::ClosePanel,
                self.core
                    .icon("Close Sidebar", SymbolName::X, Action::ClosePanel),
            );
        if !bookmarks {
            toolbar = toolbar.leading(
                Action::ClearHistory,
                self.core
                    .button("Clear", None, Action::ClearHistory)
                    .enabled(!entries.is_empty()),
            );
        }
        let mut rows = VStack::new()
            .alignment(StackAlignment::Stretch)
            .gap(StackGap::ExtraSmall);
        if entries.is_empty() {
            rows = rows.child(
                Padding::all(Theme::current().spacing.large).content(
                    Text::body(if bookmarks {
                        "No bookmarks yet."
                    } else {
                        "No browsing history in this session."
                    })
                    .tone(TextTone::Secondary),
                ),
            );
        }
        for entry in entries {
            let interaction = self
                .core
                .rows
                .borrow_mut()
                .entry(entry.url.clone())
                .or_default()
                .clone();
            let pending = self.core.pending.clone();
            let url = entry.url.clone();
            let mut row = HStack::new()
                .gap(StackGap::Small)
                .alignment(StackAlignment::Center)
                .child(
                    ListRow::with_interaction(interaction, &entry.title)
                        .subtitle(&entry.url)
                        .on_select(move || enqueue(&pending, Action::Open(url.clone())))
                        .layout()
                        .flex_grow(1.0),
                );
            if bookmarks {
                row = row.child(self.core.icon(
                    "Remove Bookmark",
                    SymbolName::X,
                    Action::RemoveBookmark(entry.url),
                ));
            }
            rows = rows.child(row);
        }
        VStack::new()
            .alignment(StackAlignment::Stretch)
            .gap(StackGap::None)
            .child(Toolbar::new(toolbar))
            .child(
                Scroll::new(self.core.panel_scroll.clone())
                    .content(rows)
                    .layout()
                    .flex_grow(1.0),
            )
    }
    fn content(&self, theme: &Theme, width: f32) -> Surface<VStack> {
        let mut layout = VStack::new()
            .alignment(StackAlignment::Stretch)
            .gap(StackGap::None)
            .child(
                self.tabstrip(theme)
                    .height(theme.layout.top_bar_height)
                    .flex_shrink(0.0),
            )
            .child(self.navigation().layout().flex_shrink(0.0));
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
            .unwrap_or_else(|| security.into());
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
struct Address(Rc<Core>);
impl View for Address {
    fn measure(&self, c: Constraints, m: &mut MeasureContext<'_>) -> Size {
        self.0.address.measure(c, m)
    }
    fn paint(&self, b: Rect, c: &mut PaintContext<'_>) {
        if self.0.focus_address.get() {
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
        let browser = self.core.browser.borrow();
        let reveal = (browser.tab().id, b.size.width.to_bits());
        if self.core.last_reveal.get() != Some(reveal) {
            let button = self
                .core
                .icon("New Tab (Ctrl+T)", SymbolName::Plus, Action::New);
            let size = button.measure(
                Constraints::loose(b.size),
                &mut MeasureContext {
                    theme: c.theme,
                    typography: c.typography,
                    text_measurer: c.text_measurer,
                },
            );
            let visible = (b.size.width - size.width - c.theme.spacing.extra_small).max(0.0);
            let left = browser.active as f32 * c.theme.layout.tab_width;
            let offset = self.core.tab_scroll.offset_x();
            if left < offset {
                self.core.tab_scroll.set_offset(left, 0.);
            } else if left + c.theme.layout.tab_width > offset + visible {
                self.core
                    .tab_scroll
                    .set_offset((left + c.theme.layout.tab_width - visible).max(0.), 0.);
            }
            self.core.last_reveal.set(Some(reveal));
        }
        drop(browser);
        self.content(c.theme, b.size.width).paint(b, c);
    }
    fn handle_event(&self, b: Rect, e: &ViewEvent, c: &mut EventContext<'_>) -> EventResult {
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
                        let len = b.tabs.len();
                        let index = if modifiers.shift() {
                            (b.active + len - 1) % len
                        } else {
                            (b.active + 1) % len
                        };
                        drop(b);
                        self.core.select(index);
                        c.request_redraw();
                        return EventResult::Consumed;
                    }
                    _ => None,
                }
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
        let result = self.content(c.theme(), b.size.width).handle_event(b, e, c);
        {
            let mut browser = self.core.browser.borrow_mut();
            let active = browser.active;
            browser.tabs[active].draft = self.core.address_state.value();
        }
        self.core.select(self.core.selected.get());
        self.core.drain();
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
    }
    impl Engine for Harness {
        fn available(&self) -> bool {
            true
        }
        fn send(&mut self, c: Command) -> Result<(), String> {
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
                let (_, nodes) = snapshot(&v, b, &theme);
                let field = nodes
                    .iter()
                    .find(|n| n.role == AccessibilityRole::TextField)
                    .unwrap()
                    .bounds;
                assert!(field.size.width > 0. && field.size.height > 0.);
                assert!(field.origin.x >= 0. && field.origin.x + field.size.width <= width + 0.1);
                for label in ["Back", "Forward", "Reload", "Bookmark Page", "More"] {
                    let node = nodes
                        .iter()
                        .find(|n| n.label.as_deref() == Some(label))
                        .unwrap();
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
        let request = c.take_context_menu_request().expect("standard Picker menu");
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
        snapshot(&v, b, &theme);
        assert!(v.core.tab_scroll.offset_x() > 0.);
        let old = v.core.tab_scroll.offset_x();
        snapshot(&v, b, &theme);
        assert_eq!(v.core.tab_scroll.offset_x(), old);
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
}
