# Agent notes

## After every change to the app

Whenever you change the app's code or behaviour (anything under `src/`, `Cargo.toml` dependencies, etc.):

1. **Bump the version first** in `Cargo.toml` (`version = "x.y.z"`): patch for fixes, minor for new features.
2. **Run the tests**: `cargo test`.
3. **Reinstall the binary** so the End key uses the new build: `cargo install --path .`
4. Check it: `stt --version` should print the new version.

The Hyprland binding runs `~/.cargo/bin/stt`, so an unreinstalled change isn't live.
