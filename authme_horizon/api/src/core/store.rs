//! The accounts, `config/authme/accounts.json`, and the players held in limbo,
//! `config/authme/limbo.json` (their game mode before the mod made them spectators, so it
//! comes back even after a crash). Both are written whole to a temporary file and renamed
//! over the old one, so a crash mid-write never leaves half a file.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Account {
    /// The name as registered, with its letter case (AuthMe `realname`).
    pub realname: String,
    /// `$scrypt$...` or an imported AuthMe `$SHA$...`.
    pub hash: String,
    /// Unix seconds.
    #[serde(default)]
    pub registered: u64,
    #[serde(default)]
    pub last_login: Option<u64>,
    #[serde(default)]
    pub email: Option<String>,
    /// The IP address of the last password login, for sessions; cleared by /logout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_ip: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Accounts {
    #[serde(default = "one")]
    pub version: u32,
    /// By lower-case name.
    #[serde(default)]
    pub accounts: BTreeMap<String, Account>,
}

fn one() -> u32 {
    1
}

impl Accounts {
    pub fn get(&self, name: &str) -> Option<&Account> {
        self.accounts.get(&name.to_ascii_lowercase())
    }
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Account> {
        self.accounts.get_mut(&name.to_ascii_lowercase())
    }
    pub fn insert(&mut self, account: Account) {
        self.accounts
            .insert(account.realname.to_ascii_lowercase(), account);
    }
    pub fn remove(&mut self, name: &str) -> Option<Account> {
        self.accounts.remove(&name.to_ascii_lowercase())
    }
}

/// A player the mod made a spectator: the mode to give back, by UUID (hex).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Limbo {
    #[serde(default)]
    pub game_modes: BTreeMap<String, i32>,
}

pub fn uuid_key(uuid: u128) -> String {
    format!("{uuid:032x}")
}

/// Reads JSON at `path`: the default when the file is missing; an error (never an empty
/// store over a broken file) when it can't be read or parsed.
pub fn load<T: Default + for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Writes `value` as JSON to `path`, through `path.tmp` and a rename.
pub fn save<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let tmp = tmp_path(path);
    fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

fn tmp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_load_round_trip() {
        let dir = std::env::temp_dir().join(format!("authme-store-{}", std::process::id()));
        let path = dir.join("accounts.json");
        let mut a = Accounts::default();
        a.insert(Account {
            realname: "Roman".into(),
            hash: "$scrypt$x".into(),
            registered: 1,
            last_login: None,
            email: None,
            last_ip: None,
        });
        save(&path, &a).unwrap();
        let b: Accounts = load(&path).unwrap();
        assert_eq!(a, b);
        assert!(b.get("ROMAN").is_some());
        assert!(!tmp_path(&path).exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn missing_is_empty_broken_is_error() {
        let dir = std::env::temp_dir().join(format!("authme-store2-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let a: Accounts = load(&dir.join("none.json")).unwrap();
        assert!(a.accounts.is_empty());
        fs::write(dir.join("bad.json"), "{not json").unwrap();
        assert!(load::<Accounts>(&dir.join("bad.json")).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
