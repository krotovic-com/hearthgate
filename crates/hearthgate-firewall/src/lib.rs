//! Hearthgate's perimeter: an nftables firewall in its own table, blocklist
//! feeds and automatic bans.
//!
//! [`ruleset::render`] turns the desired state into a complete table, which
//! [`Nft::apply`] installs in one transaction. Nothing outside the
//! `inet hearthgate` table is ever changed, so rules managed by the user,
//! Docker or other tools stay as they are.

pub mod bans;
pub mod feeds;
pub mod nft;
pub mod perimeter;
pub mod ruleset;

pub use nft::{Dropped, Nft, NftError, RecentDrop};
pub use perimeter::{CONFIRM_WINDOW, ConfirmError, Pending, Perimeter, Status};
pub use ruleset::{Dynamic, Inputs, OpenPort, Reason};
