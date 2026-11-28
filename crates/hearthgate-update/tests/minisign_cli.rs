//! A signature made by the `minisign` command line tool, as the release
//! workflow makes them, verifies.

use hearthgate_update::Version;
use hearthgate_update::verify::verify_sums;

#[test]
fn verifies_a_signature_from_the_minisign_tool() {
    let key = include_str!("fixtures/cli-key.pub").trim();
    let key = minisign_verify::PublicKey::from_base64(key).unwrap();
    let sums = include_bytes!("fixtures/SHA256SUMS");
    let sig = include_str!("fixtures/SHA256SUMS.minisig");
    let v = Version::parse("1.3.0").unwrap();
    let checked = verify_sums(sums, sig, v, std::slice::from_ref(&key)).unwrap();
    checked.check_bytes("hearthgate_1.3.0_amd64.deb", b"hearthgate 1.3.0\n").unwrap();
    assert!(verify_sums(sums, sig, Version::parse("1.3.1").unwrap(), &[key]).is_err());
}
