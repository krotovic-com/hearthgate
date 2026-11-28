//! Ask AI: a conversation about the screen the user has open, which can
//! walk them through a setup one proposed change at a time.
//!
//! The conversation lives in the dashboard; every request carries it in
//! full, so nothing is stored here. As with reviews, all material is
//! redacted before it is sent, the answer is mapped back locally, and a
//! proposed change only ever reaches the configuration when the user
//! applies it.

use std::fmt::Write as _;

use hearthgate_model::DesiredState;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::review::{RawFix, json_object, propose, read_fix};
use crate::{Error, Fix, PatchOp, Prompt, Redactor};

/// Turns kept from the conversation, newest last. Older ones are dropped.
pub const MAX_TURNS: usize = 40;
/// Characters kept from one turn.
pub const MAX_TURN_CHARS: usize = 8_000;

/// Who wrote a turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    User,
    Assistant,
    /// Something the dashboard noted: the user opened another screen,
    /// opened a dialog, or applied or skipped a proposal.
    Context,
}

/// One turn of the conversation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChatTurn {
    pub role: ChatRole,
    pub text: String,
}

/// One field of an open dialog, as the user sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DialogField {
    pub label: String,
    pub value: String,
}

/// An open dialog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChatDialog {
    pub title: String,
    #[serde(default)]
    pub fields: Vec<DialogField>,
}

/// What the user has in front of them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChatContext {
    /// The screen's title, such as `Services`.
    #[serde(default)]
    pub screen: String,
    /// The dashboard path, such as `/services/minecraft`.
    #[serde(default)]
    pub path: String,
    /// The selected item, such as a service name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selection: Option<String>,
    /// The dialog open on top of the screen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog: Option<ChatDialog>,
}

/// A change the assistant proposes. It changes nothing until the user
/// applies it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Proposal {
    /// `container`, `stack`, `service`, `interface`, `route`, `firewall`,
    /// `vpn`, `settings` or `other`.
    pub kind: String,
    pub title: String,
    /// What it does and why, in a sentence or two.
    pub summary: String,
    /// Its step in the plan, from 1, or 0 when it isn't part of one.
    #[serde(default)]
    pub step: u32,
    pub changes: Vec<PatchOp>,
}

impl Proposal {
    #[must_use]
    pub fn fix(&self) -> Fix {
        Fix {
            summary: self.title.clone(),
            changes: self.changes.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PlanStep {
    pub title: String,
    /// The screen it changes and a detail, such as `Routes · photos.example.com`.
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub done: bool,
}

/// A multi-step setup the assistant is guiding the user through.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Plan {
    pub title: String,
    pub steps: Vec<PlanStep>,
}

/// The assistant's answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChatReply {
    pub reply: String,
    /// Changes to review, in the order to apply them.
    pub proposals: Vec<Proposal>,
    /// The plan, when a multi-step setup is under way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<Plan>,
    /// Short follow-up questions the user might ask next.
    #[serde(default)]
    pub suggestions: Vec<String>,
}

const SYSTEM: &str = "You are the assistant inside the admin dashboard of Hearthgate, a \
self-hosted gateway that is the only internet entry point of a home server. It has public \
interfaces (domains with automatic TLS certificates, and TCP or UDP ports), internal services \
(local ports or managed Docker containers and Compose stacks), routes from interface listeners \
to services with load balancing, an nftables firewall with blocklists and bans, UPnP port \
forwarding, a WireGuard VPN with internal DNS, and the dashboard itself, reachable only from \
the LAN or VPN.

The dashboard's screens: Overview (/), Public interfaces (/interfaces), Services (/services), \
Containers (/containers, managed containers and stacks), Routes (/routes), VPN peers (/vpn), \
Firewall (/firewall), AI review (/ai), History (/history, every applied revision, with \
rollback) and Settings (/settings).

You get the conversation so far, what the user has open (the screen, the selected item and any \
open dialog with its fields), the desired configuration as JSON, the JSON schema of that \
configuration, and what the gateway sees now. Lines marked [context] are notes from the \
dashboard, not from the user: they say when the user moved to another screen, opened a dialog, \
or applied or skipped one of your proposals.

Data was redacted before it reached you: text such as <PUBLIC_IP_1>, <PRIVATE_IP_2>, \
<DOMAIN_1> or <SECRET_3> stands for a real value, also in the user's own messages. The same \
placeholder always means the same value. Keep placeholders exactly as written when you mention \
or reuse those values, and never guess what they hide.

Answer the user's question about what they see, or help them do what they ask. When they want \
something set up that takes several changes (for example a new service from start to finish: \
a container, the internal service, a public interface, a route and a firewall rule), give a \
plan of short steps and propose the changes one step at a time: propose the next step, wait \
until the user applies it, then propose the following one. Keep the plan in every answer while \
the setup is under way, marking the steps that were applied as done. Leave the plan's title \
empty and its steps empty when there is no setup under way.

A proposal changes the desired configuration with JSON Patch operations (add, remove or \
replace) and JSON Pointer paths into the configuration you were given, such as /routes/- to \
add a route. Give each value as JSON text in value_json (use \"null\" for remove), following \
the schema. Each proposal must stand on its own against the current configuration. Prefer \
small proposals that do one thing, set kind to the part of the configuration it changes, and \
number it with its plan step (0 when there is no plan). Only propose changes you are confident \
in; ask when something important is unclear, such as a domain name or which image to run. \
The user reviews and applies every change themselves, and can roll it back in History. Never \
claim that something was changed: only the user applies changes.

Write plainly and briefly for someone who runs a home server. Offer up to three short \
follow-up questions the user might ask next. Answer with JSON only.";

/// The JSON schema the answer must follow.
#[must_use]
pub fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["reply", "proposals", "plan", "suggestions"],
        "properties": {
            "reply": {"type": "string"},
            "proposals": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["kind", "title", "summary", "step", "changes"],
                    "properties": {
                        "kind": {"type": "string", "enum": [
                            "container", "stack", "service", "interface", "route",
                            "firewall", "vpn", "settings", "other"
                        ]},
                        "title": {"type": "string"},
                        "summary": {"type": "string"},
                        "step": {"type": "integer"},
                        "changes": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "additionalProperties": false,
                                "required": ["op", "path", "value_json"],
                                "properties": {
                                    "op": {"type": "string", "enum": ["add", "remove", "replace"]},
                                    "path": {"type": "string"},
                                    "value_json": {"type": "string"}
                                }
                            }
                        }
                    }
                }
            },
            "plan": {
                "type": "object",
                "additionalProperties": false,
                "required": ["title", "steps"],
                "properties": {
                    "title": {"type": "string"},
                    "steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["title", "detail", "done"],
                            "properties": {
                                "title": {"type": "string"},
                                "detail": {"type": "string"},
                                "done": {"type": "boolean"}
                            }
                        }
                    }
                }
            },
            "suggestions": {"type": "array", "items": {"type": "string"}}
        }
    })
}

fn clip(text: &str) -> &str {
    match text.char_indices().nth(MAX_TURN_CHARS) {
        Some((at, _)) => &text[..at],
        None => text,
    }
}

/// The question: the conversation, which is redacted here, and
/// `material`, which must be redacted already. `config_schema` is the
/// configuration's JSON schema; it holds no user data.
#[must_use]
pub fn prompt(
    turns: &[ChatTurn],
    material: &Value,
    config_schema: &Value,
    redactor: &mut Redactor,
) -> Prompt {
    let skip = turns.len().saturating_sub(MAX_TURNS);
    let mut conversation = String::new();
    for t in &turns[skip..] {
        let who = match t.role {
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
            ChatRole::Context => "context",
        };
        let _ = write!(
            conversation,
            "[{who}] {}\n\n",
            redactor.redact(clip(&t.text))
        );
    }
    let material = serde_json::to_string_pretty(material).unwrap_or_default();
    let config_schema = serde_json::to_string(config_schema).unwrap_or_default();
    Prompt {
        system: SYSTEM.to_owned(),
        user: format!(
            "Conversation so far, oldest first. Answer the last user message.\n\n\
             {conversation}Material:\n{material}\n\nSchema of the configuration:\n{config_schema}"
        ),
        schema: schema(),
    }
}

#[derive(Deserialize)]
struct RawReply {
    #[serde(default)]
    reply: String,
    #[serde(default)]
    proposals: Vec<RawProposal>,
    #[serde(default)]
    plan: Option<Plan>,
    #[serde(default)]
    suggestions: Vec<String>,
}

#[derive(Deserialize)]
struct RawProposal {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    step: u32,
    #[serde(default)]
    changes: Vec<crate::review::RawChange>,
}

const KINDS: [&str; 9] = [
    "container",
    "stack",
    "service",
    "interface",
    "route",
    "firewall",
    "vpn",
    "settings",
    "other",
];

/// Reads an answer and maps placeholders back to real values. Proposals
/// that don't apply to `base` are dropped, and the reply says so.
///
/// # Errors
/// When the answer is not the requested JSON.
pub fn parse(answer: &str, redactor: &Redactor, base: &DesiredState) -> Result<ChatReply, Error> {
    let body = json_object(answer).ok_or_else(|| Error::Answer("no JSON object in it".into()))?;
    let raw: RawReply = serde_json::from_str(body).map_err(|e| Error::Answer(e.to_string()))?;
    let mut reply = redactor.unredact(raw.reply.trim());
    let mut proposals = Vec::new();
    let mut dropped = Vec::new();
    for p in raw.proposals {
        let title = redactor.unredact(p.title.trim());
        let fix = read_fix(
            RawFix {
                summary: title.clone(),
                changes: p.changes,
            },
            redactor,
        );
        let Some(fix) = fix else {
            dropped.push(title);
            continue;
        };
        if let Err(why) = propose(base, &fix) {
            tracing::debug!("Ask AI proposal {title:?} dropped: {why}");
            dropped.push(title);
            continue;
        }
        let kind = p.kind.trim().to_ascii_lowercase();
        proposals.push(Proposal {
            kind: if KINDS.contains(&kind.as_str()) {
                kind
            } else {
                "other".into()
            },
            title,
            summary: redactor.unredact(p.summary.trim()),
            step: p.step,
            changes: fix.changes,
        });
    }
    if !dropped.is_empty() {
        let (what, them) = if dropped.len() == 1 {
            ("a change that isn't", "it")
        } else {
            ("changes that aren't", "them")
        };
        let _ = write!(
            reply,
            "\n\n(I suggested {what} valid for this configuration, so I left {them} out: {}. \
             Ask me to try again.)",
            dropped.join("; ")
        );
    }
    let plan = raw.plan.filter(|p| !p.steps.is_empty()).map(|p| Plan {
        title: redactor.unredact(p.title.trim()),
        steps: p
            .steps
            .into_iter()
            .map(|s| PlanStep {
                title: redactor.unredact(s.title.trim()),
                detail: redactor.unredact(s.detail.trim()),
                done: s.done,
            })
            .collect(),
    });
    let suggestions = raw
        .suggestions
        .iter()
        .map(|s| redactor.unredact(s.trim()))
        .filter(|s| !s.is_empty())
        .take(3)
        .collect();
    Ok(ChatReply {
        reply,
        proposals,
        plan,
        suggestions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn turn(role: ChatRole, text: &str) -> ChatTurn {
        ChatTurn {
            role,
            text: text.into(),
        }
    }

    #[test]
    fn prompt_redacts_the_conversation() {
        let mut r = Redactor::new(&[]).unwrap();
        let turns = vec![
            turn(ChatRole::User, "Set up immich at photos.example.com"),
            turn(ChatRole::Context, "You opened Public interfaces"),
        ];
        let p = prompt(&turns, &json!({"a": 1}), &json!({}), &mut r);
        assert!(!p.user.contains("example.com"));
        assert!(p.user.contains("[user] Set up immich at photos.<DOMAIN_1>"));
        assert!(p.user.contains("[context] You opened Public interfaces"));
    }

    #[test]
    fn keeps_only_recent_turns() {
        let mut r = Redactor::new(&[]).unwrap();
        let turns: Vec<_> = (0..MAX_TURNS + 5)
            .map(|i| turn(ChatRole::User, &format!("message number {i}")))
            .collect();
        let p = prompt(&turns, &json!({}), &json!({}), &mut r);
        assert!(!p.user.contains("message number 4\n"));
        assert!(
            p.user
                .contains(&format!("message number {}", MAX_TURNS + 4))
        );
    }

    #[test]
    fn reads_proposals_and_drops_invalid_ones() {
        let mut r = Redactor::new(&[]).unwrap();
        let domain = r.redact("photos.example.com");
        let answer = json!({
            "reply": format!("Here is step 3 for {domain}."),
            "proposals": [
                {"kind": "router", "title": format!("Forward ports for {domain}"), "summary": "s", "step": 3,
                 "changes": [{"op": "add", "path": "/router/upnp", "value_json": "true"}]},
                {"kind": "route", "title": "Broken", "summary": "", "step": 4,
                 "changes": [{"op": "replace", "path": "/routes/7", "value_json": "{}"}]},
                {"kind": "route", "title": "Unreadable", "summary": "", "step": 4,
                 "changes": [{"op": "add", "path": "/routes/-", "value_json": "{nope"}]}
            ],
            "plan": {"title": format!("Immich at {domain}"), "steps": [
                {"title": "Container", "detail": "Containers", "done": true},
                {"title": "Interface", "detail": "Public interfaces", "done": false}
            ]},
            "suggestions": ["a", " ", "b", "c", "d"]
        })
        .to_string();
        let out = parse(&answer, &r, &DesiredState::default()).unwrap();
        assert_eq!(out.proposals.len(), 1);
        let p = &out.proposals[0];
        assert_eq!(p.kind, "other");
        assert_eq!(p.title, "Forward ports for photos.example.com");
        assert_eq!(p.step, 3);
        assert!(
            out.reply
                .starts_with("Here is step 3 for photos.example.com.")
        );
        assert!(out.reply.contains("Broken; Unreadable"));
        let plan = out.plan.unwrap();
        assert_eq!(plan.title, "Immich at photos.example.com");
        assert!(plan.steps[0].done);
        assert_eq!(out.suggestions, vec!["a", "b", "c"]);
    }

    #[test]
    fn an_empty_plan_is_no_plan() {
        let r = Redactor::new(&[]).unwrap();
        let answer =
            r#"{"reply":"Hi","proposals":[],"plan":{"title":"","steps":[]},"suggestions":[]}"#;
        let out = parse(answer, &r, &DesiredState::default()).unwrap();
        assert!(out.plan.is_none());
        assert_eq!(out.proposals.len(), 0);
    }
}
