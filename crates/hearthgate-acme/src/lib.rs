//! Certificates for Hearthgate's `https` interfaces: storage on disk,
//! manual upload, and (from M1.4) automatic issuance and renewal with ACME.

pub mod store;

pub use store::{CertError, CertInfo, CertSource, CertStore};
