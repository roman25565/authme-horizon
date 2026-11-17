fn authme_chat(hub: &Hub, i: usize) -> std::ops::ControlFlow<()> {
    match hub.guests.get(i) {
        Some(g) if !crate::authme_gate::may_act(g.uuid, true) => std::ops::ControlFlow::Break(()),
        _ => std::ops::ControlFlow::Continue(()),
    }
}
