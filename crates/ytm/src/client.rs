use std::{collections::HashSet, fmt, sync::Arc, time::Duration};

use oauth2::reqwest::{self, StatusCode, Url};
use tokio::{sync::Mutex, time::Instant};

use crate::{
    Playlist,
    auth::{self, AuthError, Session},
    playlist::PlaylistPage,
};

const PLAYLISTS_URL: &str = "https://www.googleapis.com/youtube/v3/playlists";
const REFRESH_MARGIN: Duration = Duration::from_secs(60);

/// An authenticated YouTube client. Clones share one session and refresh lock.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    state: Arc<Mutex<State>>,
    playlists_url: Url,
}

struct State {
    session: Option<Session>,
    generation: u64,
}

struct Bearer {
    token: String,
    generation: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Please sign in again to access your YouTube playlists.")]
    SignInRequired,
    #[error(transparent)]
    Auth(AuthError),
    #[error("Could not contact YouTube. Please try again.")]
    Network,
    #[error("YouTube rejected the request (HTTP {status}).")]
    Api { status: u16 },
    #[error("YouTube returned an invalid playlist response.")]
    InvalidResponse,
}

impl From<AuthError> for Error {
    fn from(error: AuthError) -> Self {
        match error {
            AuthError::SignInRequired => Self::SignInRequired,
            error => Self::Auth(error),
        }
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client").finish_non_exhaustive()
    }
}

impl Client {
    pub fn new(session: Session) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| Error::Network)?;

        Ok(Self {
            http,
            state: Arc::new(Mutex::new(State {
                session: Some(session),
                generation: 0,
            })),
            playlists_url: Url::parse(PLAYLISTS_URL).expect("the YouTube playlists URL is valid"),
        })
    }

    pub async fn sign_in() -> Result<Self, Error> {
        Self::new(auth::sign_in().await?)
    }

    pub async fn restore_session() -> Result<Option<Self>, Error> {
        auth::restore_session().await?.map(Self::new).transpose()
    }

    /// Delete persisted credentials and invalidate every clone of this client.
    pub async fn sign_out(&self) -> Result<(), Error> {
        let mut state = self.state.lock().await;

        if let Some(session) = &state.session {
            session.sign_out().await?;
        }

        state.session = None;
        state.generation += 1;
        Ok(())
    }

    /// Fetch every playlist owned by this account, including private playlists.
    /// This is the YouTube Data API, not the full YouTube Music saved library.
    pub async fn playlists(&self) -> Result<Vec<Playlist>, Error> {
        let mut playlists = Vec::new();
        let mut page_token = None;
        let mut seen = HashSet::new();

        loop {
            let page = self.playlist_page(page_token.as_deref()).await?;
            playlists.extend(page.items.into_iter().map(Playlist::from));

            match page.next_page_token.filter(|token| !token.is_empty()) {
                Some(token) if seen.insert(token.clone()) => page_token = Some(token),
                Some(_) => return Err(Error::InvalidResponse),
                None => return Ok(playlists),
            }
        }
    }

    async fn playlist_page(&self, page_token: Option<&str>) -> Result<PlaylistPage, Error> {
        let mut bearer = self.bearer(None).await?;

        for attempt in 0..2 {
            let mut request = self
                .http
                .get(self.playlists_url.clone())
                .bearer_auth(&bearer.token)
                .query(&[
                    ("part", "snippet,contentDetails,status"),
                    ("mine", "true"),
                    ("maxResults", "50"),
                ]);

            if let Some(token) = page_token {
                request = request.query(&[("pageToken", token)]);
            }

            let response = request.send().await.map_err(|_| Error::Network)?;
            let status = response.status();

            if status == StatusCode::UNAUTHORIZED {
                if attempt == 1 {
                    return Err(Error::SignInRequired);
                }

                bearer = self.bearer(Some(bearer.generation)).await?;
                continue;
            }

            if !status.is_success() {
                return Err(Error::Api {
                    status: status.as_u16(),
                });
            }

            let body = response.bytes().await.map_err(|_| Error::Network)?;

            return serde_json::from_slice(&body).map_err(|_| Error::InvalidResponse);
        }

        unreachable!("the last attempt always returns")
    }

    async fn bearer(&self, rejected_generation: Option<u64>) -> Result<Bearer, Error> {
        let mut state = self.state.lock().await;
        let generation = state.generation;
        let session = state.session.as_mut().ok_or(Error::SignInRequired)?;
        let expiring = session
            .expires_at()
            .saturating_duration_since(Instant::now())
            <= REFRESH_MARGIN;
        let rejected = rejected_generation == Some(generation);

        if expiring || rejected {
            match session.refresh().await {
                Ok(()) => state.generation += 1,
                Err(AuthError::SignInRequired) => {
                    state.session = None;

                    return Err(Error::SignInRequired);
                }
                Err(error) => return Err(error.into()),
            }
        }

        let session = state.session.as_ref().ok_or(Error::SignInRequired)?;

        Ok(Bearer {
            token: session.access_token().to_owned(),
            generation: state.generation,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{collections::HashMap, sync::Mutex as StdMutex};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        task::JoinHandle,
    };

    #[derive(Clone)]
    struct Request {
        path: String,
        query: HashMap<String, String>,
        authorization: Option<String>,
        form: HashMap<String, String>,
    }

    struct Server {
        url: String,
        requests: Arc<StdMutex<Vec<Request>>>,
        task: JoinHandle<()>,
    }

    impl Drop for Server {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    impl Server {
        async fn start(
            handler: impl Fn(&Request) -> (u16, serde_json::Value) + Send + Sync + 'static,
        ) -> Self {
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
                .await
                .unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(StdMutex::new(Vec::new()));
            let recorded = requests.clone();
            let handler = Arc::new(handler);
            let task = tokio::spawn(async move {
                loop {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let recorded = recorded.clone();
                    let handler = handler.clone();
                    tokio::spawn(async move {
                        let mut raw = Vec::new();

                        let (header_end, length) = loop {
                            let mut chunk = [0; 1024];
                            let count = stream.read(&mut chunk).await.unwrap();
                            assert!(count > 0);
                            raw.extend_from_slice(&chunk[..count]);

                            if let Some(end) = raw.windows(4).position(|chunk| chunk == b"\r\n\r\n")
                            {
                                let end = end + 4;
                                let headers = std::str::from_utf8(&raw[..end]).unwrap();
                                let length: usize = headers
                                    .lines()
                                    .find_map(|line| {
                                        let (key, value) = line.split_once(':')?;
                                        key.eq_ignore_ascii_case("content-length")
                                            .then(|| value.trim().parse().unwrap())
                                    })
                                    .unwrap_or(0);
                                break (end, length);
                            }
                        };

                        while raw.len() < header_end + length {
                            let mut chunk = [0; 1024];
                            let count = stream.read(&mut chunk).await.unwrap();
                            assert!(count > 0);
                            raw.extend_from_slice(&chunk[..count]);
                        }

                        let headers = std::str::from_utf8(&raw[..header_end]).unwrap();
                        let target = headers
                            .lines()
                            .next()
                            .unwrap()
                            .split_whitespace()
                            .nth(1)
                            .unwrap();
                        let url = Url::parse(&format!("http://localhost{target}")).unwrap();
                        let authorization = headers.lines().find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("authorization")
                                .then(|| value.trim().to_owned())
                        });
                        let form = Url::parse(&format!(
                            "http://localhost/?{}",
                            std::str::from_utf8(&raw[header_end..]).unwrap()
                        ))
                        .unwrap()
                        .query_pairs()
                        .into_owned()
                        .collect();
                        let request = Request {
                            path: url.path().to_owned(),
                            query: url.query_pairs().into_owned().collect(),
                            authorization,
                            form,
                        };

                        recorded.lock().unwrap().push(request.clone());
                        let (status, body) = handler(&request);
                        let body = body.to_string();
                        let response = format!(
                            "HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        stream.write_all(response.as_bytes()).await.unwrap();
                        stream.shutdown().await.unwrap();
                    });
                }
            });

            Self {
                url,
                requests,
                task,
            }
        }

        fn client(&self, lifetime: Duration) -> Client {
            let session = auth::test_session(&format!("{}/token", self.url), lifetime);
            let mut client = Client::new(session).unwrap();
            client.playlists_url = Url::parse(&format!("{}/playlists", self.url)).unwrap();
            client.http = reqwest::Client::builder().no_proxy().build().unwrap();
            client
        }

        fn count(&self, path: &str) -> usize {
            self.requests
                .lock()
                .unwrap()
                .iter()
                .filter(|request| request.path == path)
                .count()
        }
    }

    fn refreshed_token() -> serde_json::Value {
        json!({"access_token": "new-access", "refresh_token": "rotated-refresh", "token_type": "Bearer", "expires_in": 3600})
    }

    fn playlist(id: &str, privacy: &str) -> serde_json::Value {
        json!({
            "id": id,
            "snippet": {"title": "My playlist", "description": "Favorites", "channelTitle": "Owner", "thumbnails": {"high": {"url": "https://example.com/high.jpg"}}},
            "contentDetails": {"itemCount": 12},
            "status": {"privacyStatus": privacy},
        })
    }

    #[tokio::test]
    async fn playlists_collects_every_page_and_maps_metadata() {
        let server = Server::start(|request| {
            assert_eq!(request.path, "/playlists");
            assert_eq!(request.authorization.as_deref(), Some("Bearer old-access"));
            assert_eq!(request.query["mine"], "true");
            assert_eq!(request.query["maxResults"], "50");
            assert_eq!(request.query["part"], "snippet,contentDetails,status");

            if request
                .query
                .get("pageToken")
                .is_some_and(|token| token == "second")
            {
                (200, json!({"items": [playlist("second-id", "private")]}))
            } else {
                (
                    200,
                    json!({"items": [playlist("first-id", "public")], "nextPageToken": "second"}),
                )
            }
        })
        .await;
        let playlists = server
            .client(Duration::from_secs(3600))
            .playlists()
            .await
            .unwrap();
        assert_eq!(playlists.len(), 2);
        assert_eq!(playlists[0].id, "first-id");
        assert_eq!(playlists[0].title, "My playlist");
        assert_eq!(playlists[0].description, "Favorites");
        assert_eq!(playlists[0].owner, "Owner");
        assert_eq!(playlists[0].item_count, 12);
        assert_eq!(
            playlists[0].thumbnail_url.as_deref(),
            Some("https://example.com/high.jpg")
        );
        assert_eq!(playlists[1].privacy, crate::PlaylistPrivacy::Private);
        assert_eq!(server.count("/token"), 0);
        assert_eq!(server.count("/playlists"), 2);
    }

    #[tokio::test]
    async fn near_expiry_refreshes_once_across_concurrent_requests() {
        let server = Server::start(|request| {
            if request.path == "/token" {
                assert_eq!(request.form["grant_type"], "refresh_token");
                assert_eq!(request.form["refresh_token"], "saved-refresh");

                return (200, refreshed_token());
            }

            assert_eq!(request.authorization.as_deref(), Some("Bearer new-access"));
            (200, json!({"items": []}))
        })
        .await;
        let client = server.client(Duration::from_secs(30));
        let cloned = client.clone();
        let (first, second) = tokio::join!(client.playlists(), cloned.playlists());
        assert!(first.unwrap().is_empty());
        assert!(second.unwrap().is_empty());
        assert_eq!(server.count("/token"), 1);
        assert_eq!(server.count("/playlists"), 2);
    }

    #[tokio::test]
    async fn concurrent_unauthorized_requests_share_one_refresh() {
        let server = Server::start(|request| {
            if request.path == "/token" {
                return (200, refreshed_token());
            }

            if request.authorization.as_deref() == Some("Bearer old-access") {
                return (401, json!({"error": "unauthorized"}));
            }

            assert_eq!(request.authorization.as_deref(), Some("Bearer new-access"));
            (200, json!({"items": []}))
        })
        .await;
        let client = server.client(Duration::from_secs(3600));
        let cloned = client.clone();
        let (first, second) = tokio::join!(client.playlists(), cloned.playlists());
        assert!(first.is_ok());
        assert!(second.is_ok());
        assert_eq!(server.count("/token"), 1);
    }

    #[tokio::test]
    async fn repeated_unauthorized_response_stops_after_one_retry() {
        let server = Server::start(|request| {
            if request.path == "/token" {
                return (200, refreshed_token());
            }

            (401, json!({"error": "unauthorized"}))
        })
        .await;
        assert!(matches!(
            server.client(Duration::from_secs(3600)).playlists().await,
            Err(Error::SignInRequired)
        ));
        assert_eq!(server.count("/token"), 1);
        assert_eq!(server.count("/playlists"), 2);
    }

    #[tokio::test]
    async fn forbidden_response_does_not_refresh_or_retry() {
        let server = Server::start(|_| (403, json!({"error": {"reason": "quotaExceeded"}}))).await;
        assert!(matches!(
            server.client(Duration::from_secs(3600)).playlists().await,
            Err(Error::Api { status: 403 })
        ));
        assert_eq!(server.count("/token"), 0);
        assert_eq!(server.count("/playlists"), 1);
    }

    #[tokio::test]
    async fn revoked_refresh_token_invalidates_shared_client() {
        let server = Server::start(|_| (400, json!({"error": "invalid_grant"}))).await;
        let client = server.client(Duration::ZERO);
        assert!(matches!(
            client.playlists().await,
            Err(Error::SignInRequired)
        ));
        assert!(matches!(
            client.clone().playlists().await,
            Err(Error::SignInRequired)
        ));
        assert_eq!(server.count("/token"), 1);
        assert_eq!(server.count("/playlists"), 0);
    }

    #[tokio::test]
    async fn malformed_response_and_repeated_page_token_are_rejected() {
        let malformed = Server::start(|_| (200, json!({"unexpected": []}))).await;
        assert!(matches!(
            malformed
                .client(Duration::from_secs(3600))
                .playlists()
                .await,
            Err(Error::InvalidResponse)
        ));
        let repeated =
            Server::start(|_| (200, json!({"items": [], "nextPageToken": "same"}))).await;
        assert!(matches!(
            repeated.client(Duration::from_secs(3600)).playlists().await,
            Err(Error::InvalidResponse)
        ));
        assert_eq!(repeated.count("/playlists"), 2);
    }

    #[tokio::test]
    async fn sign_out_invalidates_all_clones_without_network_requests() {
        let server = Server::start(|_| panic!("signed-out clients must not send requests")).await;
        let client = server.client(Duration::from_secs(3600));
        let cloned = client.clone();
        client.sign_out().await.unwrap();
        assert!(matches!(
            cloned.playlists().await,
            Err(Error::SignInRequired)
        ));
        client.sign_out().await.unwrap();
        assert_eq!(server.count("/playlists"), 0);
    }
}
