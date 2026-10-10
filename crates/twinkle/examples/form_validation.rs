use twinkle_ui::prelude::*;

#[derive(Default)]
struct SignIn {
    email: String,
    submitted: bool,
}

#[derive(Clone)]
enum Message {
    EditEmail(String),
    Submit,
}

impl SignIn {
    fn email_error(&self) -> Option<&'static str> {
        (!self.email.is_empty() && !self.email.contains('@')).then_some("Enter a valid email")
    }
}

impl Application for SignIn {
    type Message = Message;

    fn update(&mut self, message: Message) {
        match message {
            Message::EditEmail(value) => {
                self.email = value;
                self.submitted = false;
            }
            Message::Submit if self.email_error().is_none() && !self.email.is_empty() => {
                self.submitted = true;
            }
            Message::Submit => {}
        }
    }

    fn view(&self, _context: twinkle_ui::ViewContext) -> impl View<Message> {
        let status = if self.submitted {
            "Submitted"
        } else {
            self.email_error().unwrap_or("Enter an email address")
        };
        Column::new()
            .id(id!(sign_in))
            .gap(10.0)
            .padding(Insets::all(24.0))
            .child(Text::new("Sign in").scale(3.0))
            .child(
                TextField::on_change_with_placeholder(
                    &self.email,
                    "name@example.com",
                    Message::EditEmail,
                )
                .id(id!(email))
                .accessibility_label("Email address"),
            )
            .child(Text::new(status))
            .child(Button::new(Message::Submit, "Continue"))
    }

    fn title(&self) -> &str {
        "Twinkle form validation"
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run(SignIn::default())
}
