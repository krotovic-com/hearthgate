//! Internal services: active health checks and read-only Docker discovery.

pub mod docker;
pub mod health;

pub use docker::Containers;
pub use health::{HealthChecker, HealthJob};
