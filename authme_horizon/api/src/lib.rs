//! AuthMe for Horizon: a server-side login mod, ported from AuthMeReloaded
//! (https://github.com/AuthMe/AuthMeReloaded, GPL-3.0, by the AuthMe team).
//! Copyright (C) 2026 the port's authors. GPL-3.0-or-later, as AuthMeReloaded.
//!
//! This file is the glue between Horizon's hook points and the rules in `core/`: each hook
//! asks the rules, and the [`Effect`]s they return are carried out here. The rules are pure
//! Rust, tested on their own (see `core-tests/`).
//!
//! Only on a dedicated server (`NativeAuthority::DedicatedServer`): in single player and on a
//! LAN host the mod does nothing.

mod core;

use crate::core::auth::{self, Auth, Done, Effect, Job, Online, Tone};
use horizon_client::mod_api::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, mpsc};
use std::time::Instant;

pub fn hooks() -> Box<dyn Hooks> {
    let (tx, rx) = mpsc::channel();
    Box::new(AuthMe {
        auth: Mutex::new(None),
        done_tx: Mutex::new(tx),
        done_rx: Mutex::new(rx),
        commands: Mutex::new(Vec::new()),
        active: AtomicBool::new(false),
        own_command: AtomicBool::new(false),
        clock: Instant::now(),
    })
}

struct AuthMe {
    /// Opened at the first tick of a dedicated server.
    auth: Mutex<Option<Auth>>,
    done_tx: Mutex<mpsc::Sender<Done>>,
    done_rx: Mutex<mpsc::Receiver<Done>>,
    /// Server commands waiting for the next tick (they can't run inside a player's hook).
    commands: Mutex<Vec<String>>,
    /// A dedicated server's world is open.
    active: AtomicBool,
    /// The mod itself is running a command (its `gamemode` must not be refused by its own
    /// game-mode guard).
    own_command: AtomicBool,
    clock: Instant,
}

fn color(tone: Tone) -> u32 {
    match tone {
        Tone::Info => 0x55FFFF,
        Tone::Good => 0x55FF55,
        Tone::Bad => 0xFF5555,
    }
}

fn line(text: &str, tone: Tone) -> Text {
    Text::colored(text.to_owned(), color(tone))
}

impl AuthMe {
    fn lock(&self) -> MutexGuard<'_, Option<Auth>> {
        // A panic while the lock was held must not take every later hook down with it.
        self.auth.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn active(&self) -> bool {
        self.active.load(Ordering::Relaxed)
    }

    /// Whether the acting player (if any) may act. No acting player: the world itself, allowed.
    fn acting_may_act(&self, host: &dyn Host) -> bool {
        if !self.active() {
            return true;
        }
        let Some(p) = host.acting_player() else {
            return true;
        };
        self.may_act(p.uuid)
    }

    fn may_act(&self, uuid: u128) -> bool {
        if !self.active() {
            return true;
        }
        self.lock().as_ref().is_none_or(|a| a.is_authed(uuid))
    }

    fn deny(&self, host: &mut dyn Host) -> Flow {
        if let Some(p) = host.acting_player() {
            let text = self.lock().as_ref().map(|a| {
                if a.config.language == "uk" {
                    "Спершу авторизуйтесь: /login <пароль>"
                } else {
                    "Log in first: /login <password>"
                }
            });
            if let Some(text) = text {
                host.action_bar(p.uuid, line(text, Tone::Bad));
            }
        }
        Flow::Cancel
    }

    /// Carries out `effects`. Lines to `replying` go into `replies` when given (inside
    /// `player_command`, where telling a player reaches nobody).
    fn apply(
        &self,
        host: &mut dyn Host,
        effects: Vec<Effect>,
        mut replies: Option<(u128, &mut Vec<Text>)>,
    ) {
        for effect in effects {
            match effect {
                Effect::Tell(uuid, text, tone) => match &mut replies {
                    Some((who, lines)) if *who == uuid => lines.push(line(&text, tone)),
                    _ => {
                        host.tell(uuid, line(&text, tone));
                    }
                },
                Effect::ActionBar(uuid, text) => {
                    host.action_bar(uuid, line(&text, Tone::Info));
                }
                Effect::Broadcast(text) => host.broadcast(line(&text, Tone::Info)),
                Effect::Console(text) => log::info!("{text}"),
                Effect::Command(c) => self
                    .commands
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(c),
            }
        }
    }

    fn spawn(&self, jobs: Vec<Job>) {
        for job in jobs {
            let tx = self.done_tx.lock().unwrap_or_else(|p| p.into_inner()).clone();
            let spawned = std::thread::Builder::new()
                .name("authme-hash".into())
                .spawn(move || {
                    let _ = tx.send(auth::run(job));
                });
            if let Err(e) = spawned {
                log::error!("[AuthMe] could not start a hashing thread: {e}");
            }
        }
    }

    fn run_commands(&self, host: &mut dyn Host) {
        let commands = std::mem::take(&mut *self.commands.lock().unwrap_or_else(|p| p.into_inner()));
        self.own_command.store(true, Ordering::Relaxed);
        for c in commands {
            if !host.run_command(&c) {
                log::warn!("[AuthMe] the server refused: {c}");
            }
        }
        self.own_command.store(false, Ordering::Relaxed);
    }
}

impl Hooks for AuthMe {
    fn server_tick(&self, host: &mut dyn Host, event: &mut ServerTickEvent<'_>) {
        if event.context.authority != NativeAuthority::DedicatedServer {
            return;
        }
        if !matches!(event.phase, ServerTickPhase::ServerTickEnd { .. }) {
            return;
        }
        let mut effects = Vec::new();
        let jobs = {
            let mut guard = self.lock();
            if guard.is_none() {
                match host.config_dir() {
                    Ok(dir) => {
                        let (a, opened) = Auth::open(&dir.join("authme"));
                        effects.extend(opened);
                        *guard = Some(a);
                        self.active.store(true, Ordering::Relaxed);
                    }
                    Err(e) => {
                        log::error!("[AuthMe] no config folder: {e}");
                        return;
                    }
                }
            }
            let a = guard.as_mut().expect("opened");
            let done: Vec<Done> = self
                .done_rx
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .try_iter()
                .collect();
            for d in done {
                effects.extend(a.complete(d));
            }
            let players: Vec<Online> = host
                .server_players()
                .into_iter()
                .map(|p| Online {
                    uuid: p.uuid,
                    name: p.name,
                    dimension: p.dimension,
                    position: p.position,
                    game_mode: p.game_mode,
                    // Horizon 2944480 doesn't give players' network ids yet (added later as
                    // `NativeServerPlayer::entity_id`): damage protection then rests on spectator mode.
                    entity_id: 0,
                })
                .collect();
            let now = self.clock.elapsed().as_millis() as u64;
            // Chat refused since the last tick: tell them why (with the tick's own lines).
            take_addresses(a);
            for uuid in horizon_client::authme_gate::take_denied() {
                let _ = a.chat(uuid);
            }
            effects.extend(a.tick(now, &players));
            horizon_client::authme_gate::publish(true, a.authed_uuids());
            a.take_jobs()
        };
        self.spawn(jobs);
        self.apply(host, effects, None);
        self.run_commands(host);
    }

    fn register_commands(&self, host: &mut dyn Host, event: &mut RegisterCommandsEvent<'_>) {
        let _ = host;
        // So the chat box completes them. The mod answers these itself in `player_command`,
        // before the game parses (or logs) them.
        for usage in [
            "login <password:word>",
            "l <password:word>",
            "register <password:word> <confirm:word>",
            "reg <password:word> <confirm:word>",
            "logout",
            "changepassword <old:word> <new:word>",
            "unregister <password:word>",
        ] {
            event.commands.push(NativeCommand::new(usage));
        }
        let level = self.lock().as_ref().map_or(3, |a| a.config.admin_level);
        for usage in [
            "authme [args:text]",
        ] {
            event.commands.push(NativeCommand::op(usage, level));
        }
    }

    fn player_command(&self, host: &mut dyn Host, event: &mut PlayerCommandEvent<'_>) -> Flow {
        if !self.active() {
            return Flow::Continue;
        }
        let effects = {
            let mut guard = self.lock();
            let Some(a) = guard.as_mut() else {
                return Flow::Continue;
            };
            let effects = a.player_command(event.player, event.name, event.command);
            let jobs = a.take_jobs();
            drop(guard);
            self.spawn(jobs);
            effects
        };
        match effects {
            None => Flow::Continue,
            Some(effects) => {
                self.apply(host, effects, Some((event.player, &mut *event.replies)));
                Flow::Cancel
            }
        }
    }

    fn perform_command(&self, host: &mut dyn Host, event: &mut PerformCommandEvent<'_>) -> Flow {
        let mut words = event.command.split_whitespace();
        if !words.next().is_some_and(|w| w.eq_ignore_ascii_case("authme")) {
            return Flow::Continue;
        }
        let args: Vec<&str> = words.collect();
        let uuid = event.player.uuid;
        let (effects, jobs) = {
            let mut guard = self.lock();
            let Some(a) = guard.as_mut() else {
                host.tell(uuid, line("[AuthMe] not ready yet", Tone::Bad));
                return Flow::Cancel;
            };
            if event.player.permission < a.config.admin_level || !a.is_authed(uuid) {
                drop(guard);
                host.tell(uuid, line("[AuthMe] no permission", Tone::Bad));
                return Flow::Cancel;
            }
            let effects = a.admin(Some(uuid), &args);
            (effects, a.take_jobs())
        };
        self.spawn(jobs);
        self.apply(host, effects, None);
        Flow::Cancel
    }

    fn perform_console_command(
        &self,
        host: &mut dyn Host,
        event: &mut PerformConsoleCommandEvent<'_>,
    ) -> Flow {
        let mut words = event.command.split_whitespace();
        if !words.next().is_some_and(|w| w.eq_ignore_ascii_case("authme")) {
            return Flow::Continue;
        }
        let args: Vec<&str> = words.collect();
        let (effects, jobs) = {
            let mut guard = self.lock();
            let Some(a) = guard.as_mut() else {
                host.feedback(Text::literal("[AuthMe] not ready yet"));
                return Flow::Cancel;
            };
            (a.admin(None, &args), a.take_jobs())
        };
        self.spawn(jobs);
        for e in &effects {
            if let Effect::Console(text) = e {
                host.feedback(Text::literal(text.clone()));
            }
        }
        let rest = effects
            .into_iter()
            .filter(|e| !matches!(e, Effect::Console(_)))
            .collect();
        self.apply(host, rest, None);
        Flow::Cancel
    }

    fn player_login(&self, host: &mut dyn Host, event: &mut PlayerLoginEvent<'_>) -> Flow {
        let _ = host;
        if !self.active() {
            return Flow::Continue;
        }
        let refused = {
            let mut guard = self.lock();
            let Some(a) = guard.as_mut() else {
                return Flow::Continue;
            };
            let refused = a.check_login(event.name).err();
            if refused.is_none() {
                a.connecting(event.player);
            }
            refused
        };
        match refused {
            Some(reason) => {
                *event.reason = Some(line(&reason, Tone::Bad));
                Flow::Cancel
            }
            None => Flow::Continue,
        }
    }

    fn player_logged_in(&self, host: &mut dyn Host, event: &mut PlayerLoggedInEvent<'_>) {
        if !self.active() {
            return;
        }
        let effects = match self.lock().as_mut() {
            Some(a) => {
                take_addresses(a);
                a.joined(event.player, event.name)
            }
            None => return,
        };
        self.apply(host, effects, None);
    }

    fn player_logged_out(&self, host: &mut dyn Host, event: &mut PlayerLoggedOutEvent<'_>) {
        let _ = host;
        if let Some(a) = self.lock().as_mut() {
            a.left(event.player);
        }
    }

    // ---- what a player not logged in may not do ---------------------------------------------

    fn player_game_mode_changed(
        &self,
        host: &mut dyn Host,
        event: &mut PlayerGameModeChangedEvent<'_>,
    ) -> Flow {
        let _ = host;
        if self.own_command.load(Ordering::Relaxed) || self.may_act(event.player) {
            Flow::Continue
        } else {
            Flow::Cancel
        }
    }

    fn left_click_block(&self, host: &mut dyn Host, event: &mut LeftClickBlockEvent) -> Flow {
        let _ = event;
        if self.acting_may_act(host) { Flow::Continue } else { self.deny(host) }
    }

    fn right_click_block(&self, host: &mut dyn Host, event: &mut RightClickBlockEvent) -> Flow {
        let _ = event;
        if self.acting_may_act(host) { Flow::Continue } else { self.deny(host) }
    }

    fn use_item(&self, host: &mut dyn Host, event: &mut UseItemEvent<'_>) -> Flow {
        let _ = event;
        if self.acting_may_act(host) { Flow::Continue } else { self.deny(host) }
    }

    fn use_item_on(&self, host: &mut dyn Host, event: &mut UseItemOnEvent<'_>) -> Flow {
        let _ = event;
        if self.acting_may_act(host) { Flow::Continue } else { self.deny(host) }
    }

    fn attack_entity(&self, host: &mut dyn Host, event: &mut AttackEntityEvent<'_>) -> Flow {
        let _ = event;
        if self.acting_may_act(host) { Flow::Continue } else { self.deny(host) }
    }

    fn interact_entity(&self, host: &mut dyn Host, event: &mut InteractEntityEvent<'_>) -> Flow {
        let _ = event;
        if self.acting_may_act(host) { Flow::Continue } else { self.deny(host) }
    }

    fn item_picked_up(&self, host: &mut dyn Host, event: &mut ItemPickedUpEvent<'_>) -> Flow {
        let _ = host;
        if self.may_act(event.player) { Flow::Continue } else { Flow::Cancel }
    }

    fn container_opened(&self, host: &mut dyn Host, event: &mut ContainerOpenedEvent<'_>) -> Flow {
        if self.may_act(event.player) { Flow::Continue } else { self.deny(host) }
    }

    fn player_swap_hands(&self, host: &mut dyn Host, event: &mut PlayerSwapHandsEvent<'_>) -> Flow {
        let _ = host;
        if self.may_act(event.player) { Flow::Continue } else { Flow::Cancel }
    }

    fn living_incoming_damage(&self, event: &mut LivingIncomingDamageEvent<'_>) -> Flow {
        if !self.active() {
            return Flow::Continue;
        }
        let guard = self.lock();
        let Some(a) = guard.as_ref() else {
            return Flow::Continue;
        };
        // Neither hurt nor hurting before logging in.
        if a.is_protected(event.entity) || a.is_protected(event.causing) || a.is_protected(event.direct) {
            Flow::Cancel
        } else {
            Flow::Continue
        }
    }
}

/// The addresses the network admitted players from, for sessions.
fn take_addresses(a: &mut Auth) {
    for (uuid, ip) in horizon_client::authme_gate::take_addresses() {
        a.set_address(uuid, ip);
    }
}
