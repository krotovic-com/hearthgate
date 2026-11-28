//! Hearthgate's optional AI assistant.
//!
//! Nothing here talks to a provider unless AI is enabled: [`Client::new`]
//! refuses disabled settings, and it is the only way to get a client.
//! Everything sent goes through a [`Redactor`] first, so addresses, domains
//! and secrets leave the machine as placeholders, and answers are mapped
//! back locally.

pub mod chat;
pub mod patch;
pub mod provider;
pub mod redact;
pub mod review;

pub use chat::{ChatContext, ChatReply, ChatRole, ChatTurn, Plan, PlanStep, Proposal};
pub use patch::{PatchError, PatchOp};
pub use provider::{Client, Outgoing, Prompt};
pub use redact::Redactor;
pub use review::{Finding, Fix, Review, Scope, Severity};

/// Why the assistant could not answer.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// AI is turned off in settings.
    #[error("the AI assistant is turned off")]
    Disabled,
    /// The settings are incomplete.
    #[error("the AI assistant isn't set up: {0}")]
    Config(String),
    /// The provider failed or declined.
    #[error("the AI provider {0}")]
    Provider(String),
    /// The answer didn't follow the requested format.
    #[error("the AI answer couldn't be read: {0}")]
    Answer(String),
}
