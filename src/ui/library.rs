//! Native browser libraries and page tools. All page operations cross Engine.
use super::*;
impl Core {
    pub(super) fn tab_options(&self) -> Vec<(&'static str, Action, bool)> {
        let b = self.browser.borrow();
        let visible = b.visible_tabs();
        let position = visible.iter().position(|i| *i == b.active).unwrap_or(0);
        let left = position > 0 && b.tabs[visible[position - 1]].pinned == b.tab().pinned;
        let right =
            position + 1 < visible.len() && b.tabs[visible[position + 1]].pinned == b.tab().pinned;
        vec![
            ("Duplicate Tab", Action::Duplicate, true),
            (
                if b.tab().pinned {
                    "Unpin Tab"
                } else {
                    "Pin Tab"
                },
                Action::Pin,
                true,
            ),
            ("Move Tab Left", Action::Move(-1), left),
            ("Move Tab Right", Action::Move(1), right),
            (
                "Close Other Tabs",
                Action::ClosePeers(false),
                !b.peer_tabs(false).is_empty(),
            ),
            (
                "Close Tabs to the Right",
                Action::ClosePeers(true),
                !b.peer_tabs(true).is_empty(),
            ),
            ("Close Tab", Action::Close(b.tab().id), true),
        ]
    }
    fn tab_actions_menu(&self) -> Rc<MenuButton> {
        let options = self.tab_options();
        let key = format!(
            "{}:{:?}",
            self.browser.borrow().tab().id,
            options
                .iter()
                .map(|(label, _, enabled)| (*label, *enabled))
                .collect::<Vec<_>>()
        );
        if let Some((old, menu)) = self.tab_menu.borrow().as_ref() {
            if old == &key {
                return menu.clone();
            }
        }
        let mut menu = MenuButton::new("Tab Actions", SymbolName::More);
        for (label, action, enabled) in options {
            if enabled {
                let pending = self.pending.clone();
                menu = menu.option(label, move || enqueue(&pending, action.clone()));
            } else {
                menu = menu.disabled_option(label);
            }
        }
        let menu = Rc::new(menu);
        *self.tab_menu.borrow_mut() = Some((key, menu.clone()));
        menu
    }
    pub(super) fn open_link_tab(&self, source: TabId, input: &str, background: bool) {
        let url = match crate::browser::normalize_address(input) {
            Ok(url) => url,
            Err(e) => {
                *self.feedback.borrow_mut() = Some(e);
                return;
            }
        };
        let (old, group) = {
            let b = self.browser.borrow();
            (
                b.tab().id,
                b.tabs
                    .iter()
                    .find(|t| t.id == source)
                    .map(|t| t.group.clone()),
            )
        };
        let Some(group) = group else {
            return;
        };
        let result = self.browser.borrow_mut().new_tab();
        match result {
            Ok(id) => {
                {
                    let mut b = self.browser.borrow_mut();
                    let active = b.active;
                    b.tabs[active].group = group;
                }
                self.lifecycle(Command::Create(id));
                if !background {
                    self.lifecycle(Command::Activate(id));
                }
                let request = self.browser.borrow_mut().navigate(&url);
                if let Ok(command) = request {
                    self.send(command);
                }
                if background {
                    let index = self
                        .browser
                        .borrow()
                        .tabs
                        .iter()
                        .position(|t| t.id == old)
                        .unwrap();
                    self.browser.borrow_mut().active = index;
                } else {
                    self.editing_address.set(false);
                    self.address_state.set_focused(false);
                }
                self.sync_address();
                self.schedule_save();
            }
            Err(e) => *self.feedback.borrow_mut() = Some(e),
        }
    }
    pub(super) fn page_command(&self, command: Command) -> bool {
        match self.engine.borrow_mut().send(command) {
            Ok(()) => true,
            Err(error) => {
                *self.feedback.borrow_mut() = Some(error);
                false
            }
        }
    }
    fn show_panel(&self, panel: Panel) {
        self.panel.set(if self.panel.get() == panel {
            Panel::None
        } else {
            panel
        });
        self.library_query.set_value(String::new());
        self.panel_scroll.reset();
    }
    pub(super) fn library_action(&self, action: Action) {
        match action {
            Action::NewGroup => {
                self.bookmark_title.set_value(String::new());
                self.panel.set(Panel::GroupEditor);
            }
            Action::CreateGroup => {
                let result = self
                    .browser
                    .borrow_mut()
                    .create_group(&self.bookmark_title.value());
                match result {
                    Ok(id) => {
                        self.lifecycle(Command::Create(id));
                        self.lifecycle(Command::Activate(id));
                        self.sync_address();
                        self.panel.set(Panel::Tabs);
                    }
                    Err(e) => *self.feedback.borrow_mut() = Some(e),
                }
            }
            Action::SelectGroup(group) => {
                let index = self
                    .browser
                    .borrow()
                    .tabs
                    .iter()
                    .position(|t| t.group == group);
                if let Some(index) = index {
                    self.select(index);
                } else if self.browser.borrow().groups.contains(&group) {
                    let result = self.browser.borrow_mut().new_tab();
                    if let Err(error) = &result {
                        *self.feedback.borrow_mut() = Some(error.clone());
                    }
                    if let Ok(id) = result {
                        let mut b = self.browser.borrow_mut();
                        let active = b.active;
                        b.tabs[active].group = group;
                        drop(b);
                        self.lifecycle(Command::Create(id));
                        self.lifecycle(Command::Activate(id));
                        self.sync_address();
                    }
                }
            }
            Action::Duplicate => {
                let result = self.browser.borrow_mut().duplicate();
                match result {
                    Ok(id) => {
                        self.lifecycle(Command::Create(id));
                        self.lifecycle(Command::Activate(id));
                        self.sync_address();
                        let url = self.browser.borrow().tab().draft.clone();
                        if !url.is_empty() {
                            self.perform(Action::Open(url));
                        } else {
                            self.focus_address();
                        }
                    }
                    Err(e) => *self.feedback.borrow_mut() = Some(e),
                }
            }
            Action::Pin => {
                self.browser.borrow_mut().toggle_pin();
                self.sync_address();
            }
            Action::Move(delta) => {
                self.browser.borrow_mut().move_tab(delta);
                self.sync_address();
            }
            Action::ClosePeers(right) => {
                let ids = self.browser.borrow().peer_tabs(right);
                for id in ids {
                    self.perform(Action::Close(id));
                }
            }
            Action::SelectTab(id) => {
                let index = self.browser.borrow().tabs.iter().position(|t| t.id == id);
                if let Some(index) = index {
                    self.select(index);
                    self.panel.set(Panel::None);
                }
            }
            Action::ReadingList => self.show_panel(Panel::ReadingList),
            Action::Downloads => self.show_panel(Panel::Downloads),
            Action::Tabs => self.show_panel(Panel::Tabs),
            Action::Settings => self.show_panel(Panel::Settings),
            Action::ToggleReading => self.browser.borrow_mut().toggle_reading(),
            Action::RemoveReading(url) => self
                .browser
                .borrow_mut()
                .reading_list
                .retain(|e| e.url != url),
            Action::SearchProvider(value) => {
                self.browser.borrow_mut().search = match value {
                    1 => SearchProvider::Google,
                    2 => SearchProvider::DuckDuckGo,
                    3 => SearchProvider::Bing,
                    _ => SearchProvider::None,
                }
            }
            Action::ToggleRestore => {
                let mut b = self.browser.borrow_mut();
                b.restore_session = !b.restore_session;
            }
            Action::AddBookmark | Action::EditBookmark(_) => {
                let entry = if let Action::EditBookmark(url) = &action {
                    self.browser
                        .borrow()
                        .bookmarks
                        .iter()
                        .find(|e| &e.url == url)
                        .cloned()
                } else {
                    self.browser.borrow().tab().document.clone()
                };
                *self.bookmark_original.borrow_mut() = if matches!(action, Action::EditBookmark(_))
                {
                    entry.as_ref().map(|e| e.url.clone())
                } else {
                    None
                };
                self.bookmark_title
                    .set_value(entry.as_ref().map(|e| e.title.clone()).unwrap_or_default());
                self.bookmark_url
                    .set_value(entry.map(|e| e.url).unwrap_or_default());
                self.panel.set(Panel::BookmarkEditor);
            }
            Action::SaveBookmark => {
                let result = self
                    .browser
                    .borrow_mut()
                    .add_bookmark(&self.bookmark_title.value(), &self.bookmark_url.value());
                match result {
                    Ok(()) => {
                        let url =
                            crate::browser::normalize_address(&self.bookmark_url.value()).unwrap();
                        if let Some(old) = self.bookmark_original.borrow_mut().take() {
                            if old != url {
                                self.browser.borrow_mut().bookmarks.retain(|e| e.url != old);
                            }
                        }
                        self.panel.set(Panel::Bookmarks);
                    }
                    Err(e) => *self.feedback.borrow_mut() = Some(e),
                }
            }
            Action::Find => {
                let b = self.browser.borrow();
                if self.engine.borrow().available()
                    && self.engine.borrow().features().find
                    && b.document_id() != 0
                {
                    self.editing_address.set(false);
                    self.address_state.set_focused(false);
                    self.focus_address.set(false);
                    self.address_ready.set(None);
                    self.address_bounds.set(None);
                    self.finding.set(true);
                    self.find_focus.set(true);
                    self.find_ready.set(None);
                    self.find_text.set_focused(true);
                } else {
                    *self.feedback.borrow_mut() =
                        Some("Find on Page requires a connected engine and a loaded page.".into());
                }
            }
            Action::FindNext(backwards) => {
                let b = self.browser.borrow();
                if !self.engine.borrow().features().find || b.document_id() == 0 {
                    return;
                }
                let text = self.find_text.value();
                if text.is_empty() {
                    drop(b);
                    self.library_action(Action::CloseFind);
                    return;
                }
                let request = self.next_find.get();
                let Some(next) = request.checked_add(1) else {
                    return;
                };
                self.next_find.set(next);
                let command = Command::Find {
                    tab: b.tab().id,
                    document: b.document_id(),
                    request,
                    text: text.chars().take(4096).collect(),
                    backwards,
                };
                drop(b);
                if self.page_command(command) {
                    let mut b = self.browser.borrow_mut();
                    let active = b.active;
                    b.tabs[active].find = Some((request, 0, 0));
                    b.tabs[active].find_pending = true;
                }
            }
            Action::CloseFind => {
                {
                    let mut b = self.browser.borrow_mut();
                    let active = b.active;
                    b.tabs[active].find = None;
                    b.tabs[active].find_pending = false;
                }
                self.finding.set(false);
                self.find_focus.set(false);
                self.find_ready.set(None);
                self.find_text.set_focused(false);
                let b = self.browser.borrow();
                if b.document_id() != 0 && self.engine.borrow().features().find {
                    let command = Command::ClearFind {
                        tab: b.tab().id,
                        document: b.document_id(),
                    };
                    drop(b);
                    self.page_command(command);
                }
            }
            Action::Reader | Action::Print => {
                let b = self.browser.borrow();
                let features = self.engine.borrow().features();
                if b.document_id() == 0 || !self.engine.borrow().available() {
                    return;
                }
                let command = if action == Action::Reader && features.reader {
                    Command::ToggleReader {
                        tab: b.tab().id,
                        document: b.document_id(),
                    }
                } else if action == Action::Print && features.printing {
                    Command::Print {
                        tab: b.tab().id,
                        document: b.document_id(),
                    }
                } else {
                    return;
                };
                drop(b);
                self.page_command(command);
            }
            Action::Zoom(delta) => {
                let b = self.browser.borrow();
                if !self.engine.borrow().features().zoom || b.document_id() == 0 {
                    return;
                }
                let percent = if delta == 0 {
                    100
                } else {
                    (b.tab().zoom as i16 + delta).clamp(50, 300) as u16
                };
                let command = Command::SetZoom {
                    tab: b.tab().id,
                    document: b.document_id(),
                    percent,
                };
                drop(b);
                if self.page_command(command) {
                    let mut b = self.browser.borrow_mut();
                    let active = b.active;
                    b.tabs[active].zoom = percent;
                }
            }
            Action::AcceptDownload(id) | Action::CancelDownload(id) => {
                if !self.engine.borrow().available() || !self.engine.borrow().features().downloads {
                    return;
                }
                let accept = matches!(action, Action::AcceptDownload(_));
                let valid = self.browser.borrow().downloads.iter().any(|d| {
                    d.id == id
                        && if accept {
                            d.state == DownloadState::Offered
                        } else {
                            matches!(d.state, DownloadState::Offered | DownloadState::Active)
                        }
                });
                if valid
                    && self.page_command(if accept {
                        Command::AcceptDownload(id)
                    } else {
                        Command::CancelDownload(id)
                    })
                {
                    if let Some(d) = self
                        .browser
                        .borrow_mut()
                        .downloads
                        .iter_mut()
                        .find(|d| d.id == id)
                    {
                        d.state = if accept {
                            DownloadState::Active
                        } else {
                            DownloadState::Cancelled
                        };
                    }
                }
            }
            Action::ClearDownloads => self
                .browser
                .borrow_mut()
                .downloads
                .retain(|d| matches!(d.state, DownloadState::Offered | DownloadState::Active)),
            _ => {}
        }
    }
}
impl Drop for Core {
    fn drop(&mut self) {
        self.persist();
    }
}
impl BrowserView {
    pub(super) fn library_panel(&self) -> VStack {
        let panel = self.core.panel.get();
        let title = match panel {
            Panel::Bookmarks => "Bookmarks",
            Panel::History => "History",
            Panel::ReadingList => "Reading List",
            Panel::Tabs => "Tabs",
            Panel::Downloads => "Downloads",
            Panel::Settings => "Browser Settings",
            Panel::BookmarkEditor => "Bookmark",
            Panel::GroupEditor => "New Tab Group",
            Panel::None => "",
        };
        let mut toolbar = ActionToolbar::new(self.core.panel_toolbar.clone())
            .title(title)
            .trailing(
                Action::ClosePanel,
                self.core
                    .icon("Close Sidebar", SymbolName::X, Action::ClosePanel),
            );
        if panel == Panel::Bookmarks {
            toolbar = toolbar.leading(
                Action::AddBookmark,
                self.core
                    .icon("Add Bookmark", SymbolName::Plus, Action::AddBookmark),
            );
        }
        if panel == Panel::History {
            toolbar = toolbar.leading(
                Action::ClearHistory,
                self.core
                    .button("Clear", None, Action::ClearHistory)
                    .enabled(!self.core.browser.borrow().history.is_empty()),
            );
        }
        let mut body = VStack::new()
            .alignment(StackAlignment::Stretch)
            .gap(StackGap::Small);
        if panel == Panel::ReadingList {
            let b = self.core.browser.borrow();
            let saved = b
                .tab()
                .document
                .as_ref()
                .is_some_and(|e| b.reading_list.iter().any(|r| r.url == e.url));
            body = body.child(
                self.core
                    .button(
                        if saved {
                            "Remove Current Page"
                        } else {
                            "Add Current Page"
                        },
                        None,
                        Action::ToggleReading,
                    )
                    .enabled(b.document_id() != 0),
            );
        }
        match panel {
            Panel::GroupEditor => {
                body = body
                    .child(Text::label("Group name"))
                    .child(
                        TextField::with_interaction(self.core.bookmark_title.clone())
                            .placeholder("Work, Research, …"),
                    )
                    .child(
                        self.core
                            .button("Create Tab Group", None, Action::CreateGroup)
                            .style(ButtonStyle::Primary),
                    );
            }
            Panel::BookmarkEditor => {
                body = body
                    .child(Text::label("Name"))
                    .child(
                        TextField::with_interaction(self.core.bookmark_title.clone())
                            .placeholder("Bookmark name"),
                    )
                    .child(Text::label("Address"))
                    .child(
                        TextField::with_interaction(self.core.bookmark_url.clone())
                            .placeholder("https://example.org"),
                    )
                    .child(
                        self.core
                            .button("Save Bookmark", None, Action::SaveBookmark)
                            .style(ButtonStyle::Primary),
                    );
            }
            Panel::Settings => {
                let b = self.core.browser.borrow();
                let mut provider = Picker::new(b.search.label());
                for (index, label) in ["Address only", "Google", "DuckDuckGo", "Bing"]
                    .into_iter()
                    .enumerate()
                {
                    let pending = self.core.pending.clone();
                    provider = provider.option(label, move || {
                        enqueue(&pending, Action::SearchProvider(index as u8))
                    });
                }
                body=body.child(Text::label("Search engine")).child(provider)
                    .child(Text::caption("Searches are sent only when you submit. No online suggestions are requested.").tone(TextTone::Secondary))
                    .child(self.core.button(if b.restore_session {"Restore Previous Tabs: On"} else {"Restore Previous Tabs: Off"},None,Action::ToggleRestore))
                    .child(Text::caption("Saved tabs reopen as addresses. A page is loaded only by the connected browsing engine.").tone(TextTone::Secondary));
            }
            Panel::Downloads => {
                body=body.child(Text::caption("Downloads require a connected engine. File saving uses the system file broker.").tone(TextTone::Secondary));
                let b = self.core.browser.borrow();
                for download in b.downloads.iter().rev() {
                    let status = match &download.state {
                        DownloadState::Offered => "Ready to download".into(),
                        DownloadState::Active => format!(
                            "{} bytes received{}",
                            download.received,
                            download
                                .total
                                .map(|n| format!(" of {n}"))
                                .unwrap_or_default()
                        ),
                        DownloadState::Finished => "Complete".into(),
                        DownloadState::Failed(e) => format!("Failed: {e}"),
                        DownloadState::Cancelled => "Cancelled".into(),
                    };
                    body = body
                        .child(Text::label(&download.name))
                        .child(
                            Text::caption(&download.url)
                                .single_line(true)
                                .tone(TextTone::Secondary),
                        )
                        .child(Text::caption(status));
                    match download.state {
                        DownloadState::Offered => {
                            body = body.child(
                                self.core
                                    .button("Save…", None, Action::AcceptDownload(download.id))
                                    .enabled(self.core.engine.borrow().features().downloads),
                            );
                        }
                        DownloadState::Active => {
                            if let Some(total) = download.total.filter(|t| *t > 0) {
                                body = body.child(ProgressBar::new(
                                    download.received as f32 / total as f32,
                                ));
                            }
                        }
                        _ => {}
                    }
                    if matches!(
                        download.state,
                        DownloadState::Offered | DownloadState::Active
                    ) {
                        body = body.child(self.core.button(
                            "Cancel",
                            None,
                            Action::CancelDownload(download.id),
                        ));
                    }
                }
                if b.downloads.is_empty() {
                    body = body.child(Text::body("No downloads."));
                }
                body = body.child(
                    self.core
                        .button("Clear Finished", None, Action::ClearDownloads),
                );
            }
            _ => {
                if panel == Panel::Tabs {
                    let b = self.core.browser.borrow();
                    let mut groups = Picker::new(&b.tab().group);
                    for group in &b.groups {
                        let pending = self.core.pending.clone();
                        let group = group.clone();
                        groups = groups.option(group.clone(), move || {
                            enqueue(&pending, Action::SelectGroup(group.clone()))
                        });
                    }
                    body = body
                        .child(
                            HStack::new()
                                .gap(StackGap::Small)
                                .child(groups.layout().flex_grow(1.0))
                                .child(self.core.tab_actions_menu()),
                        )
                        .child(self.core.button("New Tab Group…", None, Action::NewGroup));
                }
                body = body.child(
                    TextField::with_interaction(self.core.library_query.clone())
                        .placeholder("Search this list"),
                );
                let query = self.core.library_query.value().to_lowercase();
                let b = self.core.browser.borrow();
                let entries: Vec<(Entry, Action)> = if panel == Panel::Tabs {
                    b.tabs
                        .iter()
                        .filter(|t| t.group == b.tab().group)
                        .map(|t| {
                            (
                                Entry {
                                    title: format!(
                                        "{}{}",
                                        if t.pinned { "Pinned · " } else { "" },
                                        t.title()
                                    ),
                                    url: t
                                        .document
                                        .as_ref()
                                        .map(|e| e.url.clone())
                                        .unwrap_or_else(|| t.draft.clone()),
                                },
                                Action::SelectTab(t.id),
                            )
                        })
                        .collect()
                } else {
                    let entries = match panel {
                        Panel::ReadingList => &b.reading_list,
                        Panel::History => &b.history,
                        _ => &b.bookmarks,
                    };
                    let mut entries = entries.clone();
                    if panel == Panel::History {
                        entries.reverse();
                    }
                    entries
                        .into_iter()
                        .map(|e| {
                            let action = Action::Open(e.url.clone());
                            (e, action)
                        })
                        .collect()
                };
                let mut count = 0;
                for (entry, action) in entries.into_iter().filter(|(e, _)| {
                    query.is_empty()
                        || e.title.to_lowercase().contains(&query)
                        || e.url.to_lowercase().contains(&query)
                }) {
                    count += 1;
                    let pending = self.core.pending.clone();
                    let key = format!("{title}:{}", entry.url);
                    let interaction = self.core.rows.borrow_mut().entry(key).or_default().clone();
                    let mut row = HStack::new().gap(StackGap::ExtraSmall).child(
                        ListRow::with_interaction(interaction, &entry.title)
                            .subtitle(&entry.url)
                            .single_line(true)
                            .on_select(move || enqueue(&pending, action.clone()))
                            .layout()
                            .flex_grow(1.0),
                    );
                    if panel == Panel::Bookmarks {
                        row = row
                            .child(
                                self.core
                                    .navigation_icon(
                                        "Edit Bookmark",
                                        SymbolName::Pencil,
                                        Action::EditBookmark(entry.url.clone()),
                                    )
                                    .layout()
                                    .flex_shrink(0.0),
                            )
                            .child(
                                self.core
                                    .navigation_icon(
                                        "Remove Bookmark",
                                        SymbolName::X,
                                        Action::RemoveBookmark(entry.url),
                                    )
                                    .layout()
                                    .flex_shrink(0.0),
                            );
                    } else if panel == Panel::ReadingList {
                        row = row.child(
                            self.core
                                .navigation_icon(
                                    "Remove from Reading List",
                                    SymbolName::X,
                                    Action::RemoveReading(entry.url),
                                )
                                .layout()
                                .flex_shrink(0.0),
                        );
                    }
                    body = body.child(row);
                }
                if count == 0 {
                    body = body.child(
                        Text::body(if query.is_empty() {
                            "Nothing saved here yet."
                        } else {
                            "No matching pages."
                        })
                        .tone(TextTone::Secondary),
                    );
                }
            }
        }
        VStack::new()
            .alignment(StackAlignment::Stretch)
            .gap(StackGap::None)
            .child(Toolbar::new(toolbar))
            .child(
                Scroll::new(self.core.panel_scroll.clone())
                    .content(Padding::all(Theme::current().spacing.small).content(body))
                    .layout()
                    .flex_grow(1.0),
            )
    }
    pub(super) fn start_page(&self) -> Box<dyn View> {
        let b = self.core.browser.borrow();
        let mut content = VStack::new()
            .alignment(StackAlignment::Stretch)
            .gap(StackGap::Medium)
            .child(Text::styled("Start Page", TextRole::TitleLarge))
            .child(self.core.button(
                "Search or Enter Address",
                Some(SymbolName::Search),
                Action::EditAddress,
            ));
        if !b.tab().draft.is_empty() {
            content = content
                .child(self.core.button(
                    "Open Saved Page",
                    None,
                    Action::Open(b.tab().draft.clone()),
                ))
                .child(
                    Text::caption(&b.tab().draft)
                        .single_line(true)
                        .tone(TextTone::Secondary),
                );
        }
        content = content.child(
            HStack::new()
                .gap(StackGap::Small)
                .child(self.core.button("Bookmarks", None, Action::Bookmarks))
                .child(self.core.button("Reading List", None, Action::ReadingList))
                .child(self.core.button("History", None, Action::History)),
        );
        if b.bookmarks.is_empty() {
            content = content
                .child(Text::body("Keep your favourite websites here.").tone(TextTone::Secondary))
                .child(self.core.button(
                    "Add a Bookmark…",
                    Some(SymbolName::Plus),
                    Action::AddBookmark,
                ));
        } else {
            for entry in b.bookmarks.iter().take(8) {
                let pending = self.core.pending.clone();
                let url = entry.url.clone();
                content = content.child(
                    ListRow::with_interaction(
                        self.core
                            .rows
                            .borrow_mut()
                            .entry(format!("start:{}", entry.url))
                            .or_default()
                            .clone(),
                        &entry.title,
                    )
                    .subtitle(&entry.url)
                    .single_line(true)
                    .on_select(move || enqueue(&pending, Action::Open(url.clone()))),
                );
            }
        }
        if !self.core.engine.borrow().available() {
            content = content.child(
                Text::caption("The browsing engine is not connected. Pages cannot load yet.")
                    .tone(TextTone::Secondary),
            );
        }
        Box::new(Scroll::vertical(ContentArea::pane(content)))
    }
    pub(super) fn find_bar(&self) -> Toolbar<HStack> {
        let field = FindInput(self.core.clone());
        let b = self.core.browser.borrow();
        let status = if b.tab().find_pending {
            "Searching…".into()
        } else {
            b.tab()
                .find
                .map(|(_, current, total)| {
                    if total == 0 {
                        "No matches".into()
                    } else {
                        format!("{current} of {total}")
                    }
                })
                .unwrap_or_else(|| "Press Return to find".into())
        };
        Toolbar::new(
            HStack::new()
                .gap(StackGap::Small)
                .child(field.layout().flex_grow(1.0))
                .child(Text::caption(status))
                .child(self.core.navigation_icon(
                    "Previous Match",
                    SymbolName::ArrowBackward,
                    Action::FindNext(true),
                ))
                .child(self.core.navigation_icon(
                    "Next Match",
                    SymbolName::ArrowForward,
                    Action::FindNext(false),
                ))
                .child(
                    self.core
                        .navigation_icon("Close Find", SymbolName::X, Action::CloseFind),
                ),
        )
    }
}

struct FindInput(Rc<Core>);
impl FindInput {
    fn field(&self) -> TextField {
        let pending = self.0.pending.clone();
        TextField::with_interaction(self.0.find_text.clone())
            .placeholder("Find on page")
            .on_submit(move || enqueue(&pending, Action::FindNext(false)))
    }
}
impl View for FindInput {
    fn measure(&self, c: Constraints, m: &mut MeasureContext<'_>) -> Size {
        self.field().measure(c, m)
    }
    fn paint(&self, b: Rect, c: &mut PaintContext<'_>) {
        if self.0.find_focus.get() {
            self.0.find_text.set_focused(true);
            self.0.find_ready.set(Some(b));
        }
        self.field().paint(b, c);
    }
    fn handle_event(&self, b: Rect, e: &ViewEvent, c: &mut EventContext<'_>) -> EventResult {
        self.field().handle_event(b, e, c)
    }
}

impl BrowserView {
    /// Local saved-page suggestions; no remote autocomplete or background request.
    pub(super) fn suggestions(
        &self,
        window: Rect,
        context: &mut MeasureContext<'_>,
    ) -> Option<(Rect, Popover<Scroll>)> {
        if !self.core.editing_address.get() {
            return None;
        }
        let anchor = self.core.address_bounds.get()?;
        let query = self.core.address_state.value().trim().to_lowercase();
        if query.is_empty() {
            return None;
        }
        let b = self.core.browser.borrow();
        let mut seen = std::collections::HashSet::new();
        let entries: Vec<_> = b
            .bookmarks
            .iter()
            .chain(b.history.iter().rev())
            .chain(b.reading_list.iter())
            .filter(|e| {
                (e.title.to_lowercase().contains(&query) || e.url.to_lowercase().contains(&query))
                    && seen.insert(e.url.clone())
            })
            .take(6)
            .cloned()
            .collect();
        if entries.is_empty() {
            return None;
        }
        let theme = context.theme;
        let mut rows = VStack::new()
            .alignment(StackAlignment::Stretch)
            .gap(StackGap::None);
        for entry in entries {
            let pending = self.core.pending.clone();
            let url = entry.url.clone();
            let interaction = self
                .core
                .rows
                .borrow_mut()
                .entry(format!("suggest:{}", entry.url))
                .or_default()
                .clone();
            rows = rows.child(
                ListRow::with_interaction(interaction, &entry.title)
                    .subtitle(&entry.url)
                    .single_line(true)
                    .on_select(move || enqueue(&pending, Action::Open(url.clone()))),
            );
        }
        let margin = theme.spacing.small;
        let width = anchor
            .size
            .width
            .max(theme.layout.popover_width)
            .min((window.size.width - margin * 2.).max(0.));
        let x = anchor
            .origin
            .x
            .min(window.origin.x + window.size.width - margin - width)
            .max(window.origin.x + margin);
        let y = anchor.origin.y + anchor.size.height + theme.spacing.extra_small;
        let available = (window.origin.y + window.size.height - margin - y).max(0.);
        if available < theme.controls.height {
            return None;
        }
        let measured = rows.measure(
            Constraints::loose(Size::new(
                (width - theme.popover.padding * 2.).max(0.),
                available,
            )),
            context,
        );
        let height = (measured.height + theme.popover.padding * 2.).min(available);
        Some((
            Rect::new(x, y, width, height),
            Popover::new().content(Scroll::new(self.core.suggestions_scroll.clone()).content(rows)),
        ))
    }
}
