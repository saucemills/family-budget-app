//! Where secrets (Plaid keys and bank connection tokens) are kept.
//! - Windows: Windows Credential Manager. macOS: the login Keychain.
//! - Linux: a file readable only by you, next to the budget file (desktop Linux
//!   keyrings vary too much between distros to rely on).

#[cfg(not(target_os = "linux"))]
mod imp {
    const SERVICE: &str = "app.familybudget.desktop";

    fn entry(name: &str) -> Result<keyring::Entry, String> {
        keyring::Entry::new(SERVICE, name).map_err(|e| format!("credential store unavailable: {e}"))
    }

    pub fn get(name: &str) -> Result<Option<String>, String> {
        match entry(name)?.get_password() {
            Ok(s) => Ok(Some(s)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("couldn't read from the credential store: {e}")),
        }
    }

    pub fn set(name: &str, value: &str) -> Result<(), String> {
        entry(name)?.set_password(value).map_err(|e| format!("couldn't save to the credential store: {e}"))
    }

    pub fn delete(name: &str) -> Result<(), String> {
        match entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::OnceLock;

    static PATH: OnceLock<PathBuf> = OnceLock::new();

    pub fn init(dir: PathBuf) {
        let _ = PATH.set(dir.join("secrets.json"));
    }

    fn read() -> Result<BTreeMap<String, String>, String> {
        let path = PATH.get().ok_or("secret store not ready")?;
        match std::fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).map_err(|e| e.to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn write(map: &BTreeMap<String, String>) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let path = PATH.get().ok_or("secret store not ready")?;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| e.to_string())?;
        f.write_all(serde_json::to_string(map).map_err(|e| e.to_string())?.as_bytes()).map_err(|e| e.to_string())
    }

    pub fn get(name: &str) -> Result<Option<String>, String> {
        Ok(read()?.get(name).cloned())
    }

    pub fn set(name: &str, value: &str) -> Result<(), String> {
        let mut m = read()?;
        m.insert(name.into(), value.into());
        write(&m)
    }

    pub fn delete(name: &str) -> Result<(), String> {
        let mut m = read()?;
        m.remove(name);
        write(&m)
    }
}

pub use imp::*;
