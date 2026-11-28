//! Updates of Hearthgate itself.
//!
//! Two halves that trust each other as little as possible:
//!
//! - The daemon (unprivileged, facing the internet) checks GitHub for new
//!   releases, downloads the one asset this machine needs, verifies its
//!   signature, and asks for an install by writing a request file. See
//!   [`Updater`].
//! - The root helper (`hearthgate upgrade apply`, started by a systemd path
//!   unit, with no network) copies the files out of the daemon's reach,
//!   verifies them again from scratch, backs up what runs now, installs,
//!   restarts Hearthgate and rolls back if the new version is not healthy.
//!   See [`apply`].
//!
//! Nothing is installed unless `SHA256SUMS` carries a valid signature from a
//! key compiled into this binary ([`keys`]) and names the exact version.

pub mod apply;
pub mod fetch;
pub mod files;
pub mod github;
pub mod keys;
pub mod method;
pub mod notes;
pub mod policy;
pub mod state;
mod updater;
pub mod verify;
pub mod version;

pub use updater::{UpdateInfo, Updater};
pub use version::{Bump, Version};

/// The repository releases come from. Never configurable.
pub const REPO: &str = "krotovic-com/hearthgate";
