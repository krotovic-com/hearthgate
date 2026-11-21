//! Desired state from imports and the admin API.
#![no_main]

use hearthgate_model::DesiredState;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(state) = DesiredState::from_toml(text) else {
        return;
    };
    if state.validate().is_ok() {
        // A valid state survives a round trip unchanged.
        let again = DesiredState::from_toml(&state.to_toml()).unwrap();
        assert_eq!(again.normalized(), state.normalized(), "TOML round trip");
    }
});
