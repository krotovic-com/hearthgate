//! The first bytes of every TLS connection, read before any decryption to
//! route by SNI.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = hearthgate_proxy::tls::parse_client_hello(data);
    let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
    rt.block_on(async {
        let mut reader = data;
        if let Ok((raw, _)) = hearthgate_proxy::tls::read_client_hello(&mut reader).await {
            // Every byte read is handed on unchanged.
            assert!(data.starts_with(&raw));
        }
    });
});
