//! Hearthgate's data plane.
//!
//! [`plan::compile`] turns a validated desired state into a [`Plan`]: the
//! sockets to listen on and what each one does. [`Gateway::apply`] moves the
//! running sockets to a new plan without a restart, keeping open connections
//! on sockets that did not change.

pub mod gateway;
pub mod l4;
pub mod plan;
pub mod pool;

pub use gateway::{BindError, Gateway, SocketStatus};
pub use plan::{ContainerLookup, NoContainers, Plan, SocketKey, SocketKind, Transport, compile};
pub use pool::{Endpoint, Pool, UpstreamState, Upstreams};
