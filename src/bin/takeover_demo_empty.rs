//! Demo "game" with NO entities at all: zero Transform components ever
//! spawn. The vacuity guard must flag this as VacuousRun — a Pass on
//! an empty world proves nothing (bot inputs hitting a void).
#[cfg(feature = "takeover")]
mod game {
    use bevy_app::prelude::*;
    use bevy_playtest::PlaytestPlugin;

    pub fn main() {
        let mut app = App::new();
        app.add_plugins(PlaytestPlugin);
        app.run();
    }
}

#[cfg(feature = "takeover")]
fn main() {
    game::main()
}

#[cfg(not(feature = "takeover"))]
fn main() {}
