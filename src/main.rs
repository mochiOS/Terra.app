use appcore::prelude::*;

struct TerraApp {
    active_tab: State<usize>,
    address: State<String>,
    bookmarked: State<bool>,
}

impl App for TerraApp {
    type Body = Box<dyn View + 'static>;

    fn new() -> Self {
        Self {
            active_tab: State::new(0),
            address: State::new(String::new()),
            bookmarked: State::new(false),
        }
    }

    fn window(&self) -> WindowOptions {
        let layout = Theme::current().layout;
        WindowOptions::new("Terra")
            .size(layout.standard_window_width, layout.standard_window_height)
            .resizable(true)
    }

    fn body(&self, _context: &ViewContext) -> Self::Body {
        let bookmarked = self.bookmarked.clone();
        let bookmarked_on_click = bookmarked.clone();

        let tab_bar = Toolbar::new(
            HStack::new()
                .alignment(StackAlignment::Center)
                .gap(StackGap::Small)
                .child(
                    Tabs::new(self.active_tab.binding())
                        .item(0, "New Tab")
                        .accessibility_label("Open tabs"),
                )
                .child(Spacer::new()),
        );

        let navigation_bar = Toolbar::new(
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
                    TextField::new(self.address.binding())
                        .placeholder("Search or enter address")
                        .leading_symbol(SymbolName::Search)
                        .size(TextFieldSize::Small)
                        .on_submit(|| {})
                        .layout()
                        .flex_grow(1.0),
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
                    .child(tab_bar)
                    .child(Divider::new())
                    .child(navigation_bar)
                    .child(Divider::new())
                    .child(Surface::app().layout().flex_grow(1.0)),
            ),
        )
    }
}

fn main() -> Result<(), appcore::ViewKitError> {
    appcore::run::<TerraApp>()
}
