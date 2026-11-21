//! Queries to the internal DNS server, which any VPN device can send.
#![no_main]

use std::sync::LazyLock;

use hearthgate_vpn::dns::{Settings, Zone, answer};
use libfuzzer_sys::fuzz_target;

static SETTINGS: LazyLock<Settings> = LazyLock::new(|| Settings {
    listen: Vec::new(),
    zone: Zone {
        zone: "home.internal".into(),
        names: [
            ("nas.home.internal".to_owned(), vec!["10.8.0.1".parse().unwrap()]),
            ("app.example.com".to_owned(), vec!["10.8.0.1".parse().unwrap(), "fd00::1".parse().unwrap()]),
        ]
        .into(),
    },
    // No upstream: nothing leaves the process.
    upstream: Vec::new(),
});

fuzz_target!(|data: &[u8]| {
    let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    let _ = rt.block_on(answer(data, &SETTINGS));
});
