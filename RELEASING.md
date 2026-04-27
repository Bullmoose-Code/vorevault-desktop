# Releasing VoreVault Desktop

Tag-driven release flow. Push a `v*` tag → GitHub Actions builds + signs + publishes a Release with `.msi` (Windows) and `.dmg` (macOS, universal) artifacts plus a signed `latest.json` manifest. Installed clients pick up the new version via `tauri-plugin-updater` on their next launch.

See `docs/superpowers/specs/2026-04-26-desktop-watcher-subproject-e-design.md` (in the `vorevault` web repo) for the architectural background.

## Prerequisites (one-time, already done as of v0.5.0)

- Updater keypair generated and stored in 1Password + offline backup.
- GH secrets `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` set in the repo.
- Public key embedded in `src-tauri/tauri.conf.json` `plugins.updater.pubkey`.
- Repo is public so the updater endpoint can fetch release assets anonymously.

## Cutting a release

> ⚠ **The tag name MUST match the version in `tauri.conf.json` and `Cargo.toml`.** If they differ (e.g., bumped to `0.5.1` but tagged `v0.5.2`), the artifact filenames will be wrong and the updater manifest's version field will mismatch the actual binary version. Installed clients will either skip the update or fail signature verification. Double-check before pushing the tag.

1. **Bump the version in 3 files.** Pick the new version (e.g., `0.5.1`):
   - `src-tauri/tauri.conf.json` — `"version": "0.5.1"`
   - `src-tauri/Cargo.toml` — `version = "0.5.1"`
   - `cd src-tauri && cargo build` — regenerates `Cargo.lock` with the new version.

2. **Stage all three:**
   ```bash
   git add src-tauri/tauri.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock
   git commit -m "chore: bump to v0.5.1"
   ```

3. **Annotated tag:**
   ```bash
   git tag -a v0.5.1 -m "v0.5.1 — short description of changes"
   ```

4. **Push:**
   ```bash
   git push origin main
   git push origin v0.5.1
   ```

5. **Wait ~10–15 min.** The release workflow runs (Windows job ~25 min wall, Mac job ~5–8 min). Both must succeed before the release is promoted from draft to published.

6. **Smoke test.**
   - Download the new `.msi` or `.dmg` from the GitHub Release page.
   - Verify it installs and reports the new version.
   - From an existing prior install: open settings → click "check now" → should download in background → next quit/launch should silently update.

7. **Announce on Discord** with a link to the Release.

## Pre-release / RC tags

For testing the release workflow without affecting installed users, use tags with a **numeric-only** prerelease identifier (e.g., `v0.5.1-1`, `v0.5.1-2`). The workflow detects the `-` and marks the GitHub Release as **prerelease**. The updater endpoint `/releases/latest/download/latest.json` excludes prereleases, so installed users never pick them up.

> ⚠ **The prerelease identifier MUST be numeric-only** (and ≤ 65535). The Windows MSI bundler rejects alphabetic identifiers like `-rc.1`/`-beta.2` with: *"optional pre-release identifier in app version must be numeric-only and cannot be greater than 65535 for msi target"*. Stick to bare integers: `-1`, `-2`, etc. (We hit this on the first attempt at v0.5.0-rc.1.)

When the RC validates, either:
- Flip the prerelease flag off in the GitHub Releases UI (`gh release edit v0.5.1-1 --prerelease=false`), OR
- Bump to the final version and tag `v0.5.1` directly (preferred — cleaner release page).

## If things go wrong

**The release was published but artifacts are broken:**

```bash
gh release delete v0.5.1 --yes --cleanup-tag
git push origin :refs/tags/v0.5.1
# Fix the issue, commit, re-tag, re-push.
```

⚠ Anyone who already auto-updated to the broken version will be stuck until the next good release ships. Communicate on Discord.

**A `release.yml` run failed mid-way:**

The workflow leaves a **draft** release in place. To retry: delete the draft (`gh release delete vX.Y.Z --yes`), fix the cause (often a transient runner issue or a GH secrets problem), then re-push the tag (`git push origin :refs/tags/vX.Y.Z && git push origin vX.Y.Z`).

**You lost the updater private key:**

There is no recovery. You must:
1. Generate a new keypair locally.
2. Update GH secrets to the new private key + passphrase.
3. Update `src-tauri/tauri.conf.json` with the new pubkey.
4. Cut a new release.
5. **Every existing installed user must manually download + reinstall** to pick up the new pubkey. Their old binaries will reject signatures from the new key as a security failure.

This is why the key lives in 1Password + an offline backup. Do not lose it.

## Known untested paths

The following are NOT covered by automated tests:

- **Updater signature mismatch behavior.** The plugin will refuse the install and the app will show "couldn't check (signature mismatch)" — this code path has only been reasoned about, not exercised.
- **Disk-full / permission-denied install failures.** Rely on the plugin's error reporting + the generic `Error` state in the settings window.

If you encounter either in production, capture the log and add the case to this document.
