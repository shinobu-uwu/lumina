use std::collections::BTreeMap;

use serde::Deserialize;

/// A playlist owned by the signed-in YouTube account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playlist {
    pub id: String,
    pub title: String,
    pub description: String,
    pub owner: String,
    pub item_count: u64,
    pub thumbnail_url: Option<String>,
    pub privacy: PlaylistPrivacy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaylistPrivacy {
    Public,
    Private,
    Unlisted,
    #[serde(other)]
    Unknown,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlaylistPage {
    pub items: Vec<PlaylistResource>,
    pub next_page_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlaylistResource {
    id: String,
    snippet: Snippet,
    content_details: ContentDetails,
    status: Status,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Snippet {
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    channel_title: String,
    #[serde(default)]
    thumbnails: BTreeMap<String, Thumbnail>,
}

#[derive(Deserialize)]
struct Thumbnail {
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ContentDetails {
    item_count: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Status {
    privacy_status: PlaylistPrivacy,
}

impl From<PlaylistResource> for Playlist {
    fn from(resource: PlaylistResource) -> Self {
        let thumbnail_url = ["maxres", "standard", "high", "medium", "default"]
            .into_iter()
            .find_map(|quality| resource.snippet.thumbnails.get(quality))
            .map(|thumbnail| thumbnail.url.clone());

        Self {
            id: resource.id,
            title: resource.snippet.title,
            description: resource.snippet.description,
            owner: resource.snippet.channel_title,
            item_count: resource.content_details.item_count,
            thumbnail_url,
            privacy: resource.status.privacy_status,
        }
    }
}
