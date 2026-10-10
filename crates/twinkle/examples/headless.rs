use twinkle_ui::prelude::*;

#[derive(Default)]
struct Greeting;

impl Application for Greeting {
    type Message = ();

    fn update(&mut self, (): ()) {}

    fn view(&self, _context: twinkle_ui::ViewContext) -> impl View<()> {
        Column::new()
            .id(id!(greeting))
            .child(Text::new("Hello from a headless host"))
    }
}

fn main() {
    let host = twinkle_ui::UiHost::new(Greeting, 640, 480);
    let nodes = host.accessibility_nodes();
    assert!(
        nodes
            .iter()
            .any(|node| node.label.as_deref() == Some("Hello from a headless host"))
    );
}
