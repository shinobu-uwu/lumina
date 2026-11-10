//! Google desktop OAuth for YouTube Data API access, not a Music web session.

use std::{fmt, time::Duration};

use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointNotSet,
    EndpointSet, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, RefreshToken, RequestTokenError,
    Scope, TokenResponse, TokenUrl,
    basic::{
        BasicClient, BasicErrorResponse, BasicErrorResponseType, BasicTokenResponse, BasicTokenType,
    },
    reqwest::{self, Client, Url},
};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::{Instant, timeout},
};

use crate::{credentials::GoogleCredentials, token_store::TokenStore};

pub const YOUTUBE_READONLY_SCOPE: &str = "https://www.googleapis.com/auth/youtube.readonly";
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);
type OAuthClient =
    BasicClient<EndpointSet, EndpointNotSet, EndpointNotSet, EndpointNotSet, EndpointSet>;

/// Access tokens stay in memory; refresh tokens are saved in the OS credential store.
/// Debug deliberately omits all token contents.
#[derive(Clone)]
pub struct Session {
    token: BasicTokenResponse,
    client: OAuthClient,
    expires_at: Instant,
    store: TokenStore,
}

impl Session {
    pub fn access_token(&self) -> &str {
        self.token.access_token().secret()
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.token
            .refresh_token()
            .map(|token| token.secret().as_str())
    }

    pub fn expires_at(&self) -> Instant {
        self.expires_at
    }

    /// Refresh the session and persist any replacement refresh token.
    /// Transient failures leave the saved credentials intact.
    pub async fn refresh(&mut self) -> Result<(), AuthError> {
        let refresh_token = self
            .token
            .refresh_token()
            .ok_or(AuthError::SignInRequired)?;

        let token = match refresh_access_token(&self.client, refresh_token).await {
            Err(AuthError::SignInRequired) => {
                self.store.delete().await.map_err(|_| AuthError::Storage)?;

                return Err(AuthError::SignInRequired);
            }
            result => result?,
        };

        self.update_token(token)?;
        self.persist().await
    }

    /// Delete this session's saved refresh token.
    pub async fn sign_out(&self) -> Result<(), AuthError> {
        self.store.delete().await.map_err(|_| AuthError::Storage)
    }

    async fn persist(&self) -> Result<(), AuthError> {
        let token = self.refresh_token().ok_or(AuthError::SignInRequired)?;
        self.store.save(token).await.map_err(|_| AuthError::Storage)
    }

    fn update_token(&mut self, mut token: BasicTokenResponse) -> Result<(), AuthError> {
        if token.refresh_token().is_none() {
            token.set_refresh_token(self.token.refresh_token().cloned());
        }

        let mut updated = Self::from_token(self.client.clone(), token)?;
        updated.store = self.store.clone();
        *self = updated;
        Ok(())
    }

    fn from_token(client: OAuthClient, token: BasicTokenResponse) -> Result<Self, AuthError> {
        if token.access_token().secret().is_empty() || *token.token_type() != BasicTokenType::Bearer
        {
            return Err(AuthError::InvalidToken);
        }

        // An omitted scope means the grant matches the request.
        if token.scopes().is_some_and(|scopes| {
            !scopes
                .iter()
                .any(|scope| scope.as_str() == YOUTUBE_READONLY_SCOPE)
        }) {
            return Err(AuthError::MissingScope);
        }

        let expires_at = Instant::now()
            .checked_add(token.expires_in().ok_or(AuthError::InvalidToken)?)
            .ok_or(AuthError::InvalidToken)?;
        let store = TokenStore::new(client.client_id().as_str());

        Ok(Self {
            token,
            client,
            expires_at,
            store,
        })
    }
}

impl fmt::Debug for Session {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Session")
            .field("tokens", &"[redacted]")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Could not load Google sign-in configuration: {0}")]
    Credentials(&'static str),
    #[error("Could not open your default browser. Check your browser settings and try again.")]
    Browser,
    #[error("Could not start or read the local sign-in callback: {0}")]
    CallbackIo(#[from] std::io::Error),
    #[error("Sign-in timed out. Please try again.")]
    TimedOut,
    #[error("Invalid sign-in callback")]
    InvalidCallback,
    #[error("Google sign-in was declined or could not be completed. Please try again.")]
    Denied,
    #[error("Could not contact Google to complete sign-in. Please try again.")]
    Network,
    #[error("Google could not complete sign-in. Please try again or contact the app maintainer.")]
    TokenRejected,
    #[error("Google returned an invalid token response")]
    InvalidToken,
    #[error("YouTube playlist access was not granted. Please sign in again and allow access.")]
    MissingScope,
    #[error("Please sign in again to renew your Google session.")]
    SignInRequired,
    #[error(
        "Could not access saved Google sign-in. Unlock your system credential store and try again."
    )]
    Storage,
}

/// Sign in using Lumina's embedded credentials and the system browser.
/// Dropping this future closes the callback listener and cancels sign-in.
pub async fn sign_in() -> Result<Session, AuthError> {
    let credentials = GoogleCredentials::load().map_err(AuthError::Credentials)?;
    let authorization =
        Authorization::new(credentials.client_id, credentials.client_secret).await?;
    let url = authorization.url.as_str().to_owned();
    tokio::task::spawn_blocking(move || open::that(url))
        .await
        .map_err(|_| AuthError::Browser)?
        .map_err(|_| AuthError::Browser)?;
    let session = authorization.finish().await?;
    session.persist().await?;
    Ok(session)
}

/// Restore the saved Google login without opening a browser.
/// Missing or revoked credentials return `None`; transient failures remain errors.
pub async fn restore_session() -> Result<Option<Session>, AuthError> {
    let credentials = GoogleCredentials::load().map_err(AuthError::Credentials)?;
    let client = google_client(
        credentials.client_id,
        credentials.client_secret,
        "http://127.0.0.1",
    );
    restore_from_store(client.clone(), TokenStore::new(client.client_id().as_str())).await
}

async fn restore_from_store(
    client: OAuthClient,
    store: TokenStore,
) -> Result<Option<Session>, AuthError> {
    let Some(saved) = store.load().await.map_err(|_| AuthError::Storage)? else {
        return Ok(None);
    };

    if saved.is_empty() {
        store.delete().await.map_err(|_| AuthError::Storage)?;

        return Ok(None);
    }

    let refresh_token = RefreshToken::new(saved);

    let mut token = match refresh_access_token(&client, &refresh_token).await {
        Err(AuthError::SignInRequired) => {
            store.delete().await.map_err(|_| AuthError::Storage)?;

            return Ok(None);
        }
        result => result?,
    };

    if token.refresh_token().is_none() {
        token.set_refresh_token(Some(refresh_token));
    }

    let mut session = Session::from_token(client, token)?;
    session.store = store;
    session.persist().await?;
    Ok(Some(session))
}

/// Remove the saved login. Google authorization itself is not revoked.
pub async fn sign_out() -> Result<(), AuthError> {
    let credentials = GoogleCredentials::load().map_err(AuthError::Credentials)?;
    TokenStore::new(&credentials.client_id)
        .delete()
        .await
        .map_err(|_| AuthError::Storage)
}

async fn refresh_access_token(
    client: &OAuthClient,
    token: &RefreshToken,
) -> Result<BasicTokenResponse, AuthError> {
    client
        .exchange_refresh_token(token)
        .request_async(&http_client()?)
        .await
        .map_err(token_error)
}

/// Holds the loopback listener before the browser opens. Dropping it cancels login.
struct Authorization {
    listener: TcpListener,
    url: Url,
    redirect_uri: String,
    state: CsrfToken,
    verifier: PkceCodeVerifier,
    client: OAuthClient,
}

impl Authorization {
    async fn new(client_id: String, client_secret: String) -> Result<Self, AuthError> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let redirect_uri = format!("http://{}/oauth/callback", listener.local_addr()?);
        let client = google_client(client_id, client_secret, &redirect_uri);
        let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
        let (url, state) = authorization_url(&client, challenge);

        Ok(Self {
            listener,
            url,
            redirect_uri,
            state,
            verifier,
            client,
        })
    }

    async fn finish(self) -> Result<Session, AuthError> {
        let code = timeout(LOGIN_TIMEOUT, self.receive_code())
            .await
            .map_err(|_| AuthError::TimedOut)??;
        let token = self
            .client
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(self.verifier)
            .request_async(&http_client()?)
            .await
            .map_err(token_error)?;
        Session::from_token(self.client, token)
    }

    async fn receive_code(&self) -> Result<String, AuthError> {
        loop {
            let (mut stream, _) = self.listener.accept().await?;

            // Ignore unrelated requests and bound slow or oversized local clients.
            let request = match timeout(Duration::from_secs(3), read_request(&mut stream)).await {
                Ok(Ok(request)) => request,
                _ => continue,
            };

            let result = parse_callback(&request, &self.redirect_uri, &self.state);

            let (status, body) = match &result {
                Ok(_) => (
                    "200 OK",
                    "Authorization received. Return to Lumina to finish signing in.",
                ),
                Err(AuthError::Denied) => {
                    ("200 OK", "Sign-in was declined. You can return to Lumina.")
                }
                Err(_) => (
                    "400 Bad Request",
                    "Invalid sign-in callback. Return to the Google sign-in tab.",
                ),
            };

            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            // A closed browser tab must not discard an otherwise valid code.
            let _ = timeout(
                Duration::from_secs(3),
                stream.write_all(response.as_bytes()),
            )
            .await;

            match result {
                Err(AuthError::InvalidCallback) => continue,
                result => return result,
            }
        }
    }
}

fn google_client(client_id: String, client_secret: String, redirect_uri: &str) -> OAuthClient {
    BasicClient::new(ClientId::new(client_id))
        .set_client_secret(ClientSecret::new(client_secret))
        .set_auth_type(AuthType::RequestBody)
        .set_auth_uri(
            AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".into())
                .expect("the Google authorization URL is valid"),
        )
        .set_token_uri(
            TokenUrl::new("https://oauth2.googleapis.com/token".into())
                .expect("the Google token URL is valid"),
        )
        .set_redirect_uri(
            RedirectUrl::new(redirect_uri.to_owned()).expect("the generated loopback URL is valid"),
        )
}

fn authorization_url(client: &OAuthClient, challenge: PkceCodeChallenge) -> (Url, CsrfToken) {
    client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new(YOUTUBE_READONLY_SCOPE.into()))
        .set_pkce_challenge(challenge)
        .add_extra_param("access_type", "offline")
        .add_extra_param("prompt", "consent")
        .url()
}

fn http_client() -> Result<Client, AuthError> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| AuthError::Network)
}

fn token_error<E: std::error::Error + 'static>(
    error: RequestTokenError<E, BasicErrorResponse>,
) -> AuthError {
    // Responses and errors can contain tokens; expose only sanitized categories.
    match error {
        RequestTokenError::ServerResponse(response)
            if *response.error() == BasicErrorResponseType::InvalidGrant =>
        {
            AuthError::SignInRequired
        }
        RequestTokenError::ServerResponse(_) => AuthError::TokenRejected,
        RequestTokenError::Request(_) => AuthError::Network,
        RequestTokenError::Parse(_, _) | RequestTokenError::Other(_) => AuthError::InvalidToken,
    }
}

async fn read_request(stream: &mut TcpStream) -> Result<String, AuthError> {
    let mut request = Vec::new();
    let mut chunk = [0_u8; 1024];

    while request.len() < 8192 {
        let count = stream.read(&mut chunk).await?;

        if count == 0 {
            break;
        }

        request.extend_from_slice(&chunk[..count]);

        if request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
            return String::from_utf8(request).map_err(|_| AuthError::InvalidCallback);
        }
    }

    Err(AuthError::InvalidCallback)
}

fn parse_callback(
    request: &str,
    redirect_uri: &str,
    state: &CsrfToken,
) -> Result<String, AuthError> {
    let mut line = request
        .lines()
        .next()
        .unwrap_or_default()
        .split_whitespace();

    if line.next() != Some("GET") {
        return Err(AuthError::InvalidCallback);
    }

    let target = line.next().ok_or(AuthError::InvalidCallback)?;

    if !target.starts_with('/') || target.starts_with("//") {
        return Err(AuthError::InvalidCallback);
    }

    let redirect = Url::parse(redirect_uri).map_err(|_| AuthError::InvalidCallback)?;
    let url = redirect
        .join(target)
        .map_err(|_| AuthError::InvalidCallback)?;

    if url.origin() != redirect.origin() || url.path() != redirect.path() {
        return Err(AuthError::InvalidCallback);
    }

    let params: Vec<_> = url.query_pairs().collect();
    let value = |key: &str| {
        let mut values = params.iter().filter(|(name, _)| name == key);
        let (_, value) = values.next()?;
        values.next().is_none().then_some(value.as_ref())
    };

    // Use oauth2's timing-resistant comparison for the returned state.
    if !value("state").is_some_and(|value| CsrfToken::new(value.to_owned()) == *state) {
        return Err(AuthError::InvalidCallback);
    }

    if value("error").is_some() {
        return Err(AuthError::Denied);
    }

    value("code")
        .filter(|code| !code.is_empty())
        .map(str::to_owned)
        .ok_or(AuthError::InvalidCallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oauth2::http::{Response, header::CONTENT_TYPE};
    use std::collections::HashMap;

    const REDIRECT: &str = "http://127.0.0.1:12345/oauth/callback";

    fn client() -> OAuthClient {
        google_client("client".into(), "client-secret".into(), REDIRECT)
    }

    fn token(json: &str) -> BasicTokenResponse {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn authorization_includes_pkce_scope_and_offline_consent() {
        let verifier = PkceCodeVerifier::new("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        let (url, state) = authorization_url(
            &client(),
            PkceCodeChallenge::from_code_verifier_sha256(&verifier),
        );
        let params: HashMap<_, _> = url.query_pairs().collect();
        assert_eq!(
            params["code_challenge"],
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(params["redirect_uri"], REDIRECT);
        assert_eq!(params["scope"], YOUTUBE_READONLY_SCOPE);
        assert_eq!(params["state"], *state.secret());
        assert_eq!(params["access_type"], "offline");
        assert_eq!(params["prompt"], "consent");
        assert!(!params.contains_key("client_secret"));
    }

    #[test]
    fn callback_decodes_code_and_checks_state_before_errors() {
        let state = CsrfToken::new("expected".into());
        assert_eq!(
            parse_callback(
                "GET /oauth/callback?state=expected&code=a%2Bb HTTP/1.1\r\n",
                REDIRECT,
                &state
            )
            .unwrap(),
            "a+b"
        );
        assert!(matches!(
            parse_callback(
                "GET /oauth/callback?state=expected&error=access_denied HTTP/1.1\r\n",
                REDIRECT,
                &state
            ),
            Err(AuthError::Denied)
        ));
        assert!(matches!(
            parse_callback(
                "GET /oauth/callback?state=wrong&error=access_denied HTTP/1.1\r\n",
                REDIRECT,
                &state
            ),
            Err(AuthError::InvalidCallback)
        ));
    }

    #[test]
    fn callback_rejects_unrelated_or_ambiguous_requests() {
        let state = CsrfToken::new("expected".into());

        for target in [
            "/favicon.ico?state=expected&code=secret",
            "/oauth/callback?code=secret",
            "/oauth/callback?state=wrong&code=secret",
            "/oauth/callback?state=expected&state=expected&code=secret",
            "/oauth/callback?state=expected&code=a&code=b",
            "/oauth/callback?state=expected&code=",
            "//example.com/oauth/callback?state=expected&code=secret",
        ] {
            assert!(matches!(
                parse_callback(&format!("GET {target} HTTP/1.1\r\n"), REDIRECT, &state),
                Err(AuthError::InvalidCallback)
            ));
        }
    }

    #[tokio::test]
    async fn listener_survives_invalid_callback_then_accepts_valid_code() {
        let auth = Authorization::new("client".into(), "secret".into())
            .await
            .unwrap();
        let callback = format!(
            "{}?state={}&code=accepted",
            auth.redirect_uri,
            auth.state.secret()
        );
        let client = Client::builder().no_proxy().build().unwrap();
        let (code, ()) = tokio::join!(auth.receive_code(), async {
            let response = client
                .get(format!("{}?state=wrong&code=rejected", auth.redirect_uri))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
            assert!(
                client
                    .get(callback)
                    .send()
                    .await
                    .unwrap()
                    .status()
                    .is_success()
            );
        });
        assert_eq!(code.unwrap(), "accepted");
    }

    #[tokio::test]
    async fn exchange_sends_google_credentials_and_pkce_in_request_body() {
        let verifier = PkceCodeVerifier::new("verifier".into());
        let token = client().exchange_code(AuthorizationCode::new("code+value".into()))
            .set_pkce_verifier(verifier)
            .request_async(&|request: oauth2::HttpRequest| async move {
                assert_eq!(request.uri(), "https://oauth2.googleapis.com/token");
                assert!(!request.headers().contains_key("authorization"));
                let body: HashMap<_, _> = Url::parse(&format!("http://localhost/?{}", std::str::from_utf8(request.body()).unwrap())).unwrap().query_pairs().into_owned().collect();
                assert_eq!(body["client_id"], "client");
                assert_eq!(body["client_secret"], "client-secret");
                assert_eq!(body["code"], "code+value");
                assert_eq!(body["code_verifier"], "verifier");
                assert_eq!(body["redirect_uri"], REDIRECT);
                assert_eq!(body["grant_type"], "authorization_code");
                Ok::<_, std::io::Error>(Response::builder().header(CONTENT_TYPE, "application/json")
                    .body(br#"{"access_token":"access","refresh_token":"refresh","token_type":"Bearer","expires_in":3600}"#.to_vec()).unwrap())
            }).await.unwrap();
        let session = Session::from_token(client(), token).unwrap();
        assert_eq!(session.access_token(), "access");
        assert_eq!(session.refresh_token(), Some("refresh"));
    }

    #[test]
    fn session_rejects_missing_permission_or_invalid_tokens_and_redacts_secrets() {
        let session = Session::from_token(client(), token(r#"{"access_token":"sensitive-access","refresh_token":"sensitive-refresh","token_type":"Bearer","expires_in":3600}"#)).unwrap();
        let debug = format!("{session:?}");
        assert!(!debug.contains("sensitive-access"));
        assert!(!debug.contains("sensitive-refresh"));
        assert!(!debug.contains("client-secret"));
        assert!(matches!(
            Session::from_token(
                client(),
                token(
                    r#"{"access_token":"access","token_type":"Bearer","expires_in":3600,"scope":"email"}"#
                )
            ),
            Err(AuthError::MissingScope)
        ));
        assert!(matches!(
            Session::from_token(
                client(),
                token(r#"{"access_token":"access","token_type":"Bearer"}"#)
            ),
            Err(AuthError::InvalidToken)
        ));
        assert!(matches!(
            Session::from_token(
                client(),
                token(r#"{"access_token":"","token_type":"Bearer","expires_in":3600}"#)
            ),
            Err(AuthError::InvalidToken)
        ));
    }

    #[test]
    fn refresh_preserves_or_rotates_refresh_token_and_retains_session_on_failure() {
        let mut session = Session::from_token(client(), token(r#"{"access_token":"old","refresh_token":"original","token_type":"Bearer","expires_in":3600}"#)).unwrap();
        session
            .update_token(token(
                r#"{"access_token":"new","token_type":"Bearer","expires_in":3600}"#,
            ))
            .unwrap();
        assert_eq!(session.access_token(), "new");
        assert_eq!(session.refresh_token(), Some("original"));
        session.update_token(token(r#"{"access_token":"rotated","refresh_token":"replacement","token_type":"Bearer","expires_in":3600}"#)).unwrap();
        assert_eq!(session.refresh_token(), Some("replacement"));
        assert!(session.update_token(token(r#"{"access_token":"invalid","token_type":"Bearer","expires_in":3600,"scope":"email"}"#)).is_err());
        assert_eq!(session.access_token(), "rotated");
        assert_eq!(session.refresh_token(), Some("replacement"));
    }

    #[tokio::test]
    async fn refresh_without_refresh_token_requires_sign_in() {
        let mut session = Session::from_token(
            client(),
            token(r#"{"access_token":"access","token_type":"Bearer","expires_in":3600}"#),
        )
        .unwrap();
        assert!(matches!(
            session.refresh().await,
            Err(AuthError::SignInRequired)
        ));
    }

    async fn token_server(
        status: u16,
        response: &'static str,
    ) -> (OAuthClient, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let client = client().set_token_uri(
            TokenUrl::new(format!("http://{}/token", listener.local_addr().unwrap())).unwrap(),
        );
        let task = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = read_request(&mut stream).await.unwrap().into_bytes();
            let header_end = request
                .windows(4)
                .position(|chunk| chunk == b"\r\n\r\n")
                .unwrap()
                + 4;
            let headers = std::str::from_utf8(&request[..header_end]).unwrap();
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();

            while request.len() < header_end + length {
                let mut chunk = [0; 1024];
                let count = stream.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                request.extend_from_slice(&chunk[..count]);
            }

            let body: HashMap<_, _> = Url::parse(&format!(
                "http://localhost/?{}",
                std::str::from_utf8(&request[header_end..]).unwrap()
            ))
            .unwrap()
            .query_pairs()
            .into_owned()
            .collect();
            assert_eq!(body["grant_type"], "refresh_token");
            assert_eq!(body["refresh_token"], "saved-refresh");
            let response = format!(
                "HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",
                response.len()
            );
            stream.write_all(response.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
        });
        (client, task)
    }

    #[tokio::test]
    async fn missing_saved_login_returns_none_without_network_access() {
        assert!(
            restore_from_store(client(), TokenStore::memory())
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn restore_refreshes_access_and_persists_rotated_refresh_token() {
        let store = TokenStore::memory();
        store.save("saved-refresh").await.unwrap();
        let (client, server) = token_server(200, r#"{"access_token":"restored-access","refresh_token":"rotated-refresh","token_type":"Bearer","expires_in":3600}"#).await;
        let session = restore_from_store(client, store.clone())
            .await
            .unwrap()
            .unwrap();
        server.await.unwrap();
        assert_eq!(session.access_token(), "restored-access");
        assert_eq!(session.refresh_token(), Some("rotated-refresh"));
        assert_eq!(
            store.load().await.unwrap().as_deref(),
            Some("rotated-refresh")
        );
    }

    #[tokio::test]
    async fn refresh_preserves_saved_refresh_token_when_google_omits_replacement() {
        let store = TokenStore::memory();
        let (client, server) = token_server(
            200,
            r#"{"access_token":"renewed-access","token_type":"Bearer","expires_in":3600}"#,
        )
        .await;
        let mut session = Session::from_token(client, token(r#"{"access_token":"old-access","refresh_token":"saved-refresh","token_type":"Bearer","expires_in":3600}"#)).unwrap();
        session.store = store.clone();
        session.persist().await.unwrap();
        session.refresh().await.unwrap();
        server.await.unwrap();
        assert_eq!(session.access_token(), "renewed-access");
        assert_eq!(
            store.load().await.unwrap().as_deref(),
            Some("saved-refresh")
        );
    }

    #[tokio::test]
    async fn revoked_saved_token_is_deleted_but_server_failures_keep_it() {
        for (status, response, revoked) in [
            (400, r#"{"error":"invalid_grant"}"#, true),
            (503, r#"{"error":"temporarily_unavailable"}"#, false),
        ] {
            let store = TokenStore::memory();
            store.save("saved-refresh").await.unwrap();
            let (client, server) = token_server(status, response).await;
            let result = restore_from_store(client, store.clone()).await;
            server.await.unwrap();

            if revoked {
                assert!(result.unwrap().is_none());
                assert!(store.load().await.unwrap().is_none());
            } else {
                assert!(matches!(result, Err(AuthError::TokenRejected)));
                assert_eq!(
                    store.load().await.unwrap().as_deref(),
                    Some("saved-refresh")
                );
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn test_session(token_url: &str, lifetime: Duration) -> Session {
    let client = google_client(
        "test-client".into(),
        "test-secret".into(),
        "http://127.0.0.1",
    )
    .set_token_uri(TokenUrl::new(token_url.to_owned()).unwrap());
    let token = serde_json::from_value(serde_json::json!({
        "access_token": "old-access",
        "refresh_token": "saved-refresh",
        "token_type": "Bearer",
        "expires_in": lifetime.as_secs(),
    }))
    .unwrap();
    let mut session = Session::from_token(client, token).unwrap();
    session.store = TokenStore::memory();
    session
}
