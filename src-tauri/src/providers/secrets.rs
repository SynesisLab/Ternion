//! API-key storage for endpoint profiles (design §5.1 + §10.1): secrets live
//! in Windows Credential Manager via the `keyring` crate; the DB stores only
//! the handle (`api_key_ref`, `endpoint.<profile id>`), and the secret is read
//! only at request time. Nothing here logs values.

const SERVICE: &str = "com.paperplane.ternion";

/// The handle stored in `endpoint_profiles.api_key_ref` for a profile id.
pub fn ref_for(profile_id: &str) -> String {
    format!("endpoint.{profile_id}")
}

fn entry(key_ref: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, key_ref).map_err(|e| format!("keyring entry: {e}"))
}

/// True when a secret is present. Errors (lockouts, platform quirks) read as
/// "absent" — the request path will surface a clearer failure if it matters.
pub fn has_api_key(key_ref: &str) -> bool {
    matches!(read_api_key(key_ref), Ok(Some(_)))
}

pub fn read_api_key(key_ref: &str) -> Result<Option<String>, String> {
    match entry(key_ref)?.get_password() {
        Ok(secret) => Ok(Some(secret)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("keyring read: {e}")),
    }
}

pub fn store_api_key(key_ref: &str, secret: &str) -> Result<(), String> {
    entry(key_ref)?
        .set_password(secret)
        .map_err(|e| format!("keyring store: {e}"))
}

pub fn delete_api_key(key_ref: &str) -> Result<(), String> {
    match entry(key_ref)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keyring delete: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip through Windows Credential Manager. Uses a throwaway entry
    /// name and cleans up after itself.
    #[test]
    fn store_read_delete_roundtrip() {
        let key_ref = ref_for("test_profile_zzz");
        assert_eq!(read_api_key(&key_ref).unwrap(), None);
        assert!(!has_api_key(&key_ref));

        store_api_key(&key_ref, "sk-test-123").unwrap();
        assert_eq!(read_api_key(&key_ref).unwrap().as_deref(), Some("sk-test-123"));
        assert!(has_api_key(&key_ref));

        // Overwrite in place.
        store_api_key(&key_ref, "sk-test-456").unwrap();
        assert_eq!(read_api_key(&key_ref).unwrap().as_deref(), Some("sk-test-456"));

        delete_api_key(&key_ref).unwrap();
        assert_eq!(read_api_key(&key_ref).unwrap(), None);
        // Deleting a missing entry is a no-op.
        delete_api_key(&key_ref).unwrap();
    }

    #[test]
    fn ref_names_are_namespaced_per_profile() {
        assert_eq!(ref_for("ep_abc"), "endpoint.ep_abc");
    }
}