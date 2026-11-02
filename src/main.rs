mod browser;
mod engine;
mod ui;
use appcore::prelude::*;
struct TerraApp {
    view: ui::BrowserView,
}
impl App for TerraApp {
    type Body = ApplicationMenuBar<ui::BrowserView>;
    fn new() -> Self {
        Self {
            view: ui::BrowserView::new(Box::new(engine::Unavailable)),
        }
    }
    fn window(&self) -> WindowOptions {
        let layout = Theme::current().layout;
        WindowOptions::new("Terra")
            .size(layout.standard_window_width, layout.standard_window_height)
            .resizable(true)
    }
    fn body(&self, _: &ViewContext) -> Self::Body {
        self.view.with_menus()
    }
}
fn main() -> Result<(), ViewKitError> {
    run::<TerraApp>()
}
