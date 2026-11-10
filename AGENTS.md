# Repository Guidelines

## Project Structure & Module Organization

Lumina is a Rust 2024 desktop client for YouTube Music using Iced. The Cargo workspace includes `crates/*`, with `crates/app` as the default member.

- `crates/app/src/`: application startup, Iced views, messages, and orchestration.
- `crates/ytm/src/`: authenticated client, playlist models, OAuth, and secure refresh-token persistence.
- `crates/playback/src/lib.rs`: empty library reserved for audio playback and playback events.
- `.github/workflows/ci.yml`: formatting and builds on Linux, Windows, and macOS.
- `flake.nix`: optional Nix development environment and checks.

There are no dedicated test or asset directories yet. Keep UI dependencies in `app`; keep service and playback implementations in their respective libraries. The README still describes GPUI Kit; follow the current Iced implementation and Cargo manifests when they disagree.

## Build, Test, and Development Commands

- `cargo run --locked`: launch the desktop app.
- `cargo build --workspace --locked`: build every crate, matching CI.
- `cargo check --workspace --all-targets --locked`: type-check all targets.
- `cargo fmt --all -- --check`: verify Rust formatting; use `cargo fmt --all` to apply it.
- `cargo test --workspace --locked`: run unit, integration, and documentation tests.
- `nix develop`: enter the optional development shell.
- `nix flake check`: run configured Nix checks, including Taplo, cargo-deny, and nextest.

## Coding Style & Naming Conventions

Use rustfmt defaults, including four-space indentation. Use `snake_case` for modules and functions, `PascalCase` for types, and `SCREAMING_SNAKE_CASE` for constants. Keep modules focused on one responsibility. Declare shared dependency versions in the root `Cargo.toml` and inherit them with `workspace = true`. Format TOML with `taplo fmt`; `taplo.toml` configures dependency-key sorting.

Separate control-flow expressions (`if`, `match`, `loop`, `for`, `while`, and `let ... else`) from adjacent statements with a blank line, including assignments whose value is a control-flow expression. Do not insert blank lines between `match` arms. Insert a blank line before a standalone `return` when it follows another statement; no blank line is needed when `return` is the first statement in a block or appears directly in a match arm. Keep `else` attached to its `if`, and do not add empty lines directly inside opening or closing braces solely for this rule.

## Testing Guidelines

Use Rust's built-in test framework. Place unit tests in `#[cfg(test)] mod tests` alongside implementation; place integration tests in each crate's `tests/` directory when needed. Name tests after observable behavior, such as `removing_earlier_entry_preserves_current_track`. No coverage threshold is configured. Manually verify window rendering and input for UI changes; compilation alone does not validate desktop behavior.

## Commit & Pull Request Guidelines

The repository has no commits yet, so no commit-message convention is established. Use concise, imperative subjects such as `Add playback event handling`. PRs should explain the change, list validation performed, link relevant issues, and include screenshots for visible UI changes. Note any platform-specific limitations.
