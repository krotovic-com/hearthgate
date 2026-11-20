//! The built-in VPN: a WireGuard interface for remote devices, the
//! configuration files and QR codes they import, and the internal DNS zone
//! that names every service.

pub mod client;
pub mod device;
pub mod dns;
pub mod keys;
pub mod plan;
pub mod runtime;

pub use client::{ClientConfig, qr_svg, qr_terminal};
pub use device::{Backend, Device, PeerConfig, PeerStatus, Setup, Tools};
pub use keys::KeyPair;
pub use plan::{Plan, client_routes, endpoint, service_addresses};
pub use runtime::{Info, Runtime, load_or_create_key};

/// Name of the WireGuard interface Hearthgate creates.
pub const INTERFACE: &str = "hearthgate";
