#![cfg_attr(all(feature = "engine-probe", target_os = "mochios"), feature(alloc_error_hook))]
mod browser;
mod engine;
mod store;
mod ui;
use appcore::prelude::*;
struct TerraApp {
    view: ui::BrowserView,
}
impl App for TerraApp {
    type Body = ui::BrowserView;
    fn new() -> Self {
        Self {
            view: ui::BrowserView::persistent(engine::connect()),
        }
    }
    fn window(&self) -> WindowOptions {
        let layout = Theme::current().layout;
        WindowOptions::new("Terra")
            .size(layout.standard_window_width, layout.standard_window_height)
            .resizable(true)
            .titlebar(self.view.titlebar())
    }
    fn body(&self, _: &ViewContext) -> Self::Body {
        self.view.clone()
    }
}
fn main() -> Result<(), ViewKitError> {
    #[cfg(all(target_os = "mochios", feature = "engine-probe"))]
    {
        let result = engine::run_probe();
        if let Err(e) = &result {
            eprintln!("Terra engine probe: {e}");
        }
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }
    #[cfg(not(all(target_os = "mochios", feature = "engine-probe")))]
    run::<TerraApp>()
}
