use iced::{
    Element, Task,
    widget::{button, column},
};
use tracing::error;
use ytm::Client;

#[derive(Debug, Clone)]
pub struct Login;

#[derive(Debug, Clone)]
pub enum Message {
    SignIn,
    SignInFinished(Result<Client, String>),
}

#[derive(Debug)]
pub enum Event {
    SignedIn(Client),
}

impl Login {
    pub fn new() -> Self {
        Self
    }

    pub fn update(&mut self, message: Message) -> (Task<Message>, Option<Event>) {
        match message {
            Message::SignIn => (
                Task::perform(Client::sign_in(), |result| {
                    Message::SignInFinished(result.map_err(|e| e.to_string()))
                }),
                None,
            ),
            Message::SignInFinished(Ok(client)) => (Task::none(), Some(Event::SignedIn(client))),
            Message::SignInFinished(Err(error)) => {
                error!("Error signin in: {error}");
                (Task::none(), None)
            }
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        column![button("Sign In").on_press(Message::SignIn)].into()
    }
}
