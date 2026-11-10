pub use client::{Client, Error};
pub use playlist::{Playlist, PlaylistPrivacy};

pub mod auth;

mod client;
mod credentials;
mod playlist;
mod token_store;
