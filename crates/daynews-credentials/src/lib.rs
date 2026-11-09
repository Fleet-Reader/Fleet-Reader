// SPDX-License-Identifier: Apache-2.0
//! Native credential storage. Failure is explicit; there is no plaintext or mock fallback.
#[derive(Debug, Clone, Copy)]
pub struct Unavailable;

#[cfg(any(target_os = "android", target_env = "ohos"))]
mod mobile;

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "windows",
    all(target_os = "linux", not(target_env = "ohos"))
))]
mod platform {
    use super::Unavailable;
    fn entry(key: &str) -> Result<keyring::Entry, Unavailable> {
        keyring::Entry::new("org.appfair.app.FleetReader.sync", key).map_err(|_| Unavailable)
    }
    pub fn save(key: &str, password: &str) -> Result<(), Unavailable> {
        entry(key)?.set_password(password).map_err(|_| Unavailable)
    }
    pub fn load(key: &str) -> Result<Option<String>, Unavailable> {
        match entry(key)?.get_password() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(Unavailable),
        }
    }
    pub fn remove(key: &str) -> Result<(), Unavailable> {
        match entry(key)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(Unavailable),
        }
    }
}
#[cfg(any(target_os = "android", target_env = "ohos"))]
mod platform {
    use super::{Unavailable, mobile};
    pub fn save(key: &str, password: &str) -> Result<(), Unavailable> {
        if mobile::save(key, password).map_err(|_| Unavailable)? {
            Ok(())
        } else {
            Err(Unavailable)
        }
    }
    pub async fn load(key: &str) -> Result<Option<String>, Unavailable> {
        let v = mobile::load(key).await.map_err(|_| Unavailable)?;
        if v.is_empty() { Ok(None) } else { Ok(Some(v)) }
    }
    pub fn remove(key: &str) -> Result<(), Unavailable> {
        if mobile::remove(key).map_err(|_| Unavailable)? {
            Ok(())
        } else {
            Err(Unavailable)
        }
    }
}
#[cfg(target_arch = "wasm32")]
mod platform {
    use super::Unavailable;
    pub fn save(_: &str, _: &str) -> Result<(), Unavailable> {
        Err(Unavailable)
    }
    pub fn load(_: &str) -> Result<Option<String>, Unavailable> {
        Err(Unavailable)
    }
    pub fn remove(_: &str) -> Result<(), Unavailable> {
        Err(Unavailable)
    }
}
pub use platform::{load, remove, save};
