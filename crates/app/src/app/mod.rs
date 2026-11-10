use iced::{Element, Task};
use ytm::Client;

use crate::app::screens::login::{self, Login};

pub mod screens;

pub struct App {
    screen: Screen,
    client: Option<Client>,
    loading: bool,
    login_screen: login::Login,
}

#[derive(Debug, Clone)]
pub enum Message {
    Navigate(Screen),
    SessionRestored(Result<Option<Client>, String>),
    Login(login::Message),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Login,
    Home,
}

impl App {
    pub fn new() -> (Self, Task<Message>) {
        (
            Self {
                screen: Screen::Login,
                client: None,
                loading: true,
                login_screen: Login::new(),
            },
            Task::perform(Client::restore_session(), |result| {
                Message::SessionRestored(result.map_err(|e| e.to_string()))
            }),
        )
    }

    pub fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::Navigate(screen) => {
                if !self.is_authenticated() {
                    self.screen = Screen::Login;

                    return Task::none();
                }

                self.screen = screen;

                Task::none()
            }
            Message::SessionRestored(result) => {
                self.loading = false;

                match result {
                    Ok(c) => {
                        self.client = c;
                        self.screen = if self.is_authenticated() {
                            Screen::Home
                        } else {
                            Screen::Login
                        }
                    }
                    Err(e) => {
                        tracing::error!("Failed to restore session: {e}");
                        self.screen = Screen::Login;
                    }
                }

                Task::none()
            }
            Message::Login(message) => {
                let (task, event) = self.login_screen.update(message);

                if let Some(login::Event::SignedIn(client)) = event {
                    self.client = Some(client);

                    return Task::done(Message::Navigate(Screen::Home));
                }

                task.map(Message::Login)
            }
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        if self.loading {
            return iced::widget::text("Loading...").into();
        }

        match self.screen {
            Screen::Login => self.login_screen.view().map(Message::Login),
            Screen::Home => todo!(),
        }
    }

    const fn is_authenticated(&self) -> bool {
        self.client.is_some()
    }
}
