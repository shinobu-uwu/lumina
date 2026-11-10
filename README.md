# Lumina

A desktop app built with [GPUI Kit](https://gpui-kit.com/docs/getting-started/), targeting Linux, Windows, and macOS.

## Prerequisites

Install Rust 1.92 or later and Cargo. Use the checked-in `Cargo.lock` for reproducible dependency resolution. See the [GPUI Kit installation guide](https://gpui-kit.com/docs/installation/) for upstream requirements.

- **Windows 10 or later:** install Visual Studio 2022 Build Tools or Community with the **Desktop development with C++** workload, MSVC, and a Windows SDK. Install CMake and ensure it is on `PATH`. Use an MSVC Rust toolchain (for example, `stable-x86_64-pc-windows-msvc`). A Visual Studio Developer PowerShell can provide the native build environment.
- **macOS 15 or later:** install Xcode Command Line Tools with `xcode-select --install` and verify the selected developer directory with `xcode-select -p`. GPUI Kit already enables the platform's `font-kit` text backend.
- **Linux:** install a C/C++ compiler, Clang, pkg-config, and the native libraries below, and run in a graphical X11 or Wayland session with a working Vulkan driver.

On Ubuntu 24.04:

```sh
sudo apt update
sudo apt install -y gcc g++ clang pkg-config libfontconfig-dev libwayland-dev \
  libwebkit2gtk-4.1-dev libxkbcommon-x11-dev libx11-xcb-dev \
  libssl-dev libzstd-dev vulkan-validationlayers libvulkan1
```

The repository also provides an optional Nix development shell on Linux and macOS: `nix develop` (or `direnv allow` with nix-direnv installed).

## Run and build

From the repository root, on any supported platform:

```sh
cargo run --locked
cargo build --locked --release
```

The starter window displays “Lumina” and a GPUI Kit button. Clicking the button prints `Clicked!` when a console is available. Windows builds use the GUI subsystem to avoid opening a separate console window.

The app uses GPUI Kit's default features, bundled icon assets, theme initialization, and platform-aware window helpers. It does not need separate GPUI dependencies or platform-specific startup implementations.

## Google sign-in

Lumina's current implementation uses Iced. The login screen opens the system
browser and uses Google's desktop OAuth authorization-code flow with PKCE and a
temporary callback on `127.0.0.1`. It requests read-only YouTube access. This
authorizes the YouTube Data API; it does not create a YouTube Music browser
session or authorize native audio streaming.

Users click **Continue with Google** and sign in in their browser. They do not
need a Google Cloud project or OAuth credentials of their own.

Maintainer setup (once for Lumina):

1. Create a Google Cloud project and enable **YouTube Data API v3**.
2. Configure the Google OAuth consent screen. While the app is in testing, add
   your Google account as a test user.
3. Create an OAuth client with application type **Desktop app**.
4. Download that client's JSON and save it as `google-credential.json` in the
   repository root. This file is ignored by Git.
5. Start Lumina and click **Continue with Google**:

```sh
cargo run --locked
```

The `ytm` crate owns credential loading, browser launch, and the OAuth flow.
It uses the `oauth2` crate for PKCE, authorization URLs, token exchange, and refresh;
the local browser callback and UI cancellation remain Lumina's responsibility.
The `app` crate holds a `ytm::Client`, starts sign-in, restores the saved session
on startup, and requests sign-out. Its login screen owns restoration, login tasks,
cancellation, and errors in `crates/app/src/app/screens/login.rs`. `App` only routes
screens and owns the connected client. It never handles tokens or credential-store
details. Credentials always
come from `google-credential.json`; environment overrides are not used. The JSON
must contain the Desktop client's `installed` object with `client_id` and
`client_secret`.

The `ytm` crate embeds the JSON at compile time with `include_str!`. The file must
exist in the repository root when building or testing, including in CI. The
compiled application needs no credentials file at runtime. Rebuild after changing
the JSON to update the embedded credentials.

No hosted callback service is needed. The app chooses an available loopback port.
Sign-in can be cancelled and times out after five minutes waiting for the callback.
The `ytm` crate uses `keyring` to persist refresh tokens in the native credential
store (Secret Service on Linux, Keychain on macOS, Credential Manager on Windows).
Access tokens stay in memory. Startup restores the saved refresh token and obtains
a fresh access token without opening a browser. The credential entry uses service
`lumina.youtube-music` and the OAuth client ID as its key; currently one account
is saved per client.

`ytm::auth::Session::refresh()` persists refresh-token replacements and retains
the previous refresh token if Google omits a replacement. Revoked credentials are
removed and require another sign-in; temporary failures preserve the saved login.
**Sign out** deletes the saved credential before clearing the UI session, without
revoking Google's grant. Credential-store failures are shown in the UI; on Linux,
a running and unlocked Secret Service is needed. No plaintext token file is used.
`ytm::Client` refreshes tokens on demand before they expire and retries a failed
request once after HTTP 401. Clones share the session and refresh lock. Revoked
credentials return `ytm::Error::SignInRequired`; permission and quota failures do
not trigger refreshes.

### Fetch playlists

```rust
let client = ytm::Client::sign_in().await?;
let playlists = client.playlists().await?;

for playlist in playlists {
    println!("{} ({} items)", playlist.title, playlist.item_count);
}
```

`Client::restore_session()` restores a saved login without a browser, and
`client.sign_out()` deletes it and invalidates every clone. `playlists()` follows
all pages and returns the account's owned YouTube playlists, including private
ones. It does not include the full YouTube Music saved-library experience. This
uses [`playlists.list`](https://developers.google.com/youtube/v3/docs/playlists/list)
with `mine=true` and `maxResults=50`.

Reference: [Google's desktop OAuth guide](https://developers.google.com/identity/protocols/oauth2/native-app).

## Validation

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo build --workspace --locked
```

GitHub Actions builds on Linux, Windows, and macOS. Native window rendering and input still require a manual run on each platform; compilation alone does not verify the GUI.
