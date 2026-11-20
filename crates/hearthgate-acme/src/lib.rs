//! Certificates for Hearthgate's `https` interfaces: storage on disk,
//! manual upload, and (from M1.4) automatic issuance and renewal with ACME.

pub mod acme;
pub mod store;

pub use acme::{CertJob, Issuer, Outcome, jobs};
pub use store::{CertError, CertInfo, CertSource, CertStore};
