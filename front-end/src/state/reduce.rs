//! The reducer: `(State, Action) -> Vec<Effect>`.
//!
//! Pure on purpose — no `web_sys`, no waveform handle, no LSP — so it can be
//! unit tested on the host target (`cargo test -p front-end --bins`) and so a
//! change is always "this fact changes these fields". Anything that needs the
//! outside world is returned as an [`Effect`] for the app to execute.

use super::action::{Action, Effect};
use super::state::State;
use super::types::{clamp_ratio, clamp_sidebar};

pub fn reduce(state: &mut State, action: Action) -> Vec<Effect> {
    match action {
        Action::Layout(layout) => {
            state.layout = layout.clamped();
        }
        Action::ResizeSidebar { width, max } => {
            state.layout.sidebar_w = clamp_sidebar(width, max);
        }
        Action::ToggleDrawer => {
            state.layout.drawer_open = !state.layout.drawer_open;
        }
        Action::SetEditorRatio(ratio) => {
            state.layout.editor_ratio = clamp_ratio(ratio);
        }
        Action::Status(status) => {
            state.nav.status = Some(status);
        }
        Action::ClearStatus => {
            state.nav.status = None;
        }
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::types::{Layout, Status, StatusTone, EDITOR_RATIO_MIN, SIDEBAR_MIN};

    fn state() -> State {
        State::default()
    }

    #[test]
    fn resize_sidebar_clamps_into_the_window() {
        let mut state = state();
        reduce(&mut state, Action::ResizeSidebar { width: 340.0, max: 640.0 });
        assert_eq!(state.layout.sidebar_w, 340.0);
        reduce(&mut state, Action::ResizeSidebar { width: 5000.0, max: 640.0 });
        assert_eq!(state.layout.sidebar_w, 640.0);
        reduce(&mut state, Action::ResizeSidebar { width: 10.0, max: 640.0 });
        assert_eq!(state.layout.sidebar_w, SIDEBAR_MIN);
    }

    #[test]
    fn toggle_drawer_flips_the_flag() {
        let mut state = state();
        assert!(state.layout.drawer_open);
        reduce(&mut state, Action::ToggleDrawer);
        assert!(!state.layout.drawer_open);
        reduce(&mut state, Action::ToggleDrawer);
        assert!(state.layout.drawer_open);
    }

    #[test]
    fn editor_ratio_is_clamped_on_write() {
        let mut state = state();
        reduce(&mut state, Action::SetEditorRatio(0.9));
        assert_eq!(state.layout.editor_ratio, 0.75);
        reduce(&mut state, Action::SetEditorRatio(0.01));
        assert_eq!(state.layout.editor_ratio, EDITOR_RATIO_MIN);
    }

    #[test]
    fn replacing_the_layout_clamps_every_field() {
        let mut state = state();
        reduce(
            &mut state,
            Action::Layout(Layout { sidebar_w: 4000.0, editor_ratio: 0.02, drawer_open: false }),
        );
        assert_eq!(state.layout.sidebar_w, 4000.0_f64.max(SIDEBAR_MIN));
        assert_eq!(state.layout.editor_ratio, EDITOR_RATIO_MIN);
        assert!(!state.layout.drawer_open);
    }

    #[test]
    fn status_is_set_and_cleared() {
        let mut state = state();
        assert_eq!(state.nav.status, None);
        reduce(&mut state, Action::Status(Status::error("top.sv: not utf-8")));
        let status = state.nav.status.clone().expect("status");
        assert_eq!(status.tone, StatusTone::Error);
        assert_eq!(status.text, "top.sv: not utf-8");
        reduce(&mut state, Action::ClearStatus);
        assert_eq!(state.nav.status, None);
    }

    #[test]
    fn step_one_produces_no_effects() {
        let mut state = state();
        assert!(reduce(&mut state, Action::ToggleDrawer).is_empty());
        assert!(reduce(&mut state, Action::Status(Status::info("ok"))).is_empty());
    }
}
