use twinkle_ui::prelude::*;

#[derive(Default)]
struct Greeting {
    clicks: usize,
}

#[derive(Clone)]
enum Message {
    Greet,
}

impl Application for Greeting {
    type Message = Message;

    fn update(&mut self, message: Message) {
        match message {
            Message::Greet => self.clicks += 1,
        }
    }

    fn view(&self, _context: twinkle_ui::ViewContext) -> impl View<Message> {
        ui! {
            <Column gap={12.0} padding={Insets::all(24.0)}>
                <Text>{format!("Greetings: {}", self.clicks)}</Text>
                <Button on_press={Message::Greet}>{"Say hello"}</Button>
            </Column>
        }
    }

    fn title(&self) -> &str {
        "Twinkle external consumer"
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run(Greeting::default())
}
