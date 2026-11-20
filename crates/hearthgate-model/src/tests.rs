use crate::*;

const EXAMPLE: &str = include_str!("../../../packaging/examples/state.toml");

fn messages(text: &str) -> Vec<String> {
    match DesiredState::from_toml(text) {
        Ok(_) => Vec::new(),
        Err(ParseError::Invalid(errors)) => errors.0.iter().map(ToString::to_string).collect(),
        Err(other) => panic!("unexpected parse error: {other}"),
    }
}

fn assert_problem(text: &str, needle: &str) {
    let found = messages(text);
    assert!(
        found.iter().any(|m| m.contains(needle)),
        "expected a problem containing {needle:?}, got {found:#?}"
    );
}

#[test]
fn example_is_valid() {
    let state = DesiredState::from_toml(EXAMPLE).unwrap();
    assert_eq!(state.interfaces.len(), 3);
    assert_eq!(state.routes.len(), 3);
    assert!(state.interface("web").unwrap().wants_acme());
}

#[test]
fn toml_round_trip_is_lossless() {
    let state = DesiredState::from_toml(EXAMPLE).unwrap();
    let exported = state.to_toml();
    let again = DesiredState::from_toml(&exported).unwrap();
    assert_eq!(state, again);
    assert_eq!(exported, again.to_toml());
    assert_eq!(state.to_canonical_json(), again.to_canonical_json());
}

#[test]
fn json_round_trip_is_lossless() {
    let state = DesiredState::from_toml(EXAMPLE).unwrap();
    let back = DesiredState::from_json(&state.to_canonical_json()).unwrap();
    assert_eq!(state, back);
}

#[test]
fn normalization_ignores_order_and_case() {
    let a = DesiredState::from_toml(
        r#"
[[interfaces]]
name = "a"
domains = ["B.example.com", "a.example.com"]
listeners = [{ port = 81, mode = "http" }, { port = 80, mode = "http" }]
"#,
    )
    .unwrap();
    let b = DesiredState::from_toml(
        r#"
[[interfaces]]
name = "a"
domains = ["a.example.com", "b.example.com."]
listeners = [{ port = 80, mode = "http" }, { port = 81, mode = "http" }]
"#,
    )
    .unwrap();
    assert_eq!(a.to_canonical_json(), b.to_canonical_json());
}

#[test]
fn empty_state_is_valid() {
    assert_eq!(messages(""), [] as [std::string::String; 0]);
}

#[test]
fn unknown_fields_are_rejected() {
    let err = DesiredState::from_toml("[[interfaces]]\nname = \"a\"\nlistners = []\n").unwrap_err();
    assert!(matches!(err, ParseError::Toml(_)), "{err}");
}

#[test]
fn rejects_bad_names_and_duplicates() {
    assert_problem(
        r#"
[[services]]
name = "My_Service"
kind = "external"
host = "127.0.0.1"
ports = { http = 80 }
"#,
        "invalid name",
    );
    assert_problem(
        r#"
[[services]]
name = "a"
kind = "external"
host = "127.0.0.1"
ports = { http = 80 }
[[services]]
name = "a"
kind = "external"
host = "127.0.0.1"
ports = { http = 81 }
"#,
        "more than once",
    );
}

#[test]
fn https_needs_terms_or_manual_certificate() {
    let base = r#"
[[interfaces]]
name = "web"
domains = ["app.example.com"]
listeners = [{ port = 443, mode = "https" }]
"#;
    assert_problem(base, "accept_terms");
    let manual = base.replace("name = \"web\"", "name = \"web\"\ncertificate = \"mine\"");
    assert_eq!(messages(&manual), [] as [std::string::String; 0]);
}

#[test]
fn wildcards_need_manual_certificates() {
    assert_problem(
        r#"
acme = { accept_terms = true }
[[interfaces]]
name = "web"
domains = ["*.example.com"]
listeners = [{ port = 443, mode = "https" }]
"#,
        "wildcard",
    );
}

#[test]
fn udp_only_on_raw() {
    assert_problem(
        r#"
[[interfaces]]
name = "web"
domains = ["a.example.com"]
listeners = [{ port = 80, mode = "http", protocol = "udp" }]
"#,
        "TCP only",
    );
}

#[test]
fn shared_ports() {
    // Two TLS interfaces on 443 is the normal multi-site setup.
    let ok = r#"
acme = { accept_terms = true }
[[interfaces]]
name = "a"
domains = ["a.example.com"]
listeners = [{ port = 443, mode = "https" }]
[[interfaces]]
name = "b"
domains = ["b.example.com"]
listeners = [{ port = 443, mode = "tls-passthrough" }]
"#;
    assert!(messages(ok).is_empty(), "{:?}", messages(ok));
    assert_problem(
        &ok.replace("b.example.com", "a.example.com"),
        "served by both",
    );
    assert_problem(
        &ok.replace("mode = \"tls-passthrough\"", "mode = \"http\""),
        "cannot share",
    );
    assert_problem(
        r#"
[[interfaces]]
name = "a"
listeners = [{ port = 2222, mode = "raw" }]
[[interfaces]]
name = "b"
listeners = [{ port = 2222, mode = "raw" }]
"#,
        "raw forwarding cannot share",
    );
    // TCP and UDP on the same number are different sockets.
    assert_eq!(
        messages(
            r#"
[[interfaces]]
name = "a"
listeners = [{ port = 53, mode = "raw" }]
[[interfaces]]
name = "b"
listeners = [{ port = 53, mode = "raw", protocol = "udp" }]
"#
        ),
        [] as [std::string::String; 0]
    );
}

#[test]
fn route_references_are_checked() {
    let base = r#"
[[interfaces]]
name = "web"
domains = ["a.example.com"]
listeners = [{ port = 80, mode = "http" }]
[[services]]
name = "app"
kind = "external"
host = "127.0.0.1"
ports = { http = 8080 }
"#;
    assert_problem(
        &format!(
            "{base}[[routes]]\nname=\"r\"\ninterface=\"nope\"\nlistener=80\ntargets=[{{service=\"app\",port=\"http\"}}]\n"
        ),
        "interface nope does not exist",
    );
    assert_problem(
        &format!(
            "{base}[[routes]]\nname=\"r\"\ninterface=\"web\"\nlistener=81\ntargets=[{{service=\"app\",port=\"http\"}}]\n"
        ),
        "no listener on port 81",
    );
    assert_problem(
        &format!(
            "{base}[[routes]]\nname=\"r\"\ninterface=\"web\"\nlistener=80\ntargets=[{{service=\"app\",port=\"https\"}}]\n"
        ),
        "no port named https",
    );
    assert_problem(
        &format!("{base}[[routes]]\nname=\"r\"\ninterface=\"web\"\nlistener=80\ntargets=[]\n"),
        "at least one target",
    );
    assert_problem(
        &format!(
            "{base}[[routes]]\nname=\"r\"\ninterface=\"web\"\nlistener=80\nhosts=[\"b.example.com\"]\ntargets=[{{service=\"app\",port=8080}}]\n"
        ),
        "not one of interface",
    );
    let dup = format!(
        "{base}[[routes]]\nname=\"r1\"\ninterface=\"web\"\nlistener=80\ntargets=[{{service=\"app\",port=8080}}]\n\
         [[routes]]\nname=\"r2\"\ninterface=\"web\"\nlistener=80\nhosts=[\"a.example.com\"]\ntargets=[{{service=\"app\",port=8080}}]\n"
    );
    assert_problem(&dup, "already matches host a.example.com");
    let paths = dup.replace(
        "hosts=[\"a.example.com\"]",
        "hosts=[\"a.example.com\"]\npath_prefix=\"/api\"",
    );
    assert!(messages(&paths).is_empty(), "{:?}", messages(&paths));
}

#[test]
fn raw_routes_are_exclusive() {
    assert_problem(
        r#"
[[interfaces]]
name = "ssh"
listeners = [{ port = 2222, mode = "raw" }]
[[services]]
name = "sshd"
kind = "external"
host = "127.0.0.1"
ports = { ssh = 22 }
[[routes]]
name = "a"
interface = "ssh"
listener = 2222
targets = [{ service = "sshd", port = "ssh" }]
[[routes]]
name = "b"
interface = "ssh"
listener = 2222
targets = [{ service = "sshd", port = "ssh" }]
"#,
        "already has route a",
    );
}

#[test]
fn service_kinds() {
    assert_problem(
        "[[services]]\nname=\"a\"\nkind=\"external\"\nports={p=1}\n",
        "need a host",
    );
    assert_problem(
        "[[services]]\nname=\"a\"\nkind=\"docker\"\nports={p=1}\n",
        "container name",
    );
    assert_problem(
        "[[services]]\nname=\"a\"\nkind=\"external\"\nhost=\"x\"\nports={p=1}\nhealth={type=\"http\",interval=\"1s\",timeout=\"5s\"}\n",
        "timeout must not be longer",
    );
}

#[test]
fn acme_directory_defaults() {
    let mut acme = AcmeSettings::default();
    assert_eq!(acme.directory_url(), LETS_ENCRYPT_PRODUCTION);
    acme.staging = true;
    assert_eq!(acme.directory_url(), LETS_ENCRYPT_STAGING);
    acme.directory = Some("https://localhost:14000/dir".into());
    assert_eq!(acme.directory_url(), "https://localhost:14000/dir");
}

#[test]
#[ignore = "prints the exported example for manual inspection"]
fn print_export() {
    println!("{}", DesiredState::from_toml(EXAMPLE).unwrap().to_toml());
}

#[test]
fn vpn_is_checked() {
    const KEY: &str = "Y2FD6uTgrq+/bbBAXCNOtE+PGk8Papoh7qaWBpQUXn0=";
    let peer = |name: &str, key: &str, address: &str, access: &str| {
        format!(
            "[[vpn.peers]]\nname = \"{name}\"\npublic_key = \"{key}\"\naddress = \"{address}\"\naccess = {access}\n"
        )
    };
    let base = "[vpn]\nenabled = true\n";
    let ok = format!(
        "[[services]]\nname = \"nas\"\nkind = \"external\"\nhost = \"192.168.1.2\"\nports = {{ web = 80 }}\n\n{base}{}{}",
        peer("phone", KEY, "10.88.0.2", "\"all\""),
        peer(
            "guest",
            "YAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=",
            "10.88.0.3",
            "[\"nas\"]"
        )
    );
    assert!(messages(&ok).is_empty(), "{:?}", messages(&ok));
    let state = DesiredState::from_toml(&ok).unwrap();
    assert_eq!(state.vpn.free_address(), Some("10.88.0.4".parse().unwrap()));
    assert_eq!(DesiredState::from_toml(&state.to_toml()).unwrap(), state);

    assert_problem(
        &format!("{base}{}", peer("Phone", KEY, "10.88.0.2", "\"all\"")),
        "invalid name",
    );
    assert_problem(
        &format!("{base}{}", peer("phone", "short", "10.88.0.2", "\"all\"")),
        "not a WireGuard key",
    );
    assert_problem(
        &format!("{base}{}", peer("phone", KEY, "10.88.0.1", "\"all\"")),
        "belongs to this machine",
    );
    assert_problem(
        &format!("{base}{}", peer("phone", KEY, "10.99.0.2", "\"all\"")),
        "not a host address",
    );
    assert_problem(
        &format!("{base}{}", peer("phone", KEY, "10.88.0.2", "[\"nope\"]")),
        "no service is called `nope`",
    );
    assert_problem(
        &format!(
            "{base}{}{}",
            peer("a", KEY, "10.88.0.2", "\"all\""),
            peer("b", KEY, "10.88.0.2", "\"all\"")
        ),
        "same public key",
    );
    assert_problem("[vpn]\nsubnet = \"8.8.0.0/24\"\n", "private network");
    assert_problem("[vpn]\nsubnet = \"10.0.0.0/8\"\n", "/16 to /30");
    assert_problem("[vpn]\nendpoint = \"bad host!\"\n", "vpn.endpoint");
    assert_problem("[dns]\nupstream = [\"dns.google\"]\n", "dns.upstream");
    assert_problem(
        "[[interfaces]]\nname = \"q\"\nlisteners = [{ port = 51820, mode = \"raw\", protocol = \"udp\" }]\n[vpn]\nenabled = true\n",
        "used by a public interface",
    );
}
