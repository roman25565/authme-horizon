//! The rules: who is logged in, what each command does, what happens every tick. Pure Rust
//! with no game types, so it is tested on its own (`core-tests/`). The game glue (`lib.rs`)
//! feeds it the hooks and carries out the [`Effect`]s it returns.

use super::config::Config;
use super::hash::{self, Cost};
use super::messages::{Lang, Msg};
use super::store::{self, Account, Accounts, Limbo, uuid_key};
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How a line to a player looks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Info,
    Good,
    Bad,
}

/// What the glue does for the rules.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// A chat line to one player.
    Tell(u128, String, Tone),
    /// A line over the player's hotbar.
    ActionBar(u128, String),
    /// A line to every player.
    Broadcast(String),
    /// A line on the server console (and the log).
    Console(String),
    /// A command run as the server, at the next server tick.
    Command(String),
}

/// A player on the server, as the server tick sees them.
#[derive(Clone, Debug, PartialEq)]
pub struct Online {
    pub uuid: u128,
    pub name: String,
    pub dimension: String,
    pub position: [f64; 3],
    pub game_mode: i32,
    pub entity_id: i32,
}

/// Slow password work, done off the server thread.
#[derive(Debug)]
pub struct Job {
    /// Who asked: a player, or the console (`None`).
    pub actor: Option<u128>,
    pub kind: JobKind,
    pub cost: Cost,
}

#[derive(Debug)]
pub enum JobKind {
    Login { name: String, password: String, stored: String },
    Register { name: String, password: String },
    ChangePassword { name: String, old: String, new: String, stored: String },
    Unregister { name: String, password: String, stored: String },
    AdminSet { name: String, password: String, register: bool },
}

/// A finished [`Job`].
#[derive(Debug)]
pub struct Done {
    pub actor: Option<u128>,
    pub kind: DoneKind,
}

#[derive(Debug)]
pub enum DoneKind {
    Login { name: String, ok: bool, stored: String, rehash: Option<String> },
    Register { name: String, hash: Result<String, String> },
    ChangePassword { name: String, ok: bool, stored: String, hash: Option<Result<String, String>> },
    Unregister { name: String, ok: bool, stored: String },
    AdminSet { name: String, register: bool, hash: Result<String, String> },
}

/// Runs `job`: on a worker thread in the game, inline in tests.
pub fn run(job: Job) -> Done {
    let cost = job.cost;
    let kind = match job.kind {
        JobKind::Login { name, password, stored } => {
            let v = hash::verify(&password, &stored, cost);
            let rehash = (v.ok && v.needs_rehash)
                .then(|| hash::hash(&password, cost).ok())
                .flatten();
            DoneKind::Login { name, ok: v.ok, stored, rehash }
        }
        JobKind::Register { name, password } => DoneKind::Register {
            name,
            hash: hash::hash(&password, cost),
        },
        JobKind::ChangePassword { name, old, new, stored } => {
            let ok = hash::verify(&old, &stored, cost).ok;
            let hash = ok.then(|| hash::hash(&new, cost));
            DoneKind::ChangePassword { name, ok, stored, hash }
        }
        JobKind::Unregister { name, password, stored } => {
            let ok = hash::verify(&password, &stored, cost).ok;
            DoneKind::Unregister { name, ok, stored }
        }
        JobKind::AdminSet { name, password, register } => DoneKind::AdminSet {
            name,
            register,
            hash: hash::hash(&password, cost),
        },
    };
    Done { actor: job.actor, kind }
}

#[derive(Debug)]
struct Session {
    name: String,
    authed: bool,
    /// When they joined, or logged out (ms, the tick clock).
    since: u64,
    last_reminder: Option<u64>,
    attempts: u32,
    /// A password job of theirs is running.
    busy: bool,
    /// Spectator, blindness and the anchor are in place.
    limbo_applied: bool,
    anchor: Option<(String, [f64; 3])>,
    kicked: bool,
    /// The last tick (ms) the player was on the server; `None` until the first tick after
    /// they joined.
    last_seen: Option<u64>,
}

pub struct Auth {
    dir: PathBuf,
    pub config: Config,
    lang: Lang,
    accounts: Accounts,
    limbo: Limbo,
    /// The accounts file could not be read: nothing is written over it, nobody logs in.
    broken: Option<String>,
    sessions: HashMap<u128, Session>,
    queue: VecDeque<Job>,
    in_flight: usize,
    /// Effects that come from hooks with no way to answer (chat), for the next tick.
    deferred: Vec<Effect>,
    /// Network ids of the players not logged in: their damage is refused both ways.
    protected: HashSet<i32>,
    now: u64,
    /// The IP address each player connected from, as the network told the gate.
    addresses: HashMap<u128, String>,
}

const COMMANDS_LOGIN: [&str; 3] = ["login", "l", "log"];
const COMMANDS_REGISTER: [&str; 2] = ["register", "reg"];
const COMMANDS_LOGOUT: [&str; 1] = ["logout"];
const COMMANDS_CHANGE: [&str; 3] = ["changepassword", "changepass", "cp"];
const COMMANDS_UNREGISTER: [&str; 2] = ["unregister", "unreg"];

/// A name that may go into a server command: letters, digits and `_`, 1 to 16 long.
pub fn safe_name(name: &str) -> bool {
    (1..=16).contains(&name.len()) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn safe_dimension(d: &str) -> bool {
    !d.is_empty()
        && d.len() <= 128
        && d.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "_:./-".contains(c))
}

fn mode_name(mode: i32) -> &'static str {
    match mode {
        1 => "creative",
        2 => "adventure",
        3 => "spectator",
        _ => "survival",
    }
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Auth {
    /// Loads `dir/config.toml` (writing the defaults when missing), `accounts.json` and
    /// `limbo.json`.
    pub fn open(dir: &Path) -> (Self, Vec<Effect>) {
        let mut auth = Auth {
            dir: dir.to_owned(),
            config: Config::default(),
            lang: Lang::new("en"),
            accounts: Accounts::default(),
            limbo: Limbo::default(),
            broken: None,
            sessions: HashMap::new(),
            addresses: HashMap::new(),
            queue: VecDeque::new(),
            in_flight: 0,
            deferred: Vec::new(),
            protected: HashSet::new(),
            now: 0,
        };
        let effects = auth.reload();
        (auth, effects)
    }

    /// Reads the files again (`/authme reload`). Sessions stay as they are.
    pub fn reload(&mut self) -> Vec<Effect> {
        let mut out = Vec::new();
        let config_path = self.dir.join("config.toml");
        match std::fs::read_to_string(&config_path) {
            Ok(text) => match Config::parse(&text) {
                Ok(c) => self.config = c,
                Err(e) => out.push(Effect::Console(format!(
                    "[AuthMe] {}: {e}; keeping the previous settings",
                    config_path.display()
                ))),
            },
            Err(_) => {
                let _ = std::fs::create_dir_all(&self.dir);
                if let Err(e) = std::fs::write(&config_path, self.config.to_toml()) {
                    out.push(Effect::Console(format!("[AuthMe] {}: {e}", config_path.display())));
                }
            }
        }
        self.lang = Lang::new(&self.config.language);
        match store::load::<Accounts>(&self.dir.join("accounts.json")) {
            Ok(a) => {
                self.accounts = a;
                self.broken = None;
            }
            Err(e) => {
                out.push(Effect::Console(format!(
                    "[AuthMe] CANNOT READ ACCOUNTS ({e}). Nobody can log in until it is fixed and /authme reload is run."
                )));
                self.broken = Some(e);
            }
        }
        match store::load::<Limbo>(&self.dir.join("limbo.json")) {
            Ok(l) => self.limbo = l,
            Err(e) => out.push(Effect::Console(format!("[AuthMe] {e}"))),
        }
        out.push(Effect::Console(format!(
            "[AuthMe] AuthMe for Horizon {VERSION} (port of AuthMeReloaded, GPL-3.0): {} accounts",
            self.accounts.accounts.len()
        )));
        out
    }

    fn t(&self, msg: Msg) -> String {
        self.lang.text(msg, &[])
    }

    fn ta(&self, msg: Msg, args: &[(&str, &str)]) -> String {
        self.lang.text(msg, args)
    }

    fn uk(&self) -> bool {
        self.lang.0 == super::messages::Language::Uk
    }

    fn save_accounts(&mut self, out: &mut Vec<Effect>) -> bool {
        if self.broken.is_some() {
            out.push(Effect::Console("[AuthMe] accounts not saved: the file could not be read".into()));
            return false;
        }
        match store::save(&self.dir.join("accounts.json"), &self.accounts) {
            Ok(()) => true,
            Err(e) => {
                out.push(Effect::Console(format!("[AuthMe] {e}")));
                false
            }
        }
    }

    fn save_limbo(&mut self, out: &mut Vec<Effect>) {
        if let Err(e) = store::save(&self.dir.join("limbo.json"), &self.limbo) {
            out.push(Effect::Console(format!("[AuthMe] {e}")));
        }
    }

    /// Whether `uuid` may act. Unknown players may not.
    pub fn is_authed(&self, uuid: u128) -> bool {
        self.sessions.get(&uuid).is_some_and(|s| s.authed)
    }

    /// The players logged in now (for the chat and drop gate).
    pub fn authed_uuids(&self) -> Vec<u128> {
        self.sessions.iter().filter(|(_, s)| s.authed).map(|(u, _)| *u).collect()
    }

    /// Whether the entity with this network id is a player not yet logged in.
    pub fn is_protected(&self, entity: i32) -> bool {
        entity != 0 && self.protected.contains(&entity)
    }

    fn registered(&self, name: &str) -> bool {
        self.accounts.get(name).is_some()
    }

    // ---- joining ---------------------------------------------------------------------------

    /// Before a player is placed: `Err(reason)` refuses them.
    pub fn check_login(&self, name: &str) -> Result<(), String> {
        let c = &self.config;
        if !name.chars().all(|ch| c.allowed_nickname_characters.contains(ch)) || !safe_name(name) {
            return Err(self.ta(Msg::NameCharacters, &[("valid", "a-z A-Z 0-9 _")]));
        }
        let len = name.chars().count();
        if len < c.min_nickname_length || len > c.max_nickname_length {
            let (min, max) = (c.min_nickname_length.to_string(), c.max_nickname_length.to_string());
            return Err(self.ta(Msg::NameLength, &[("min", &min), ("max", &max)]));
        }
        if c.prevent_other_case
            && let Some(a) = self.accounts.get(name)
            && a.realname != name
        {
            return Err(self.ta(Msg::NameCase, &[("valid", &a.realname)]));
        }
        Ok(())
    }

    pub fn joined(&mut self, uuid: u128, name: &str) -> Vec<Effect> {
        // The tick and the join hook can both see a new player first: one greeting.
        if self.sessions.get(&uuid).is_some_and(|s| s.name == name) {
            return Vec::new();
        }
        let registered = self.registered(name);
        let authed = !registered && !self.config.force_registration;
        self.sessions.insert(
            uuid,
            Session {
                name: name.to_owned(),
                authed,
                since: self.now,
                last_reminder: Some(self.now),
                attempts: 0,
                busy: false,
                limbo_applied: false,
                anchor: None,
                kicked: false,
                last_seen: None,
            },
        );
        let mut out = Vec::new();
        if authed {
            // Playing unregistered: a limbo left from an earlier visit is undone.
            self.release(uuid, name, &mut out);
            return out;
        }
        if registered && self.resume_session(uuid, name, &mut out) {
            return out;
        }
        let msg = if registered { Msg::LoginRequest } else { Msg::RegisterRequest };
        out.push(Effect::Tell(uuid, self.t(msg), Tone::Info));
        out
    }

    /// A player the server accepted, before they are placed: a session left from an earlier
    /// attempt (a refused join, a kick whose leave was never heard) is dropped, so its timers
    /// and attempts never carry over.
    pub fn connecting(&mut self, uuid: u128) {
        self.sessions.remove(&uuid);
    }

    /// The IP address `uuid` connects from (the network's, before the login hooks).
    pub fn set_address(&mut self, uuid: u128, ip: String) {
        if self.addresses.len() > 4096 {
            self.addresses.clear();
        }
        self.addresses.insert(uuid, ip);
    }

    /// Logs `uuid` in without a password when the account's last password login came from
    /// the address they connect from now, with the name in the same letter case, less than
    /// `session_minutes` ago. The session then runs on from now.
    fn resume_session(&mut self, uuid: u128, name: &str, out: &mut Vec<Effect>) -> bool {
        let minutes = self.config.session_minutes;
        if minutes == 0 || self.broken.is_some() {
            return false;
        }
        let Some(ip) = self.addresses.get(&uuid).cloned() else {
            return false;
        };
        // Through a tunnel, relay or proxy every player connects from the same loopback
        // address: it says nothing about who they are, so it never resumes a session.
        if !session_address(&ip) {
            return false;
        }
        let now = now_secs();
        let Some(a) = self.accounts.get_mut(name) else {
            return false;
        };
        let fresh = a
            .last_login
            .is_some_and(|t| now.saturating_sub(t) < u64::from(minutes) * 60);
        if a.realname != name || a.last_ip.as_deref() != Some(ip.as_str()) || !fresh {
            return false;
        }
        a.last_login = Some(now);
        self.save_accounts(out);
        self.authenticate(uuid, out);
        out.push(Effect::Tell(uuid, self.t(Msg::SessionResumed), Tone::Good));
        out.push(Effect::Console(format!("[AuthMe] {name} logged in by session from {ip}")));
        true
    }

    pub fn left(&mut self, uuid: u128) {
        // A limbo record stays: the player's file was saved as a spectator, and the mode comes
        // back when they next log in.
        self.sessions.remove(&uuid);
    }

    /// Undoes the limbo of a player who just logged in.
    fn release(&mut self, uuid: u128, name: &str, out: &mut Vec<Effect>) {
        if !safe_name(name) {
            return;
        }
        if let Some(mode) = self.limbo.game_modes.remove(&uuid_key(uuid)) {
            out.push(Effect::Command(format!("gamemode {} {name}", mode_name(mode))));
            self.save_limbo(out);
        }
        if self.config.apply_blindness {
            out.push(Effect::Command(format!("effect clear {name} minecraft:blindness")));
        }
    }

    fn authenticate(&mut self, uuid: u128, out: &mut Vec<Effect>) {
        let Some(s) = self.sessions.get_mut(&uuid) else {
            return;
        };
        s.authed = true;
        s.attempts = 0;
        s.limbo_applied = false;
        s.anchor = None;
        let name = s.name.clone();
        self.release(uuid, &name, out);
    }

    fn unauthenticate(&mut self, uuid: u128) {
        if let Some(s) = self.sessions.get_mut(&uuid) {
            s.authed = false;
            s.since = self.now;
            s.last_reminder = Some(self.now);
            s.attempts = 0;
            s.limbo_applied = false;
            s.anchor = None;
        }
    }

    // ---- what players type -----------------------------------------------------------------

    /// A command a player typed, without its slash. `None`: not the mod's, and allowed (the game
    /// runs it). `Some(effects)`: the mod took it; the game must not run or log it.
    pub fn player_command(&mut self, uuid: u128, name: &str, line: &str) -> Option<Vec<Effect>> {
        let mut words = line.split_whitespace();
        let head = words.next().unwrap_or("").to_ascii_lowercase();
        // `minecraft:tp` is `tp` for the allow list.
        let bare = head.rsplit(':').next().unwrap_or("").to_owned();
        let args: Vec<&str> = words.collect();
        if !self.sessions.contains_key(&uuid) {
            // Joined before the mod could see it: treat as a fresh join.
            let mut out = self.joined(uuid, name);
            out.retain(|e| matches!(e, Effect::Command(_)));
            self.deferred.extend(out);
        }
        let ours = |list: &[&str]| list.contains(&bare.as_str());
        let out = if ours(&COMMANDS_LOGIN) {
            self.login(uuid, name, &args)
        } else if ours(&COMMANDS_REGISTER) {
            self.register(uuid, name, &args)
        } else if ours(&COMMANDS_LOGOUT) {
            self.logout(uuid)
        } else if ours(&COMMANDS_CHANGE) {
            self.change_password(uuid, name, &args)
        } else if ours(&COMMANDS_UNREGISTER) {
            self.unregister(uuid, name, &args)
        } else if self.is_authed(uuid) || self.config.allow_commands.contains(&bare) {
            return None;
        } else {
            vec![Effect::Tell(uuid, self.t(Msg::DeniedCommand), Tone::Bad)]
        };
        Some(out)
    }

    /// A chat message: false refuses it (the player is told at the next tick).
    pub fn chat(&mut self, uuid: u128) -> bool {
        if self.is_authed(uuid) {
            return true;
        }
        self.deferred.push(Effect::Tell(uuid, self.t(Msg::DeniedChat), Tone::Bad));
        false
    }

    fn check_password(&self, name: &str, password: &str) -> Result<(), Msg> {
        let c = &self.config;
        let len = password.chars().count();
        if len < c.min_password_length || len > c.max_password_length {
            return Err(Msg::PasswordLength);
        }
        if password.chars().any(|ch| ch.is_whitespace() || ch.is_control()) {
            return Err(Msg::PasswordCharacters);
        }
        if password.eq_ignore_ascii_case(name) || password.to_lowercase().contains(&name.to_lowercase()) {
            return Err(Msg::PasswordName);
        }
        let lower = password.to_lowercase();
        if c.unsafe_passwords.iter().any(|u| u.to_lowercase() == lower) {
            return Err(Msg::PasswordUnsafe);
        }
        Ok(())
    }

    fn password_error(&self, uuid: u128, msg: Msg) -> Effect {
        let (min, max) = (
            self.config.min_password_length.to_string(),
            self.config.max_password_length.to_string(),
        );
        Effect::Tell(uuid, self.ta(msg, &[("min", &min), ("max", &max)]), Tone::Bad)
    }

    fn start(&mut self, actor: Option<u128>, kind: JobKind) {
        if let Some(uuid) = actor
            && let Some(s) = self.sessions.get_mut(&uuid)
        {
            s.busy = true;
        }
        let cost = self.config.cost();
        self.queue.push_back(Job { actor, kind, cost });
    }

    fn busy(&self, uuid: u128) -> bool {
        self.sessions.get(&uuid).is_some_and(|s| s.busy)
    }

    fn login(&mut self, uuid: u128, name: &str, args: &[&str]) -> Vec<Effect> {
        if self.is_authed(uuid) {
            return vec![Effect::Tell(uuid, self.t(Msg::AlreadyLoggedIn), Tone::Bad)];
        }
        if self.busy(uuid) {
            return vec![Effect::Tell(uuid, self.t(Msg::Busy), Tone::Info)];
        }
        if self.broken.is_some() {
            return vec![Effect::Tell(uuid, self.t(Msg::Error), Tone::Bad)];
        }
        let Some(account) = self.accounts.get(name) else {
            return vec![
                Effect::Tell(uuid, self.t(Msg::NotRegistered), Tone::Bad),
                Effect::Tell(uuid, self.t(Msg::RegisterRequest), Tone::Info),
            ];
        };
        let [password] = args else {
            return vec![Effect::Tell(uuid, self.t(Msg::LoginUsage), Tone::Bad)];
        };
        let stored = account.hash.clone();
        self.start(
            Some(uuid),
            JobKind::Login { name: name.to_owned(), password: (*password).to_owned(), stored },
        );
        Vec::new()
    }

    fn register(&mut self, uuid: u128, name: &str, args: &[&str]) -> Vec<Effect> {
        if self.registered(name) {
            let msg = if self.is_authed(uuid) { Msg::AlreadyLoggedIn } else { Msg::NameTaken };
            return vec![Effect::Tell(uuid, self.t(msg), Tone::Bad)];
        }
        if self.busy(uuid) {
            return vec![Effect::Tell(uuid, self.t(Msg::Busy), Tone::Info)];
        }
        if self.broken.is_some() {
            return vec![Effect::Tell(uuid, self.t(Msg::Error), Tone::Bad)];
        }
        let [password, confirm] = args else {
            return vec![Effect::Tell(uuid, self.t(Msg::RegisterUsage), Tone::Bad)];
        };
        if password != confirm {
            return vec![Effect::Tell(uuid, self.t(Msg::PasswordMismatch), Tone::Bad)];
        }
        if let Err(msg) = self.check_password(name, password) {
            return vec![self.password_error(uuid, msg)];
        }
        self.start(
            Some(uuid),
            JobKind::Register { name: name.to_owned(), password: (*password).to_owned() },
        );
        Vec::new()
    }

    fn logout(&mut self, uuid: u128) -> Vec<Effect> {
        let Some(s) = self.sessions.get(&uuid) else {
            return Vec::new();
        };
        if !s.authed {
            return vec![Effect::Tell(uuid, self.t(Msg::NotLoggedIn), Tone::Bad)];
        }
        if !self.registered(&s.name.clone()) {
            return vec![Effect::Tell(uuid, self.t(Msg::NotRegistered), Tone::Bad)];
        }
        self.unauthenticate(uuid);
        let mut out = Vec::new();
        let name = self.sessions.get(&uuid).map(|s| s.name.clone()).unwrap_or_default();
        if let Some(a) = self.accounts.get_mut(&name)
            && a.last_ip.take().is_some()
        {
            self.save_accounts(&mut out);
        }
        out.extend([
            Effect::Tell(uuid, self.t(Msg::LogoutSuccess), Tone::Good),
            Effect::Tell(uuid, self.t(Msg::LoginRequest), Tone::Info),
        ]);
        out
    }

    fn change_password(&mut self, uuid: u128, name: &str, args: &[&str]) -> Vec<Effect> {
        if !self.is_authed(uuid) {
            return vec![Effect::Tell(uuid, self.t(Msg::NotLoggedIn), Tone::Bad)];
        }
        if self.busy(uuid) {
            return vec![Effect::Tell(uuid, self.t(Msg::Busy), Tone::Info)];
        }
        let Some(account) = self.accounts.get(name) else {
            return vec![Effect::Tell(uuid, self.t(Msg::NotRegistered), Tone::Bad)];
        };
        let [old, new] = args else {
            return vec![Effect::Tell(uuid, self.t(Msg::ChangePasswordUsage), Tone::Bad)];
        };
        let stored = account.hash.clone();
        if let Err(msg) = self.check_password(name, new) {
            return vec![self.password_error(uuid, msg)];
        }
        self.start(
            Some(uuid),
            JobKind::ChangePassword {
                name: name.to_owned(),
                old: (*old).to_owned(),
                new: (*new).to_owned(),
                stored,
            },
        );
        Vec::new()
    }

    fn unregister(&mut self, uuid: u128, name: &str, args: &[&str]) -> Vec<Effect> {
        if !self.is_authed(uuid) {
            return vec![Effect::Tell(uuid, self.t(Msg::NotLoggedIn), Tone::Bad)];
        }
        if self.busy(uuid) {
            return vec![Effect::Tell(uuid, self.t(Msg::Busy), Tone::Info)];
        }
        let Some(account) = self.accounts.get(name) else {
            return vec![Effect::Tell(uuid, self.t(Msg::NotRegistered), Tone::Bad)];
        };
        let [password] = args else {
            return vec![Effect::Tell(uuid, self.t(Msg::UnregisterUsage), Tone::Bad)];
        };
        let stored = account.hash.clone();
        self.start(
            Some(uuid),
            JobKind::Unregister { name: name.to_owned(), password: (*password).to_owned(), stored },
        );
        Vec::new()
    }

    // ---- password work -----------------------------------------------------------------

    /// The jobs to start now, within `max_parallel_hashes`.
    pub fn take_jobs(&mut self) -> Vec<Job> {
        let mut jobs = Vec::new();
        while self.in_flight < self.config.max_parallel_hashes {
            let Some(job) = self.queue.pop_front() else {
                break;
            };
            self.in_flight += 1;
            jobs.push(job);
        }
        jobs
    }

    fn reply(&self, actor: Option<u128>, line: String, tone: Tone) -> Effect {
        match actor {
            Some(uuid) => Effect::Tell(uuid, line, tone),
            None => Effect::Console(format!("[AuthMe] {line}")),
        }
    }

    fn online_uuid(&self, name: &str) -> Option<u128> {
        self.sessions
            .iter()
            .find(|(_, s)| s.name.eq_ignore_ascii_case(name))
            .map(|(u, _)| *u)
    }

    pub fn complete(&mut self, done: Done) -> Vec<Effect> {
        self.in_flight = self.in_flight.saturating_sub(1);
        let actor = done.actor;
        if let Some(uuid) = actor
            && let Some(s) = self.sessions.get_mut(&uuid)
        {
            s.busy = false;
        }
        let mut out = Vec::new();
        match done.kind {
            DoneKind::Login { name, ok, stored, rehash } => {
                let Some(uuid) = actor else { return out };
                if !self.sessions.contains_key(&uuid) {
                    return out;
                }
                // The account may have changed while the hash ran.
                let current = self.accounts.get(&name).map(|a| a.hash.clone());
                if ok && current.as_deref() == Some(stored.as_str()) {
                    let now = now_secs();
                    let ip = self.addresses.get(&uuid).cloned();
                    if let Some(a) = self.accounts.get_mut(&name) {
                        a.last_login = Some(now);
                        a.last_ip = ip;
                        if let Some(h) = rehash {
                            a.hash = h;
                        }
                    }
                    self.save_accounts(&mut out);
                    self.authenticate(uuid, &mut out);
                    out.push(Effect::Tell(uuid, self.t(Msg::LoginSuccess), Tone::Good));
                    out.push(Effect::Console(format!("[AuthMe] {name} logged in")));
                } else {
                    let max = self.config.max_login_attempts;
                    let s = self.sessions.get_mut(&uuid).expect("checked");
                    s.attempts += 1;
                    let attempts = s.attempts;
                    out.push(Effect::Console(format!("[AuthMe] {name} used a wrong password")));
                    if max > 0 && attempts >= max {
                        let reason = self.t(Msg::TooManyAttempts);
                        out.extend(self.kick(uuid, &reason));
                    } else if max > 0 {
                        let left = (max - attempts).to_string();
                        out.push(Effect::Tell(
                            uuid,
                            self.ta(Msg::WrongPasswordAttempts, &[("left", &left)]),
                            Tone::Bad,
                        ));
                    } else {
                        out.push(Effect::Tell(uuid, self.t(Msg::WrongPassword), Tone::Bad));
                    }
                }
            }
            DoneKind::Register { name, hash } => {
                let hash = match hash {
                    Ok(h) => h,
                    Err(e) => {
                        out.push(Effect::Console(format!("[AuthMe] {e}")));
                        out.push(self.reply(actor, self.t(Msg::Error), Tone::Bad));
                        return out;
                    }
                };
                if self.registered(&name) {
                    out.push(self.reply(actor, self.t(Msg::NameTaken), Tone::Bad));
                    return out;
                }
                self.accounts.insert(Account {
                    realname: name.clone(),
                    hash,
                    registered: now_secs(),
                    last_login: Some(now_secs()),
                    email: None,
                    last_ip: actor.and_then(|u| self.addresses.get(&u).cloned()),
                });
                if !self.save_accounts(&mut out) {
                    self.accounts.remove(&name);
                    out.push(self.reply(actor, self.t(Msg::Error), Tone::Bad));
                    return out;
                }
                if let Some(uuid) = actor {
                    self.authenticate(uuid, &mut out);
                    out.push(Effect::Tell(uuid, self.t(Msg::RegisterSuccess), Tone::Good));
                }
                out.push(Effect::Console(format!("[AuthMe] {name} registered")));
                if self.config.broadcast_registration {
                    out.push(Effect::Broadcast(self.ta(Msg::PlayerRegistered, &[("name", &name)])));
                }
            }
            DoneKind::ChangePassword { name, ok, stored, hash } => {
                let current = self.accounts.get(&name).map(|a| a.hash.clone());
                match hash {
                    Some(Ok(h)) if ok && current.as_deref() == Some(stored.as_str()) => {
                        if let Some(a) = self.accounts.get_mut(&name) {
                            a.hash = h;
                        }
                        if self.save_accounts(&mut out) {
                            out.push(self.reply(actor, self.t(Msg::PasswordChanged), Tone::Good));
                            out.push(Effect::Console(format!("[AuthMe] {name} changed their password")));
                        } else {
                            out.push(self.reply(actor, self.t(Msg::Error), Tone::Bad));
                        }
                    }
                    Some(Err(e)) => {
                        out.push(Effect::Console(format!("[AuthMe] {e}")));
                        out.push(self.reply(actor, self.t(Msg::Error), Tone::Bad));
                    }
                    _ => out.push(self.reply(actor, self.t(Msg::WrongPassword), Tone::Bad)),
                }
            }
            DoneKind::Unregister { name, ok, stored } => {
                let current = self.accounts.get(&name).map(|a| a.hash.clone());
                if ok && current.as_deref() == Some(stored.as_str()) {
                    let removed = self.accounts.remove(&name);
                    if self.save_accounts(&mut out) {
                        out.push(self.reply(actor, self.t(Msg::Unregistered), Tone::Good));
                        out.push(Effect::Console(format!("[AuthMe] {name} unregistered")));
                        if let Some(uuid) = actor
                            && self.config.force_registration
                        {
                            self.unauthenticate(uuid);
                            out.push(Effect::Tell(uuid, self.t(Msg::RegisterRequest), Tone::Info));
                        }
                    } else if let Some(a) = removed {
                        self.accounts.insert(a);
                        out.push(self.reply(actor, self.t(Msg::Error), Tone::Bad));
                    }
                } else {
                    out.push(self.reply(actor, self.t(Msg::WrongPassword), Tone::Bad));
                }
            }
            DoneKind::AdminSet { name, register, hash } => {
                let hash = match hash {
                    Ok(h) => h,
                    Err(e) => {
                        out.push(Effect::Console(format!("[AuthMe] {e}")));
                        out.push(self.reply(actor, self.t(Msg::Error), Tone::Bad));
                        return out;
                    }
                };
                let uk = self.uk();
                if register {
                    if self.registered(&name) {
                        out.push(self.reply(actor, self.t(Msg::NameTaken), Tone::Bad));
                        return out;
                    }
                    self.accounts.insert(Account {
                        realname: name.clone(),
                        hash,
                        registered: now_secs(),
                        last_login: None,
                        email: None,
                        last_ip: None,
                    });
                } else {
                    let Some(a) = self.accounts.get_mut(&name) else {
                        out.push(self.reply(actor, self.t(Msg::NotRegistered), Tone::Bad));
                        return out;
                    };
                    a.hash = hash;
                    // A new password from an admin ends the player's session.
                    a.last_ip = None;
                }
                if !self.save_accounts(&mut out) {
                    out.push(self.reply(actor, self.t(Msg::Error), Tone::Bad));
                    return out;
                }
                let line = match (register, uk) {
                    (true, true) => format!("Гравця {name} зареєстровано."),
                    (true, false) => format!("Registered {name}."),
                    (false, true) => format!("Пароль гравця {name} змінено."),
                    (false, false) => format!("Changed {name}'s password."),
                };
                out.push(self.reply(actor, line, Tone::Good));
                out.push(Effect::Console(format!(
                    "[AuthMe] {} {name}'s account",
                    if register { "an admin registered" } else { "an admin changed the password of" }
                )));
                // Someone playing unregistered under that name must now log in (AuthMe does
                // the same on an admin registration).
                if register && let Some(uuid) = self.online_uuid(&name) {
                    self.unauthenticate(uuid);
                    out.push(Effect::Tell(uuid, self.t(Msg::LoggedOutByAdmin), Tone::Info));
                }
            }
        }
        out
    }

    fn kick(&mut self, uuid: u128, reason: &str) -> Vec<Effect> {
        let Some(s) = self.sessions.get_mut(&uuid) else {
            return Vec::new();
        };
        if s.kicked || !safe_name(&s.name) {
            return Vec::new();
        }
        s.kicked = true;
        let reason: String = reason.chars().filter(|c| !c.is_control()).collect();
        vec![Effect::Command(format!("kick {} {reason}", s.name))]
    }

    // ---- every tick ------------------------------------------------------------------------

    /// Once a server tick, with the clock in ms and the players on the server.
    pub fn tick(&mut self, now_ms: u64, players: &[Online]) -> Vec<Effect> {
        self.now = now_ms;
        let mut out = std::mem::take(&mut self.deferred);
        self.protected.clear();
        let c = self.config.clone();
        for p in players {
            if !self.sessions.contains_key(&p.uuid) {
                let joined = self.joined(p.uuid, &p.name);
                out.extend(joined);
            }
            let registered = self.registered(&p.name);
            let now = self.now;
            let s = self.sessions.get_mut(&p.uuid).expect("inserted");
            match s.last_seen {
                // The first tick of the visit: the timers start now (the clock stands still
                // while an empty server is paused, so the join hook's time can be old).
                None => {
                    s.since = now;
                    s.last_reminder = Some(now);
                }
                // Away for more than a second (a leave the mod never heard): a new visit, with
                // fresh timers. Logged-in players stay logged in only while they stay on.
                Some(t) if now.saturating_sub(t) > 1000 => {
                    s.since = now;
                    s.last_reminder = Some(now);
                    s.attempts = 0;
                    s.kicked = false;
                    s.limbo_applied = false;
                    s.anchor = None;
                    if s.authed && registered {
                        s.authed = false;
                    }
                    s.last_seen = Some(now);
                    if registered {
                        let mut resumed = Vec::new();
                        if self.resume_session(p.uuid, &p.name, &mut resumed) {
                            out.extend(resumed);
                            continue;
                        }
                    }
                }
                Some(_) => {}
            }
            let s = self.sessions.get_mut(&p.uuid).expect("inserted");
            s.last_seen = Some(now);
            if s.authed {
                continue;
            }
            self.protected.insert(p.entity_id);
            if !safe_name(&p.name) {
                continue;
            }
            let name = &p.name;
            if !s.limbo_applied {
                s.limbo_applied = true;
                s.anchor = Some((p.dimension.clone(), p.position));
                if c.limbo_spectator {
                    let key = uuid_key(p.uuid);
                    if !self.limbo.game_modes.contains_key(&key) {
                        self.limbo.game_modes.insert(key, p.game_mode);
                        if let Err(e) = store::save(&self.dir.join("limbo.json"), &self.limbo) {
                            out.push(Effect::Console(format!("[AuthMe] {e}")));
                        }
                    }
                    if p.game_mode != 3 {
                        out.push(Effect::Command(format!("gamemode spectator {name}")));
                    }
                }
                if c.apply_blindness {
                    out.push(Effect::Command(format!(
                        "effect give {name} minecraft:blindness infinite 0 true"
                    )));
                }
                continue;
            }
            if c.limbo_spectator && p.game_mode != 3 {
                out.push(Effect::Command(format!("gamemode spectator {name}")));
            }
            if c.freeze
                && let Some((dim, [x, y, z])) = &s.anchor
            {
                let [px, py, pz] = p.position;
                let far = (px - x).powi(2) + (py - y).powi(2) + (pz - z).powi(2)
                    > c.allowed_movement_radius.powi(2);
                if (far || *dim != p.dimension) && safe_dimension(dim) {
                    out.push(Effect::Command(format!(
                        "execute in {dim} run tp {name} {x:.3} {y:.3} {z:.3}"
                    )));
                }
            }
            let msg = if registered { Msg::LoginRequest } else { Msg::RegisterRequest };
            if c.message_interval_secs > 0
                && s.last_reminder.is_none_or(|t| now_ms.saturating_sub(t) >= u64::from(c.message_interval_secs) * 1000)
            {
                s.last_reminder = Some(now_ms);
                let line = self.lang.text(msg, &[]);
                out.push(Effect::Tell(p.uuid, line.clone(), Tone::Info));
                out.push(Effect::ActionBar(p.uuid, line));
            }
            let s = self.sessions.get(&p.uuid).expect("inserted");
            if c.login_timeout_secs > 0
                && !s.kicked
                && now_ms.saturating_sub(s.since) >= u64::from(c.login_timeout_secs) * 1000
            {
                let reason = self.t(Msg::Timeout);
                out.extend(self.kick(p.uuid, &reason));
            }
        }
        out
    }

    // ---- /authme ---------------------------------------------------------------------------

    /// `/authme ...` from an operator (`actor`) or the console (`None`), `args` after `authme`.
    pub fn admin(&mut self, actor: Option<u128>, args: &[&str]) -> Vec<Effect> {
        let uk = self.uk();
        let say = |s: &Self, ukr: String, en: String, tone: Tone| s.reply(actor, if uk { ukr } else { en }, tone);
        let sub = args.first().map(|s| s.to_ascii_lowercase()).unwrap_or_default();
        match (sub.as_str(), &args[1.min(args.len())..]) {
            ("register" | "reg", [name, password]) => {
                if !safe_name(name) {
                    return vec![say(self, "Недопустимий нікнейм.".into(), "Invalid name.".into(), Tone::Bad)];
                }
                if self.registered(name) {
                    return vec![self.reply(actor, self.t(Msg::NameTaken), Tone::Bad)];
                }
                if let Err(msg) = self.check_password(name, password) {
                    let (min, max) = (
                        self.config.min_password_length.to_string(),
                        self.config.max_password_length.to_string(),
                    );
                    return vec![self.reply(actor, self.ta(msg, &[("min", &min), ("max", &max)]), Tone::Bad)];
                }
                self.start(actor, JobKind::AdminSet {
                    name: (*name).to_owned(),
                    password: (*password).to_owned(),
                    register: true,
                });
                Vec::new()
            }
            ("password" | "changepassword" | "cp", [name, password]) => {
                if !self.registered(name) {
                    return vec![self.reply(actor, self.t(Msg::NotRegistered), Tone::Bad)];
                }
                if let Err(msg) = self.check_password(name, password) {
                    let (min, max) = (
                        self.config.min_password_length.to_string(),
                        self.config.max_password_length.to_string(),
                    );
                    return vec![self.reply(actor, self.ta(msg, &[("min", &min), ("max", &max)]), Tone::Bad)];
                }
                self.start(actor, JobKind::AdminSet {
                    name: (*name).to_owned(),
                    password: (*password).to_owned(),
                    register: false,
                });
                Vec::new()
            }
            ("unregister" | "unreg", [name]) => {
                if self.accounts.remove(name).is_none() {
                    return vec![self.reply(actor, self.t(Msg::NotRegistered), Tone::Bad)];
                }
                let mut out = Vec::new();
                self.save_accounts(&mut out);
                out.push(say(self, format!("Гравця {name} видалено."), format!("Unregistered {name}."), Tone::Good));
                out.push(Effect::Console(format!("[AuthMe] an admin unregistered {name}")));
                if let Some(uuid) = self.online_uuid(name)
                    && self.config.force_registration
                {
                    self.unauthenticate(uuid);
                    out.push(Effect::Tell(uuid, self.t(Msg::RegisterRequest), Tone::Info));
                }
                out
            }
            ("forcelogin", [name]) => {
                let Some(uuid) = self.online_uuid(name) else {
                    return vec![say(self, format!("{name} не в грі."), format!("{name} is not online."), Tone::Bad)];
                };
                let mut out = Vec::new();
                if !self.is_authed(uuid) {
                    self.authenticate(uuid, &mut out);
                    out.push(Effect::Tell(uuid, self.t(Msg::LoginSuccess), Tone::Good));
                }
                out.push(say(self, format!("{name} авторизовано."), format!("{name} is logged in."), Tone::Good));
                out
            }
            ("lastlogin", [name]) => match self.accounts.get(name) {
                None => vec![self.reply(actor, self.t(Msg::NotRegistered), Tone::Bad)],
                Some(a) => {
                    let when = a.last_login.map(format_time).unwrap_or_else(|| "-".into());
                    vec![say(
                        self,
                        format!("{}: останній вхід {when}", a.realname),
                        format!("{}: last login {when}", a.realname),
                        Tone::Info,
                    )]
                }
            },
            ("list", []) => {
                let waiting: Vec<&str> = self
                    .sessions
                    .values()
                    .filter(|s| !s.authed)
                    .map(|s| s.name.as_str())
                    .collect();
                let n = self.accounts.accounts.len();
                let w = if waiting.is_empty() { "-".to_owned() } else { waiting.join(", ") };
                vec![say(
                    self,
                    format!("Акаунтів: {n}. Не авторизовані зараз: {w}"),
                    format!("Accounts: {n}. Not logged in now: {w}"),
                    Tone::Info,
                )]
            }
            ("reload", []) => {
                let mut out = self.reload();
                out.push(say(
                    self,
                    "Конфігурації та базу даних було успішно перезавантажено!".into(),
                    "Configuration and accounts reloaded.".into(),
                    Tone::Good,
                ));
                out
            }
            ("version", []) => vec![self.reply(
                actor,
                format!(
                    "AuthMe for Horizon {VERSION}: a port of AuthMeReloaded (github.com/AuthMe/AuthMeReloaded) \
                     by the AuthMe team, GPL-3.0. Passwords: scrypt."
                ),
                Tone::Info,
            )],
            _ => {
                let lines = if uk {
                    [
                        "/authme register <гравець> <пароль> — зареєструвати",
                        "/authme unregister <гравець> — видалити акаунт",
                        "/authme password <гравець> <пароль> — змінити пароль",
                        "/authme forcelogin <гравець> — авторизувати",
                        "/authme lastlogin <гравець> — останній вхід",
                        "/authme list | reload | version",
                    ]
                } else {
                    [
                        "/authme register <player> <password>",
                        "/authme unregister <player>",
                        "/authme password <player> <password>",
                        "/authme forcelogin <player>",
                        "/authme lastlogin <player>",
                        "/authme list | reload | version",
                    ]
                };
                lines.iter().map(|l| self.reply(actor, (*l).to_owned(), Tone::Info)).collect()
            }
        }
    }
}

/// `YYYY-MM-DD HH:MM UTC` from Unix seconds.
pub fn format_time(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil from days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", rem / 3600, rem % 3600 / 60)
}

/// Whether a login from `ip` may resume a session: not a loopback, unspecified or unparsable
/// address (everyone behind a tunnel or proxy shows as 127.0.0.1).
pub fn session_address(ip: &str) -> bool {
    match ip.parse::<std::net::IpAddr>() {
        Ok(addr) => !addr.is_loopback() && !addr.is_unspecified(),
        Err(_) => false,
    }
}
