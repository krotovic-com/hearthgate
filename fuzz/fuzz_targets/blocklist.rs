//! Blocklist feeds downloaded from the internet.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = hearthgate_firewall::feeds::parse(&text);
});
