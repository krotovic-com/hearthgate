use std::collections::HashMap;

use hearthgate_model::AutoInstall;

use super::*;
use crate::fetch::Download;
use crate::verify::testkey::TestKey;

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

#[derive(Default)]
struct FakeFetch {
    files: Mutex<HashMap<String, Vec<u8>>>,
    asked: Mutex<Vec<String>>,
}

impl FakeFetch {
    fn put(&self, url: String, data: Vec<u8>) {
        self.files.lock().unwrap().insert(url, data);
    }
}

impl Fetch for FakeFetch {
    fn get(&self, url: &str, max: u64) -> Download<'_> {
        self.asked.lock().unwrap().push(url.to_owned());
        let r = match self.files.lock().unwrap().get(url) {
            Some(d) if d.len() as u64 > max => Err("too large".to_owned()),
            Some(d) => Ok(d.clone()),
            None => Err("404".to_owned()),
        };
        Box::pin(async move { r })
    }
}

const DEB: &str = "hearthgate_1.3.0_amd64.deb";

struct Setup {
    _dir: tempfile::TempDir,
    fetch: Arc<FakeFetch>,
    updater: Updater,
    key: TestKey,
}

fn setup(current: &str, method: Method, settings: Updates) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout {
        data_dir: dir.path().join("data"),
        helper_dir: dir.path().join("helper"),
    };
    std::fs::create_dir_all(&layout.data_dir).unwrap();
    std::fs::create_dir_all(&layout.helper_dir).unwrap();
    let fetch = Arc::new(FakeFetch::default());
    let key = TestKey::new();
    fetch.put(
        github::releases_url(),
        br####"[
          {"tag_name":"v1.3.0","published_at":"2026-11-26T10:00:00Z",
           "body":"### Features\n\n- Check for updates from the dashboard\n\n### Bug fixes\n\n- Fix a crash","assets":[{"name":"hearthgate_1.3.0_amd64.deb"},{"name":"SHA256SUMS"}]},
          {"tag_name":"v1.2.1","published_at":"2026-11-21T10:00:00Z","body":"### Bug fixes\n\n- Fix the firewall","assets":[]},
          {"tag_name":"v1.2.0","published_at":"2026-11-20T10:00:00Z","body":"### Features\n\n- Old","assets":[]}
        ]"####
        .to_vec(),
    );
    let (sums, sig) = key.release("1.3.0", &[(DEB, b"package")]);
    fetch.put(github::asset_url(v("1.3.0"), "SHA256SUMS"), sums);
    fetch.put(
        github::asset_url(v("1.3.0"), "SHA256SUMS.minisig"),
        sig.into_bytes(),
    );
    fetch.put(github::asset_url(v("1.3.0"), DEB), b"package".to_vec());
    let updater = Updater::new(Config {
        layout,
        current: current.into(),
        method,
        helper: true,
        arch: Arch::from_name("x86_64"),
        fetch: fetch.clone(),
        keys: vec![key.public()],
        settings,
    });
    Setup {
        _dir: dir,
        fetch,
        updater,
        key,
    }
}

#[tokio::test]
async fn finds_the_newest_release_and_its_notes() {
    let s = setup("1.2.0", Method::Deb, Updates::default());
    let info = s.updater.check().await.unwrap();
    let a = info.available.unwrap();
    assert_eq!(a.version, v("1.3.0"));
    assert_eq!(a.bump, Bump::Minor);
    assert_eq!(a.asset.as_deref(), Some(DEB));
    let texts: Vec<&str> = a.notes.iter().map(|n| n.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "Check for updates from the dashboard",
            "Fix a crash",
            "Fix the firewall"
        ]
    );
    assert!(info.can_install);

    let s = setup("1.3.0", Method::Deb, Updates::default());
    assert!(s.updater.check().await.unwrap().available.is_none());
}

#[tokio::test]
async fn install_downloads_verifies_and_asks_the_helper() {
    let s = setup("1.2.0", Method::Deb, Updates::default());
    s.updater.check().await.unwrap();
    s.updater.install(v("1.3.0"), true).await.unwrap();
    let layout = s.updater.layout();
    let request: Request = read_json(&layout.request()).unwrap();
    assert_eq!(
        request,
        Request::Install {
            version: v("1.3.0")
        }
    );
    let dir = layout.download_dir(v("1.3.0"));
    assert_eq!(std::fs::read(dir.join(DEB)).unwrap(), b"package");
    // A second one waits for the first.
    assert!(
        s.updater
            .install(v("1.3.0"), true)
            .await
            .unwrap_err()
            .contains("already")
    );
}

#[tokio::test]
async fn refuses_a_bad_signature_or_a_changed_file() {
    let s = setup("1.2.0", Method::Deb, Updates::default());
    s.updater.check().await.unwrap();
    s.fetch
        .put(github::asset_url(v("1.3.0"), DEB), b"evil".to_vec());
    assert!(s.updater.install(v("1.3.0"), true).await.is_err());
    assert!(!s.updater.layout().request().exists());

    let other = TestKey::new();
    let (sums, sig) = other.release("1.3.0", &[(DEB, b"evil")]);
    s.fetch
        .put(github::asset_url(v("1.3.0"), "SHA256SUMS"), sums);
    s.fetch.put(
        github::asset_url(v("1.3.0"), "SHA256SUMS.minisig"),
        sig.into_bytes(),
    );
    assert!(s.updater.install(v("1.3.0"), true).await.is_err());
    assert!(!s.updater.layout().request().exists());
    let _ = &s.key;
}

#[tokio::test]
async fn manual_installs_and_dev_builds_only_show_updates() {
    let s = setup("1.2.0", Method::Manual, Updates::default());
    let info = s.updater.check().await.unwrap();
    assert!(info.available.is_some());
    assert!(!info.can_install);
    assert!(s.updater.install(v("1.3.0"), true).await.is_err());

    let s = setup("0.0.0-dev", Method::Deb, Updates::default());
    assert!(!s.updater.info().can_install);
}

#[tokio::test]
async fn installs_by_itself_only_when_allowed_and_in_the_window() {
    let noon = 12 * 60;
    let four = 4 * 60;
    let quiet = |_: &str, _: &Available| {};

    // Off: never.
    let s = setup("1.2.0", Method::Deb, Updates::default());
    s.updater.tick(four, &quiet).await;
    assert!(!s.updater.layout().request().exists());

    // Patch only: a minor release waits for the person.
    let s = setup(
        "1.2.0",
        Method::Deb,
        Updates {
            auto_install: AutoInstall::Patch,
            ..Updates::default()
        },
    );
    s.updater.tick(four, &quiet).await;
    assert!(!s.updater.layout().request().exists());

    // Minor: outside the window nothing happens, inside it installs.
    let s = setup(
        "1.2.0",
        Method::Deb,
        Updates {
            auto_install: AutoInstall::Minor,
            ..Updates::default()
        },
    );
    s.updater.tick(noon, &quiet).await;
    assert!(!s.updater.layout().request().exists());
    s.updater.tick(four, &quiet).await;
    assert!(s.updater.layout().request().exists());
}

#[tokio::test]
async fn a_scheduled_install_waits_for_the_window() {
    let s = setup("1.2.0", Method::Deb, Updates::default());
    s.updater.check().await.unwrap();
    let info = s.updater.install(v("1.3.0"), false).await.unwrap();
    assert_eq!(info.scheduled, Some(v("1.3.0")));
    assert!(!s.updater.layout().request().exists());
    s.updater.tick(12 * 60, &|_, _| {}).await;
    assert!(!s.updater.layout().request().exists());
    s.updater.tick(3 * 60 + 30, &|_, _| {}).await;
    assert!(s.updater.layout().request().exists());
    assert_eq!(s.updater.info().scheduled, None);
}

#[tokio::test]
async fn notifies_once_and_not_for_skipped() {
    let s = setup("1.2.0", Method::Deb, Updates::default());
    let count = Arc::new(Mutex::new(0));
    let c = count.clone();
    let notify = move |_: &str, _: &Available| *c.lock().unwrap() += 1;
    s.updater.tick(12 * 60, &notify).await;
    s.updater.tick(12 * 60, &notify).await;
    assert_eq!(*count.lock().unwrap(), 1);

    let s = setup("1.2.0", Method::Deb, Updates::default());
    s.updater.skip(v("1.3.0"));
    s.updater.tick(12 * 60, &notify).await;
    assert_eq!(*count.lock().unwrap(), 1);
}

#[tokio::test]
async fn a_rolled_back_version_is_skipped() {
    let s = setup(
        "1.2.0",
        Method::Deb,
        Updates {
            auto_install: AutoInstall::Minor,
            ..Updates::default()
        },
    );
    s.updater.check().await.unwrap();
    let job = Job {
        action: crate::state::Action::Install,
        from: v("1.2.0"),
        to: Some(v("1.3.0")),
        steps: vec![],
        outcome: Outcome::RolledBack,
        detail: None,
        started_unix: now_unix(),
        finished_unix: Some(now_unix()),
        log: vec![],
    };
    write_json(&s.updater.layout().history(), &vec![job], 0o644).unwrap();
    assert_eq!(s.updater.info().skipped, vec![v("1.3.0")]);
    s.updater.tick(4 * 60, &|_, _| {}).await;
    assert!(!s.updater.layout().request().exists());
}

#[tokio::test]
async fn rollback_needs_a_kept_version() {
    let s = setup("1.3.0", Method::Deb, Updates::default());
    assert!(s.updater.rollback().is_err());
    let kept = Backup {
        version: v("1.2.0"),
        method: Method::Deb,
        package: None,
        taken_unix: 1,
    };
    write_json(&s.updater.layout().kept(), &kept, 0o644).unwrap();
    assert_eq!(s.updater.info().rollback_to, Some(v("1.2.0")));
    s.updater.rollback().unwrap();
    let request: Request = read_json(&s.updater.layout().request()).unwrap();
    assert_eq!(request, Request::Rollback);
}
