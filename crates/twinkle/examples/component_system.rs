use twinkle_ui::prelude::*;

#[derive(Default)]
struct Preferences {
    notifications: bool,
}

#[derive(Clone)]
enum Message {
    ToggleNotifications,
}

// A reusable system component: the caller owns its identity, content, and action.
#[component]
fn PreferenceCard(
    title: &str,
    description: &str,
    enabled: bool,
    on_toggle: Message,
) -> impl View<Message> {
    ui! {
        <Container padding={Insets::all(16.0)} background={0x252b34} radius={8.0}>
            <Column gap={8.0}>
                <Text scale={1.25}>{title}</Text>
                <Text>{description}</Text>
                <Button on_press={on_toggle}>
                    {if enabled { "Turn off" } else { "Turn on" }}
                </Button>
            </Column>
        </Container>
    }
}

impl Application for Preferences {
    type Message = Message;

    fn update(&mut self, message: Message) {
        match message {
            Message::ToggleNotifications => self.notifications = !self.notifications,
        }
    }

    fn view(&self, _context: twinkle_ui::ViewContext) -> impl View<Message> {
        ui! {
            <Column gap={16.0} padding={Insets::all(24.0)}>
                <Text scale={2.0}>{"Notification preferences"}</Text>
                <PreferenceCard
                    id={id!(notifications)}
                    title={"Notifications"}
                    description={"Choose whether this app may notify you."}
                    enabled={self.notifications}
                    on_toggle={Message::ToggleNotifications}
                />
            </Column>
        }
    }

    fn title(&self) -> &str {
        "Reusable component system"
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run(Preferences::default())
}
