//! `config/authme/config.toml`, beside `server.properties`. Written with its defaults the
//! first time the server opens a world with the mod. Names follow AuthMe's `config.yml` where
//! AuthMe has the same setting.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// `en` (English, the default) or `uk` (Ukrainian): the language of every message.
    pub language: String,
    /// Players must register before they may play (AuthMe `forceRegistration`... always on in
    /// AuthMe's default). False: unregistered players play freely and may register if they wish.
    pub force_registration: bool,
    /// Seconds a player has to log in or register before being kicked (AuthMe
    /// `restrictions.timeout`); 0 never kicks.
    pub login_timeout_secs: u32,
    /// Wrong passwords in a row before the player is kicked (AuthMe `kickOnWrongPassword`
    /// kicks at the first; here 1 is that); 0 never kicks.
    pub max_login_attempts: u32,
    /// Shortest and longest password (AuthMe `minPasswordLength`, `passwordMaxLength`).
    pub min_password_length: usize,
    pub max_password_length: usize,
    /// Passwords refused at registration and change (AuthMe `unsafePasswords`), compared
    /// case-insensitively. The player's own name is always refused.
    pub unsafe_passwords: Vec<String>,
    /// Shortest and longest name, and the characters it may use (AuthMe
    /// `minNicknameLength`, `maxNicknameLength`, `allowedNicknameCharacters` as a plain set).
    pub min_nickname_length: usize,
    pub max_nickname_length: usize,
    pub allowed_nickname_characters: String,
    /// Refuse a join whose name differs only in letter case from a registered account (AuthMe
    /// `preventOtherCase`). Offline servers give `Roman` and `roman` different UUIDs, so this
    /// keeps one person from owning both.
    pub prevent_other_case: bool,
    /// Until they log in, players are put in spectator mode (no inventory, no drops, no
    /// pick-ups, no damage) and given back their own mode afterwards. Strongly advised: Horizon
    /// has no hook to stop every way of throwing items out of an inventory.
    pub limbo_spectator: bool,
    /// Blindness until they log in (AuthMe `applyBlindEffect`).
    pub apply_blindness: bool,
    /// Keep players where they joined until they log in (AuthMe `allowMovement: false`).
    pub freeze: bool,
    /// Blocks a frozen player may stray before being put back (AuthMe `allowedMovementRadius`).
    pub allowed_movement_radius: f64,
    /// Seconds between reminders "/login <password>" (AuthMe `messageInterval`); 0: once.
    pub message_interval_secs: u32,
    /// Commands an unauthenticated player may still run, besides the mod's own (AuthMe
    /// `allowCommands`), without the slash.
    pub allow_commands: Vec<String>,
    /// scrypt cost of new hashes: N = 2^scrypt_log_n, r, p. Raising it rehashes passwords at
    /// their next login.
    pub scrypt_log_n: u8,
    pub scrypt_r: u32,
    pub scrypt_p: u32,
    /// Hashes computed at once (each takes 128 * N * r bytes, 32 MiB by default). More
    /// requests wait for the next tick.
    pub max_parallel_hashes: usize,
    /// Operator level for `/authme` (AuthMe's admin permissions).
    pub admin_level: u8,
    /// Tell everyone when a player registers.
    pub broadcast_registration: bool,
    /// Sessions (AuthMe `sessions` / `timeout`): a player who joins with the same name from
    /// the IP address of their last password login, within this many minutes of their last
    /// login, is logged in without a password. 0 turns sessions off.
    pub session_minutes: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            language: "en".into(),
            force_registration: true,
            login_timeout_secs: 180,
            max_login_attempts: 3,
            min_password_length: 6,
            max_password_length: 64,
            unsafe_passwords: [
                "123456", "1234567", "12345678", "123456789", "password", "qwerty", "qwerty123",
                "111111", "000000", "123123", "minecraft", "abc123", "йцукен",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            min_nickname_length: 3,
            max_nickname_length: 16,
            allowed_nickname_characters:
                "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_".into(),
            prevent_other_case: true,
            limbo_spectator: true,
            apply_blindness: true,
            freeze: true,
            allowed_movement_radius: 0.5,
            message_interval_secs: 10,
            allow_commands: Vec::new(),
            scrypt_log_n: 15,
            scrypt_r: 8,
            scrypt_p: 3,
            max_parallel_hashes: 4,
            admin_level: 3,
            broadcast_registration: false,
            session_minutes: 4320,
        }
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut c: Config = toml::from_str(text).map_err(|e| e.to_string())?;
        c.sanitize();
        Ok(c)
    }

    pub fn to_toml(&self) -> String {
        let body = toml::to_string_pretty(self).unwrap_or_default();
        format!(
            "# AuthMe for Horizon (port of AuthMeReloaded, GPL-3.0). See README.md for every key.\n\
             # Changes apply after `/authme reload` or a restart.\n\n{body}"
        )
    }

    fn sanitize(&mut self) {
        // An unknown language is English.
        self.language = super::messages::Language::parse(&self.language)
            .unwrap_or_default()
            .code()
            .to_owned();
        self.min_password_length = self.min_password_length.clamp(1, 128);
        self.max_password_length = self.max_password_length.clamp(self.min_password_length, 256);
        self.min_nickname_length = self.min_nickname_length.clamp(1, 16);
        self.max_nickname_length = self.max_nickname_length.clamp(self.min_nickname_length, 16);
        self.max_parallel_hashes = self.max_parallel_hashes.clamp(1, 32);
        self.admin_level = self.admin_level.clamp(1, 4);
        if !self.allowed_movement_radius.is_finite() || self.allowed_movement_radius < 0.0 {
            self.allowed_movement_radius = 0.5;
        }
        // Names go into server commands (`tp`, `gamemode`, `kick`): never let a character that
        // could break out of a command argument through, whatever the file says.
        self.allowed_nickname_characters.retain(|c| c.is_ascii_alphanumeric() || c == '_');
        if self.allowed_nickname_characters.is_empty() {
            self.allowed_nickname_characters = Config::default().allowed_nickname_characters;
        }
        self.allow_commands = self
            .allow_commands
            .iter()
            .map(|c| c.trim().trim_start_matches('/').to_ascii_lowercase())
            .filter(|c| !c.is_empty())
            .collect();
    }

    pub fn cost(&self) -> super::hash::Cost {
        super::hash::Cost {
            log_n: self.scrypt_log_n,
            r: self.scrypt_r,
            p: self.scrypt_p,
        }
        .clamped()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip() {
        let c = Config::default();
        assert_eq!(Config::parse(&c.to_toml()).unwrap(), c);
    }

    #[test]
    fn english_by_default_and_for_unknown_codes() {
        assert_eq!(Config::default().language, "en");
        assert_eq!(Config::parse("").unwrap().language, "en");
        assert_eq!(Config::parse("language = \"UA\"\n").unwrap().language, "uk");
        assert_eq!(Config::parse("language = \"de\"\n").unwrap().language, "en");
    }

    #[test]
    fn partial_file_keeps_defaults() {
        let c = Config::parse("language = \"en\"\nlogin_timeout_secs = 60\n").unwrap();
        assert_eq!(c.language, "en");
        assert_eq!(c.login_timeout_secs, 60);
        assert!(c.freeze);
    }

    #[test]
    fn unsafe_name_characters_are_dropped() {
        let c = Config::parse("allowed_nickname_characters = \"abc ;@\\\"\"\n").unwrap();
        assert_eq!(c.allowed_nickname_characters, "abc");
    }

    #[test]
    fn unknown_key_is_an_error() {
        assert!(Config::parse("pasword_hash = 1").is_err());
    }
}
