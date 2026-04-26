# vorevault-desktop

Desktop tray client for [VoreVault](https://github.com/Bullmoose-Code/vorevault). Watches a folder and uploads new files to your VoreVault instance via the existing tus endpoint.

## Status

v0.1.0 — Sub-project A: scaffold + Discord OAuth + OS keychain. **Does not yet upload anything**; that's Sub-project B.

## Build

Requires:
- Rust stable (`rustup default stable`)
- Tauri prerequisites for your OS: <https://tauri.app/start/prerequisites/>

```bash
cargo install tauri-cli --version "^2"
cargo tauri build
```

Built artifacts land in `src-tauri/target/release/bundle/`.

## Run in dev

```bash
cargo tauri dev
```

## Configuration

| Env var | Default | Description |
|---|---|---|
| `VAULT_URL` | `https://vault.bullmoosefn.com` | Override for testing against staging |

## License

MIT — see `LICENSE`.

## TODO

- [ ] Replace `src-tauri/icons/tray.png` placeholder with a hand-authored 22×22 template-mode icon matching the VoreVault brand.
