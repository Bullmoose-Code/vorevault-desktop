use keyring::Entry;

const SERVICE: &str = "fn.bullmoose.vorevault.desktop";
const ACCOUNT: &str = "session";

/// Store the session token in the OS keychain. Overwrites any existing value.
pub fn store(token: &str) -> keyring::Result<()> {
    Entry::new(SERVICE, ACCOUNT)?.set_password(token)
}

/// Load the session token from the OS keychain. Returns `Ok(None)` when no
/// entry exists (vs. `Err` for actual keychain access failures), so callers
/// can distinguish "user is signed out" from "couldn't reach the keychain."
pub fn load() -> keyring::Result<Option<String>> {
    match Entry::new(SERVICE, ACCOUNT)?.get_password() {
        Ok(p) => Ok(Some(p)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e),
    }
}

/// Delete the session token from the OS keychain. Idempotent — calling on a
/// missing entry succeeds silently.
pub fn delete() -> keyring::Result<()> {
    match Entry::new(SERVICE, ACCOUNT)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    // We don't run keychain tests in CI because GitHub-hosted runners don't
    // have a logged-in keychain session by default and asking for one would
    // require interactive setup. Manual smoke test on dev machine instead.
    //
    // No unit tests here — keychain wrapper is too thin to test without
    // mocking the OS layer, and the semantics are tested manually via the
    // sign-in flow during the smoke test in Task 15.
}
