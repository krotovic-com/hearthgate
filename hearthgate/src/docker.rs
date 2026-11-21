//! Keeps managed containers and stacks in line with the desired state, in
//! the background: pulling an image can take minutes, and the gateway
//! should not wait for it.

use std::sync::Arc;
use std::time::Duration;

use hearthgate_docker::Manager;
use hearthgate_model::DesiredState;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// How often Docker is checked against the desired state, to recreate
/// containers removed by hand and rejoin recreated stack containers.
const RECHECK: Duration = Duration::from_secs(30);

pub async fn run(
    manager: Arc<Manager>,
    mut state: watch::Receiver<Option<Arc<DesiredState>>>,
    stop: CancellationToken,
) {
    loop {
        // Nothing happens before the first revision is known, so a start
        // never removes containers.
        let current = state.borrow_and_update().clone();
        let Some(current) = current else {
            tokio::select! {
                () = stop.cancelled() => return,
                r = state.changed() => if r.is_err() { return },
            }
            continue;
        };
        tokio::select! {
            () = stop.cancelled() => return,
            _ = manager.reconcile(&current) => {}
        }
        tokio::select! {
            () = stop.cancelled() => return,
            r = state.changed() => if r.is_err() { return },
            () = tokio::time::sleep(RECHECK) => {}
        }
    }
}
