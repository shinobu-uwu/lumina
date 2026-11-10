use serde::Deserialize;

const CREDENTIALS_JSON: &str = include_str!("../../../google-credential.json");

#[derive(Deserialize)]
pub(crate) struct GoogleCredentials {
    pub client_id: String,
    pub client_secret: String,
}

impl GoogleCredentials {
    pub fn load() -> Result<Self, &'static str> {
        let downloaded: DownloadedCredentials = serde_json::from_str(CREDENTIALS_JSON).map_err(
            |_| "google-credential.json must contain Google Desktop app OAuth credentials",
        )?;
        let credentials = Self {
            client_id: downloaded.installed.client_id.trim().to_owned(),
            client_secret: downloaded.installed.client_secret.trim().to_owned(),
        };

        if credentials.client_id.is_empty() || credentials.client_secret.is_empty() {
            return Err("google-credential.json contains empty client credentials");
        }

        Ok(credentials)
    }
}

#[derive(Deserialize)]
struct DownloadedCredentials {
    installed: GoogleCredentials,
}
