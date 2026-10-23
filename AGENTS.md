# Agent notes

## After every change to the app

Whenever you change the app's code or behaviour (anything under `src/`, `Cargo.toml` dependencies, etc.):

1. **Bump the version first** in `Cargo.toml` (`version = "x.y.z"`): patch for fixes, minor for new features.
2. **Run the tests**: `cargo test`.
3. **Run the linter**: `cargo clippy --all-targets` must print no warnings.
4. **Reinstall the binary** so the End key uses the new build: `cargo install --path .`
5. Check it: `stt --version` should print the new version.

The Hyprland binding runs `~/.cargo/bin/stt`, so an unreinstalled change isn't live.

## Style

Follow the Google Rust style guide, saved in `docs/rust-style-guide.md`. The
mechanical parts are enforced by the `[lints.clippy]` table in `Cargo.toml`
(plus `clippy.toml`) and by `tests/style.rs` (typed `let` bindings, one module
per `use`); the rest (naming, module layout, iterator chains vs.
loops, generics vs. `dyn`) is on you when writing code. Don't silence a lint
with `#[allow]` without a comment saying why.
