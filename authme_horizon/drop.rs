fn authme_drop(game: &mut crate::game::Game<'_>) -> std::ops::ControlFlow<()> {
    let player = game.native_acting_id();
    if crate::authme_gate::may_act(player, false) {
        std::ops::ControlFlow::Continue(())
    } else {
        std::ops::ControlFlow::Break(())
    }
}
