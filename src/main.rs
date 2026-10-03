use appcore::prelude::*;

#[derive(Clone)]
struct BrowserTab {
    address: State<String>,
    bookmarked: State<bool>,
}

impl BrowserTab {
    fn new() -> Self {
        Self {
            address: State::new(String::new()),
            bookmarked: State::new(false),
        }
    }

    fn title(&self) -> String {
        let address = self.address.get();
        if address.is_empty() {
            String::from("New Tab")
        } else {
            address
        }
    }
}

struct TerraApp {
    tabs: State<Vec<BrowserTab>>,
    active_tab: State<usize>,
}

impl App for TerraApp {
    type Body = Box<dyn View + 'static>;

    fn new() -> Self {
        Self {
            tabs: State::new(vec![BrowserTab::new()]),
            active_tab: State::new(0),
        }
    }

    fn window(&self) -> WindowOptions {
        let layout = Theme::current().layout;
        WindowOptions::new("Terra")
            .size(layout.standard_window_width, layout.standard_window_height)
            .resizable(true)
    }

    fn body(&self, _context: &ViewContext) -> Self::Body {
        let tabs = self.tabs.get();
        let active_index = self.active_tab.get().min(tabs.len().saturating_sub(1));
        let active_tab = tabs[active_index].clone();
        let bookmarked = active_tab.bookmarked.clone();
        let bookmarked_on_click = bookmarked.clone();
        let tab_views = tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                if index == active_index {
                    TextField::new(tab.address.binding())
                        .placeholder("Search or enter address")
                        .leading_symbol(SymbolName::Search)
                        .size(TextFieldSize::Small)
                        .on_submit(|| {})
                        .layout()
                        .flex_grow(1.0)
                } else {
                    let active_tab = self.active_tab.clone();
                    Button::new(tab.title())
                        .size(ButtonSize::Small)
                        .on_click(move || {
                            active_tab.set_if_changed(index);
                        })
                        .width(Theme::current().layout.tab_width)
                }
            })
            .collect::<Vec<_>>();
        let tabs_for_new = self.tabs.clone();
        let active_for_new = self.active_tab.clone();

        let browser_bar = Toolbar::new(
            HStack::new()
                .alignment(StackAlignment::Center)
                .gap(StackGap::Small)
                .child(
                    IconButton::new(SymbolName::ArrowBackward)
                        .enabled(false)
                        .accessibility_label("Back"),
                )
                .child(
                    IconButton::new(SymbolName::ArrowForward)
                        .enabled(false)
                        .accessibility_label("Forward"),
                )
                .child(IconButton::new(SymbolName::Refresh).accessibility_label("Reload"))
                .child(IconButton::new(SymbolName::Home).accessibility_label("Home"))
                .child(
                    HStack::new()
                        .alignment(StackAlignment::Center)
                        .gap(StackGap::Small)
                        .children(tab_views)
                        .layout()
                        .flex_grow(1.0),
                )
                .child(
                    IconButton::new(SymbolName::Plus)
                        .on_click(move || {
                            let new_index = tabs_for_new.with(Vec::len);
                            tabs_for_new.update(|tabs| tabs.push(BrowserTab::new()));
                            active_for_new.set(new_index);
                        })
                        .accessibility_label("New Tab"),
                )
                .child(
                    IconButton::new(if bookmarked.get() {
                        SymbolName::BookmarkFill
                    } else {
                        SymbolName::Bookmark
                    })
                    .on_click(move || {
                        bookmarked_on_click.set(!bookmarked_on_click.get());
                    })
                    .accessibility_label("Bookmark"),
                )
                .child(IconButton::new(SymbolName::More).accessibility_label("More")),
        );

        Box::new(
            Surface::app().content(
                VStack::new()
                    .alignment(StackAlignment::Stretch)
                    .gap(StackGap::None)
                    .child(browser_bar)
                    .child(Divider::new())
                    .child(Surface::app().layout().flex_grow(1.0)),
            ),
        )
    }
}

fn main() -> Result<(), appcore::ViewKitError> {
    appcore::run::<TerraApp>()
}
