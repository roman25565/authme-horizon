//! The mod's rules end to end, as the game glue drives them: joins, commands, password jobs
//! (run inline here, on a worker thread in the game) and ticks.

use authme_core_tests::core::auth::{self, Auth, Effect, Online};
use std::path::PathBuf;

const ROMAN: u128 = 0x1111;
const EVE: u128 = 0x2222;

struct World {
    auth: Auth,
    dir: PathBuf,
    now: u64,
}

impl World {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("authme-flow-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // A cheap scrypt cost so the tests run quickly; the default is 2^15.
        std::fs::write(dir.join("config.toml"), "scrypt_log_n = 14\nscrypt_p = 1\nlanguage = \"en\"\n").unwrap();
        let (auth, _) = Auth::open(&dir);
        World { auth, dir, now: 0 }
    }

    /// Runs every queued job, as the worker threads would, and returns what came of them.
    fn finish_jobs(&mut self) -> Vec<Effect> {
        let mut out = Vec::new();
        loop {
            let jobs = self.auth.take_jobs();
            if jobs.is_empty() {
                return out;
            }
            for j in jobs {
                out.extend(self.auth.complete(auth::run(j)));
            }
        }
    }

    fn cmd(&mut self, uuid: u128, name: &str, line: &str) -> Vec<Effect> {
        let mut out = self.auth.player_command(uuid, name, line).expect("the mod's command");
        out.extend(self.finish_jobs());
        out
    }

    fn tick(&mut self, players: &[Online]) -> Vec<Effect> {
        self.now += 50;
        self.auth.tick(self.now, players)
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn online(uuid: u128, name: &str, pos: [f64; 3], mode: i32) -> Online {
    Online {
        uuid,
        name: name.into(),
        dimension: "minecraft:overworld".into(),
        position: pos,
        game_mode: mode,
        entity_id: uuid as i32,
    }
}

fn told(effects: &[Effect], uuid: u128, needle: &str) -> bool {
    effects
        .iter()
        .any(|e| matches!(e, Effect::Tell(u, t, _) if *u == uuid && t.contains(needle)))
}

fn commands(effects: &[Effect]) -> Vec<String> {
    effects
        .iter()
        .filter_map(|e| match e {
            Effect::Command(c) => Some(c.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn register_then_login_with_limbo() {
    let mut w = World::new("register");
    let out = w.auth.joined(ROMAN, "Roman");
    assert!(told(&out, ROMAN, "/register"));
    assert!(!w.auth.is_authed(ROMAN));

    // First tick: spectator, blindness, anchored where they stand.
    let out = w.tick(&[online(ROMAN, "Roman", [10.0, 64.0, 10.0], 0)]);
    let c = commands(&out);
    assert!(c.contains(&"gamemode spectator Roman".to_string()), "{c:?}");
    assert!(c.iter().any(|c| c.starts_with("effect give Roman minecraft:blindness")));
    assert!(w.auth.is_protected(ROMAN as i32));

    // Walking away puts them back.
    let out = w.tick(&[online(ROMAN, "Roman", [14.0, 64.0, 10.0], 3)]);
    assert!(commands(&out).iter().any(|c| c.starts_with("execute in minecraft:overworld run tp Roman 10.000 64.000 10.000")));

    // Chat and other commands are refused.
    assert!(!w.auth.chat(ROMAN));
    let out = w.auth.player_command(ROMAN, "Roman", "tp Roman 0 100 0").unwrap();
    assert!(told(&out, ROMAN, "authenticated"));
    let out = w.auth.player_command(ROMAN, "Roman", "minecraft:give Roman diamond 64").unwrap();
    assert!(told(&out, ROMAN, "authenticated"));

    // Bad registrations.
    assert!(told(&w.cmd(ROMAN, "Roman", "register abc"), ROMAN, "Usage"));
    assert!(told(&w.cmd(ROMAN, "Roman", "register goodpass1 goodpass2"), ROMAN, "didn't match"));
    assert!(told(&w.cmd(ROMAN, "Roman", "register 123 123"), ROMAN, "characters long"));
    assert!(told(&w.cmd(ROMAN, "Roman", "register password password"), ROMAN, "isn't safe"));
    assert!(told(&w.cmd(ROMAN, "Roman", "register roman123 roman123"), ROMAN, "your name"));

    // A good one logs them in and gives their mode back.
    let out = w.cmd(ROMAN, "Roman", "register Kv1tka-2026 Kv1tka-2026");
    assert!(told(&out, ROMAN, "registered"), "{out:?}");
    assert!(w.auth.is_authed(ROMAN));
    let c = commands(&out);
    assert!(c.contains(&"gamemode survival Roman".to_string()), "{c:?}");
    assert!(c.contains(&"effect clear Roman minecraft:blindness".to_string()));
    assert!(w.auth.chat(ROMAN));
    assert!(w.auth.player_command(ROMAN, "Roman", "tp Roman 0 100 0").is_none());

    // The stored hash is scrypt, never the password.
    let text = std::fs::read_to_string(w.dir.join("accounts.json")).unwrap();
    assert!(text.contains("$scrypt$ln=14,r=8,p=1$"));
    assert!(!text.contains("Kv1tka-2026"));

    // Next visit: login.
    w.auth.left(ROMAN);
    let out = w.auth.joined(ROMAN, "Roman");
    assert!(told(&out, ROMAN, "/login"));
    w.tick(&[online(ROMAN, "Roman", [0.0, 64.0, 0.0], 0)]);
    assert!(told(&w.cmd(ROMAN, "Roman", "login wrong-one"), ROMAN, "Attempts left: 2"));
    let out = w.cmd(ROMAN, "Roman", "login Kv1tka-2026");
    assert!(told(&out, ROMAN, "Successful login"), "{out:?}");
    assert!(w.auth.is_authed(ROMAN));
}

#[test]
fn wrong_passwords_kick() {
    let mut w = World::new("kick");
    w.auth.joined(ROMAN, "Roman");
    w.cmd(ROMAN, "Roman", "register Kv1tka-2026 Kv1tka-2026");
    w.auth.left(ROMAN);
    w.auth.joined(ROMAN, "Roman");
    w.cmd(ROMAN, "Roman", "login a1b2c3d4");
    w.cmd(ROMAN, "Roman", "login a1b2c3d5");
    let out = w.cmd(ROMAN, "Roman", "login a1b2c3d6");
    assert!(commands(&out).iter().any(|c| c.starts_with("kick Roman ")), "{out:?}");
    assert!(!w.auth.is_authed(ROMAN));
}

#[test]
fn timeout_kicks_once() {
    let mut w = World::new("timeout");
    w.auth.joined(EVE, "Eve");
    let p = [online(EVE, "Eve", [0.0, 64.0, 0.0], 0)];
    let mut kicks = 0;
    for _ in 0..(181 * 20) {
        kicks += commands(&w.tick(&p)).iter().filter(|c| c.starts_with("kick Eve")).count();
    }
    assert_eq!(kicks, 1);
}

#[test]
fn name_rules_at_join() {
    let mut w = World::new("names");
    w.auth.joined(ROMAN, "Roman");
    w.cmd(ROMAN, "Roman", "register Kv1tka-2026 Kv1tka-2026");
    assert!(w.auth.check_login("Roman").is_ok());
    assert!(w.auth.check_login("roman").is_err(), "other letter case of a registered name");
    assert!(w.auth.check_login("ab").is_err(), "too short");
    assert!(w.auth.check_login("bad name").is_err());
    assert!(w.auth.check_login("evil;op").is_err());
    assert!(w.auth.check_login("Steve_2").is_ok());
}

#[test]
fn one_login_at_a_time_and_already_logged_in() {
    let mut w = World::new("busy");
    w.auth.joined(ROMAN, "Roman");
    w.cmd(ROMAN, "Roman", "register Kv1tka-2026 Kv1tka-2026");
    w.auth.left(ROMAN);
    w.auth.joined(ROMAN, "Roman");
    assert!(w.auth.player_command(ROMAN, "Roman", "login Kv1tka-2026").unwrap().is_empty());
    let out = w.auth.player_command(ROMAN, "Roman", "login Kv1tka-2026").unwrap();
    assert!(told(&out, ROMAN, "wait"));
    w.finish_jobs();
    assert!(told(&w.cmd(ROMAN, "Roman", "login Kv1tka-2026"), ROMAN, "already logged in"));
    assert!(told(&w.cmd(ROMAN, "Roman", "register x1x1x1x1 x1x1x1x1"), ROMAN, "already logged in"));
}

#[test]
fn change_password_logout_unregister() {
    let mut w = World::new("change");
    w.auth.joined(ROMAN, "Roman");
    w.cmd(ROMAN, "Roman", "register Kv1tka-2026 Kv1tka-2026");
    assert!(told(&w.cmd(ROMAN, "Roman", "changepassword nope New-pass-77"), ROMAN, "Wrong password"));
    assert!(told(&w.cmd(ROMAN, "Roman", "changepassword Kv1tka-2026 New-pass-77"), ROMAN, "changed"));
    assert!(told(&w.cmd(ROMAN, "Roman", "logout"), ROMAN, "Logged out"));
    assert!(!w.auth.is_authed(ROMAN));
    assert!(told(&w.cmd(ROMAN, "Roman", "login Kv1tka-2026"), ROMAN, "Attempts left"));
    assert!(told(&w.cmd(ROMAN, "Roman", "login New-pass-77"), ROMAN, "Successful"));
    assert!(told(&w.cmd(ROMAN, "Roman", "unregister New-pass-77"), ROMAN, "unregistered"));
    assert!(!w.auth.is_authed(ROMAN));
}

#[test]
fn admin_commands() {
    let mut w = World::new("admin");
    let out = w.auth.admin(None, &["register", "Eve", "Strong-pass-1"]);
    assert!(out.is_empty());
    let out = w.finish_jobs();
    assert!(out.iter().any(|e| matches!(e, Effect::Console(t) if t.contains("Registered Eve"))), "{out:?}");
    w.auth.joined(EVE, "Eve");
    assert!(!w.auth.is_authed(EVE));
    let out = w.auth.admin(None, &["forcelogin", "eve"]);
    assert!(told(&out, EVE, "Successful"));
    assert!(w.auth.is_authed(EVE));
    w.auth.admin(None, &["password", "Eve", "Other-pass-2"]);
    w.finish_jobs();
    w.auth.left(EVE);
    w.auth.joined(EVE, "Eve");
    assert!(told(&w.cmd(EVE, "Eve", "login Other-pass-2"), EVE, "Successful"));
    let out = w.auth.admin(None, &["unregister", "Eve"]);
    assert!(told(&out, EVE, "register"));
    assert!(!w.auth.is_authed(EVE));
    let out = w.auth.admin(None, &["register", "bad;name", "Strong-pass-1"]);
    assert!(out.iter().any(|e| matches!(e, Effect::Console(t) if t.contains("Invalid name"))));
}

#[test]
fn imported_authme_sha256_is_upgraded() {
    use sha2::{Digest, Sha256};
    let mut w = World::new("import");
    let salt = "abcdef0123456789";
    let inner = hex::encode(Sha256::digest(b"old-password"));
    let outer = hex::encode(Sha256::digest(format!("{inner}{salt}").as_bytes()));
    let accounts = format!(
        r#"{{"version":1,"accounts":{{"roman":{{"realname":"roman","hash":"$SHA${salt}${outer}","registered":1}}}}}}"#
    );
    std::fs::write(w.dir.join("accounts.json"), accounts).unwrap();
    w.auth.reload();
    w.auth.joined(ROMAN, "roman");
    assert!(told(&w.cmd(ROMAN, "roman", "login old-password"), ROMAN, "Successful"));
    let text = std::fs::read_to_string(w.dir.join("accounts.json")).unwrap();
    assert!(text.contains("$scrypt$"), "rehashed on login: {text}");
    assert!(!text.contains("$SHA$"));
}

#[test]
fn broken_accounts_file_is_never_overwritten() {
    let mut w = World::new("broken");
    std::fs::write(w.dir.join("accounts.json"), "{oops").unwrap();
    w.auth.reload();
    w.auth.joined(ROMAN, "Roman");
    let out = w.cmd(ROMAN, "Roman", "register Kv1tka-2026 Kv1tka-2026");
    assert!(told(&out, ROMAN, "error"), "{out:?}");
    assert_eq!(std::fs::read_to_string(w.dir.join("accounts.json")).unwrap(), "{oops");
    assert!(!w.auth.is_authed(ROMAN));
}

#[test]
fn game_mode_survives_a_crash_in_limbo() {
    let mut w = World::new("crash");
    w.auth.joined(ROMAN, "Roman");
    w.cmd(ROMAN, "Roman", "register Kv1tka-2026 Kv1tka-2026");
    w.auth.left(ROMAN);
    // Creative player joins, is put in limbo, and the server dies.
    w.auth.joined(ROMAN, "Roman");
    w.tick(&[online(ROMAN, "Roman", [0.0, 64.0, 0.0], 1)]);
    let (mut again, _) = Auth::open(&w.dir);
    again.joined(ROMAN, "Roman");
    // Rejoins as the spectator their file was saved as: the creative mode is still known.
    let out = again.tick(1000, &[online(ROMAN, "Roman", [0.0, 64.0, 0.0], 3)]);
    assert!(!commands(&out).iter().any(|c| c.starts_with("gamemode")));
    let mut out = again.player_command(ROMAN, "Roman", "login Kv1tka-2026").unwrap();
    for j in again.take_jobs() {
        out.extend(again.complete(auth::run(j)));
    }
    assert!(commands(&out).contains(&"gamemode creative Roman".to_string()), "{out:?}");
}

#[test]
fn time_format() {
    assert_eq!(auth::format_time(0), "1970-01-01 00:00 UTC");
    assert_eq!(auth::format_time(1_793_738_305), "2026-11-03 20:38 UTC");
}

#[test]
fn a_refused_or_unheard_visit_never_shortens_the_next_one() {
    let mut w = World::new("revisit");
    w.auth.joined(EVE, "Eve");
    let p = [online(EVE, "Eve", [0.0, 64.0, 0.0], 0)];
    w.tick(&p);
    // Eve leaves without the mod hearing it, and comes back 10 minutes later.
    for _ in 0..(600 * 20) {
        w.tick(&[]);
    }
    let out = w.tick(&p);
    assert!(!commands(&out).iter().any(|c| c.starts_with("kick")), "{out:?}");
    // Still a full 180 s from now.
    let mut kicked_at = None;
    for i in 0..(200 * 20) {
        if commands(&w.tick(&p)).iter().any(|c| c.starts_with("kick Eve")) {
            kicked_at = Some(i);
            break;
        }
    }
    let at = kicked_at.expect("kicked at the timeout");
    assert!(at >= 179 * 20, "kicked after {} ticks", at);

    // A new connection drops whatever was left.
    w.auth.connecting(EVE);
    w.auth.joined(EVE, "Eve");
    assert!(!commands(&w.tick(&p)).iter().any(|c| c.starts_with("kick")));
}

#[test]
fn logged_in_player_who_vanished_must_log_in_again() {
    let mut w = World::new("vanish");
    w.auth.joined(ROMAN, "Roman");
    w.cmd(ROMAN, "Roman", "register Kv1tka-2026 Kv1tka-2026");
    let p = [online(ROMAN, "Roman", [0.0, 64.0, 0.0], 0)];
    w.tick(&p);
    assert!(w.auth.is_authed(ROMAN));
    for _ in 0..100 {
        w.tick(&[]);
    }
    w.tick(&p);
    assert!(!w.auth.is_authed(ROMAN));
}

#[test]
fn join_after_a_paused_server_gets_the_full_timeout() {
    // An empty server pauses: no ticks at all for 10 minutes, then Eve joins.
    let mut w = World::new("paused");
    w.tick(&[]);
    w.now += 600_000;
    w.auth.connecting(EVE);
    w.auth.joined(EVE, "Eve");
    let p = [online(EVE, "Eve", [0.0, 64.0, 0.0], 0)];
    for _ in 0..(170 * 20) {
        assert!(!commands(&w.tick(&p)).iter().any(|c| c.starts_with("kick")));
    }
}

#[test]
fn sessions_by_name_and_ip() {
    let mut w = World::new("sessions");
    w.auth.set_address(ROMAN, "192.0.2.7".into());
    w.auth.joined(ROMAN, "Roman");
    w.cmd(ROMAN, "Roman", "register Kv1tka-2026 Kv1tka-2026");
    assert!(w.auth.is_authed(ROMAN));

    // Same name, same address: no password.
    w.auth.left(ROMAN);
    w.auth.connecting(ROMAN);
    w.auth.set_address(ROMAN, "192.0.2.7".into());
    let out = w.auth.joined(ROMAN, "Roman");
    assert!(told(&out, ROMAN, "Session resumed"), "{out:?}");
    assert!(w.auth.is_authed(ROMAN));
    let c = commands(&w.tick(&[online(ROMAN, "Roman", [0.0, 64.0, 0.0], 0)]));
    assert!(!c.iter().any(|c| c.starts_with("gamemode spectator") || c.starts_with("kick")), "{c:?}");

    // Another address: the password again.
    w.auth.left(ROMAN);
    w.auth.connecting(ROMAN);
    w.auth.set_address(ROMAN, "198.51.100.1".into());
    assert!(told(&w.auth.joined(ROMAN, "Roman"), ROMAN, "/login"));
    assert!(!w.auth.is_authed(ROMAN));
    // Someone else's name from Roman's address isn't Roman.
    w.auth.set_address(EVE, "192.0.2.7".into());
    w.auth.joined(EVE, "Eve");
    assert!(!w.auth.is_authed(EVE));

    // A password login from the new address moves the session there; /logout ends it.
    w.cmd(ROMAN, "Roman", "login Kv1tka-2026");
    assert!(w.auth.is_authed(ROMAN));
    w.cmd(ROMAN, "Roman", "logout");
    w.auth.left(ROMAN);
    w.auth.connecting(ROMAN);
    w.auth.set_address(ROMAN, "198.51.100.1".into());
    assert!(told(&w.auth.joined(ROMAN, "Roman"), ROMAN, "/login"));
}

#[test]
fn sessions_expire_and_can_be_off() {
    let mut w = World::new("sessions-off");
    w.auth.set_address(ROMAN, "192.0.2.7".into());
    w.auth.joined(ROMAN, "Roman");
    w.cmd(ROMAN, "Roman", "register Kv1tka-2026 Kv1tka-2026");
    // Pretend the last login was 4 days ago.
    let path = w.dir.join("accounts.json");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let old = auth::now_secs() - 4 * 24 * 3600;
    v["accounts"]["roman"]["last_login"] = old.into();
    std::fs::write(&path, v.to_string()).unwrap();
    w.auth.reload();
    w.auth.left(ROMAN);
    w.auth.connecting(ROMAN);
    w.auth.set_address(ROMAN, "192.0.2.7".into());
    assert!(told(&w.auth.joined(ROMAN, "Roman"), ROMAN, "/login"));
}

#[test]
fn a_session_login_is_announced_once() {
    let mut w = World::new("session-once");
    w.auth.set_address(ROMAN, "192.0.2.7".into());
    w.auth.joined(ROMAN, "Roman");
    w.cmd(ROMAN, "Roman", "register Kv1tka-2026 Kv1tka-2026");
    w.auth.left(ROMAN);

    // The server empties and pauses; Roman comes back: the tick sees him before the join
    // hook does, and the clock jumped.
    w.now += 600_000;
    w.auth.connecting(ROMAN);
    w.auth.set_address(ROMAN, "192.0.2.7".into());
    let p = [online(ROMAN, "Roman", [0.0, 64.0, 0.0], 0)];
    let mut all = w.tick(&p);
    all.extend(w.auth.joined(ROMAN, "Roman"));
    for _ in 0..100 {
        all.extend(w.tick(&p));
    }
    let resumed = all
        .iter()
        .filter(|e| matches!(e, Effect::Tell(_, t, _) if t.contains("Session resumed")))
        .count();
    assert_eq!(resumed, 1, "{all:?}");
    assert!(w.auth.is_authed(ROMAN));
    assert!(!commands(&all).iter().any(|c| c.starts_with("gamemode spectator")), "{all:?}");
}

#[test]
fn a_loopback_address_never_resumes_a_session() {
    // Behind a tunnel or proxy every player comes from 127.0.0.1: the address proves nothing.
    let mut w = World::new("sessions-loopback");
    w.auth.set_address(ROMAN, "127.0.0.1".into());
    w.auth.joined(ROMAN, "Roman");
    w.cmd(ROMAN, "Roman", "register Kv1tka-2026 Kv1tka-2026");
    assert!(w.auth.is_authed(ROMAN));
    w.auth.left(ROMAN);
    w.auth.connecting(ROMAN);
    w.auth.set_address(ROMAN, "127.0.0.1".into());
    assert!(told(&w.auth.joined(ROMAN, "Roman"), ROMAN, "/login"));
    assert!(!w.auth.is_authed(ROMAN));
    assert!(!auth::session_address("::1"));
    assert!(!auth::session_address("0.0.0.0"));
    assert!(!auth::session_address("nonsense"));
    assert!(auth::session_address("192.0.2.7"));
}
