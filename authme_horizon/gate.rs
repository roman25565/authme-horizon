// AuthMe for Horizon (GPL-3.0): the gate the chat and drop hooks ask. It lives in the client
// library (`horizon_client::authme_gate`), where the chat is handled, and the mod's trait
// package keeps it up to date every server tick.

/// The mod is running on a dedicated server: the gate is closed to players not logged in.
static ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The players who are logged in (UUIDs).
static AUTHED: std::sync::Mutex<Vec<u128>> = std::sync::Mutex::new(Vec::new());
/// Players whose chat was refused since the last tick, to be told why.
static DENIED: std::sync::Mutex<Vec<u128>> = std::sync::Mutex::new(Vec::new());

/// Sets whether the gate is closed, and who is logged in.
pub fn publish(active: bool, authed: Vec<u128>) {
    *AUTHED.lock().unwrap_or_else(|p| p.into_inner()) = authed;
    ACTIVE.store(active, std::sync::atomic::Ordering::Release);
}

/// Whether `uuid` may act. Refusals are remembered for `take_denied` when `remember`.
pub fn may_act(uuid: u128, remember: bool) -> bool {
    if !ACTIVE.load(std::sync::atomic::Ordering::Acquire) {
        return true;
    }
    if AUTHED.lock().unwrap_or_else(|p| p.into_inner()).contains(&uuid) {
        return true;
    }
    if remember {
        let mut denied = DENIED.lock().unwrap_or_else(|p| p.into_inner());
        if denied.len() < 1024 && !denied.contains(&uuid) {
            denied.push(uuid);
        }
    }
    false
}

/// The players refused since the last call.
pub fn take_denied() -> Vec<u128> {
    std::mem::take(&mut *DENIED.lock().unwrap_or_else(|p| p.into_inner()))
}

/// The address each player connected from, set by the admission hook on the network thread
/// and taken by the trait mod (sessions by name and IP).
static ADDRESSES: std::sync::Mutex<Vec<(u128, String)>> = std::sync::Mutex::new(Vec::new());

/// Remembers the address `uuid` is connecting from.
pub fn remember_address(uuid: u128, ip: std::net::IpAddr) {
    let mut all = ADDRESSES.lock().unwrap_or_else(|p| p.into_inner());
    all.retain(|(u, _)| *u != uuid);
    if all.len() >= 1024 {
        all.remove(0);
    }
    // An IPv4 client on a dual-stack socket shows as ::ffff:a.b.c.d.
    let ip = match ip {
        std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped().map(std::net::IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    };
    all.push((uuid, ip.to_string()));
}

/// The addresses remembered since the last call.
pub fn take_addresses() -> Vec<(u128, String)> {
    std::mem::take(&mut *ADDRESSES.lock().unwrap_or_else(|p| p.into_inner()))
}

/// A second connection under the name of a player who is online and logged in: refused, so
/// nobody can kick a player off by joining with their name (offline mode). The player who is
/// online but not logged in yet is still replaced, as vanilla does, so a stranger sitting on
/// someone's name can't keep them out.
pub fn refuse_duplicate(uuid: u128) -> bool {
    ACTIVE.load(std::sync::atomic::Ordering::Acquire)
        && AUTHED.lock().unwrap_or_else(|p| p.into_inner()).contains(&uuid)
}
