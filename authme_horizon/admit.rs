fn authme_admit(
    address: std::net::SocketAddr,
    uuid: u128,
    shared: &Shared,
) -> std::ops::ControlFlow<Option<horizon_world::nbt::Tag>> {
    // A memory connection (single player) has no network peer and logs in from port 0.
    if address.port() != 0 {
        crate::authme_gate::remember_address(uuid, address.ip());
    }
    // Vanilla kicks the player already playing under this name; with offline mode that lets
    // anyone kick anyone. A logged-in player keeps their place and the newcomer is turned away.
    if shared.is_online(uuid) && crate::authme_gate::refuse_duplicate(uuid) {
        return std::ops::ControlFlow::Break(Some(horizon_protocol::text::literal(
            "Гравець з цим ніком уже на сервері. / A player with this name is already online.",
        )));
    }
    std::ops::ControlFlow::Continue(())
}
