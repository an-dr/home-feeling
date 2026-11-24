//! The Things window (UC-THINGS-01). Until the Lua host exists it only names
//! the app; each frame it describes its interface as a bones widget tree.
use bones_kernel::messages::ui::kinds::Label;
use bones_kernel::messages::ui::{Event, Layout, Node, Root, Spec, WidgetProps};
use bones_kernel::ui::{Frame, UiView};

/// The module name, under which bones shows this window's trees.
pub const NAME: &str = "home-feeling";

pub struct ThingsWindow;

impl UiView for ThingsWindow {
    fn on_event(&mut self, _event: Event) {}

    fn view(&mut self, _frame: &mut Frame<'_>) -> Spec {
        let label = |text: &str| Label { text: text.into() }.node(0);
        Spec {
            root: Root::Screen,
            body: Node::container(
                Layout::Vertical,
                vec![label("home-feeling"), label("No Things loaded yet.")],
            ),
        }
    }
}
