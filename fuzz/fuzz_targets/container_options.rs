//! Port and volume options of managed containers.
#![no_main]

use hearthgate_model::{Publish, Volume};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let _ = Publish::parse(text);
    let _ = Volume::parse(text);
});
