//! Managed containers and stacks against a real Docker. Skipped when Docker
//! isn't reachable, unless HEARTHGATE_DOCKER_TESTS is set.

use std::path::Path;
use std::process::Command;
use std::time::Duration;

use hearthgate_docker::{Manager, Owner};
use hearthgate_model::DesiredState;

const SOCKET: &str = "/var/run/docker.sock";
const IMAGE: &str = "busybox:1.36";

fn docker(args: &[&str]) -> bool {
    Command::new("docker")
        .args(args)
        .output()
        .is_ok_and(|o| o.status.success())
}

fn available() -> bool {
    let ok = Path::new(SOCKET).exists()
        && docker(&["info"])
        && (docker(&["image", "inspect", IMAGE]) || docker(&["pull", IMAGE]));
    if !ok {
        assert!(
            std::env::var_os("HEARTHGATE_DOCKER_TESTS").is_none(),
            "HEARTHGATE_DOCKER_TESTS is set, but Docker or {IMAGE} is not available"
        );
        eprintln!("skipping: Docker or {IMAGE} is not available");
    }
    ok
}

struct Cleanup(Vec<String>);
impl Drop for Cleanup {
    fn drop(&mut self) {
        for args in &self.0 {
            let args: Vec<&str> = args.split(' ').collect();
            let _ = Command::new("docker").args(&args).output();
        }
    }
}

fn state(text: &str) -> DesiredState {
    DesiredState::from_toml(text).unwrap()
}

#[tokio::test]
async fn runs_containers_and_stacks() {
    if !available() {
        return;
    }
    let id = std::process::id();
    let web = format!("hgtest-web-{id}");
    let squatter = format!("hgtest-other-{id}");
    let stack = format!("hgtest-{id}");
    let dir = tempfile::tempdir().unwrap();
    let manager = Manager::new(Some(Path::new(SOCKET)), dir.path());
    // Clean up whatever happens.
    let _cleanup = Cleanup(vec![
        format!("rm -f {web} {squatter}"),
        format!("compose -p {stack} down --remove-orphans"),
    ]);

    let container = |env: &str, running: bool| {
        format!(
            r#"
            [[containers]]
            name = "{web}"
            image = "{IMAGE}"
            command = ["sh", "-c", "echo started {env}; exec httpd -f -p 8080"]
            env = {{ GREETING = "{env}" }}
            running = {running}
            "#
        )
    };
    let compose =
        format!("services:\n  app:\n    image: {IMAGE}\n    command: [\"sleep\", \"3600\"]\n");
    let with_stack = |extra: &str| {
        format!("{extra}\n[[stacks]]\nname = \"{stack}\"\ncompose = '''\n{compose}'''\n")
    };

    // Created, started, on the hearthgate network and reachable from here.
    let desired = state(&with_stack(&container("one", true)));
    let problems = manager.reconcile(&desired).await;
    assert!(problems.is_empty(), "{problems:?}");
    let status = manager.status();
    assert!(status.available);
    let info = status.containers.iter().find(|c| c.name == web).unwrap();
    assert_eq!(info.state, "running", "{info:?}");
    assert_eq!(info.owner, Owner::Container);
    let address = info.address.expect("an address on the hearthgate network");
    let mut reached = false;
    for _ in 0..20 {
        if tokio::net::TcpStream::connect((address, 8080))
            .await
            .is_ok()
        {
            reached = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(reached, "cannot reach {address}:8080");
    assert_eq!(manager.addresses()[&web], address);

    // The stack is up, and its container joined the network too.
    let stack_info = status.stacks.iter().find(|s| s.name == stack).unwrap();
    assert_eq!(
        (stack_info.running, stack_info.total),
        (1, 1),
        "{stack_info:?}"
    );
    let app = status
        .containers
        .iter()
        .find(|c| matches!(&c.owner, Owner::Stack { stack: s, service } if *s == stack && service == "app"))
        .expect("the stack's container");
    assert!(app.address.is_some(), "{app:?}");

    // Logs and stats.
    let logs = manager.logs(&web, 50, Some("started")).await.unwrap();
    assert!(logs.iter().any(|l| l.text == "started one"), "{logs:?}");
    let stats = manager.stats(&web).await.unwrap();
    assert!(stats.memory_bytes > 0);
    assert!(manager.logs("not-managed", 5, None).await.is_err());

    // Reconciling again changes nothing; a new setting recreates it.
    let created = info.created;
    manager.reconcile(&desired).await;
    let again = manager.status();
    assert_eq!(
        again
            .containers
            .iter()
            .find(|c| c.name == web)
            .unwrap()
            .created,
        created
    );
    let desired = state(&with_stack(&container("two", true)));
    let problems = manager.reconcile(&desired).await;
    assert!(problems.is_empty(), "{problems:?}");
    let logs = manager.logs(&web, 50, None).await.unwrap();
    assert!(logs.iter().any(|l| l.text == "started two"), "{logs:?}");

    // Stopped on request, and kept.
    let desired = state(&with_stack(&container("two", false)));
    manager.reconcile(&desired).await;
    let info = manager.status();
    let info = info.containers.iter().find(|c| c.name == web).unwrap();
    assert_ne!(info.state, "running", "{info:?}");

    // A container Hearthgate didn't create is never touched.
    assert!(docker(&["create", "--name", &squatter, IMAGE, "true"]));
    let clash = state(&format!(
        "[[containers]]\nname = \"{squatter}\"\nimage = \"{IMAGE}\"\n"
    ));
    let problems = manager.reconcile(&clash).await;
    assert!(
        problems.iter().any(|p| p.contains("does not manage")),
        "{problems:?}"
    );
    assert!(docker(&["inspect", &squatter]));

    // Removing them from the state removes the containers and the stack.
    let problems = manager.reconcile(&DesiredState::default()).await;
    assert!(problems.is_empty(), "{problems:?}");
    assert!(!docker(&["inspect", &web]));
    assert!(!dir.path().join("stacks").join(&stack).exists());
    let leftover = Command::new("docker")
        .args([
            "ps",
            "-aq",
            "--filter",
            &format!("label=com.docker.compose.project={stack}"),
        ])
        .output()
        .unwrap();
    assert!(leftover.stdout.is_empty(), "{leftover:?}");
}
