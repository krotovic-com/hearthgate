//! Reviews and troubleshooting: what to ask, and how to read the answer.

use std::time::{SystemTime, UNIX_EPOCH};

use hearthgate_model::{AiProvider, DesiredState};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::patch::{self, Op, PatchOp};
use crate::{Error, Prompt, Redactor};

/// How much a finding matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    High,
    Medium,
    Low,
}

/// What a review looks at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Scope {
    /// The whole configuration and what is happening now.
    All,
    /// Firewall rules, bans and recent drops.
    Firewall,
    /// Why one service is unhealthy.
    Service { service: String },
}

/// A suggested change, as a patch to the desired state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Fix {
    pub summary: String,
    pub changes: Vec<PatchOp>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Finding {
    pub severity: Severity,
    /// The part of Hearthgate it concerns, such as `firewall` or a service.
    pub area: String,
    pub title: String,
    pub detail: String,
    /// A change that would address it, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
}

/// A finished review.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Review {
    pub id: String,
    /// When it finished (RFC 3339).
    pub created: String,
    /// The same, in seconds since the Unix epoch.
    #[serde(default)]
    pub created_unix: u64,
    pub scope: Scope,
    pub provider: AiProvider,
    pub model: String,
    /// Run by the weekly schedule rather than by hand.
    #[serde(default)]
    pub scheduled: bool,
    /// The revision it looked at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<i64>,
    pub summary: String,
    /// Most severe first. Empty means all clear.
    pub findings: Vec<Finding>,
}

impl Review {
    /// A review finished now.
    #[must_use]
    pub fn new(
        scope: Scope,
        provider: AiProvider,
        model: &str,
        summary: String,
        findings: Vec<Finding>,
    ) -> Self {
        let now = SystemTime::now();
        let since = now.duration_since(UNIX_EPOCH).unwrap_or_default();
        Self {
            id: format!("r{}", since.as_millis()),
            created: hearthgate_model::duration::format_rfc3339(now),
            created_unix: since.as_secs(),
            scope,
            provider,
            model: model.to_owned(),
            scheduled: false,
            revision: None,
            summary,
            findings,
        }
    }
}

const SYSTEM: &str = "You review the configuration of Hearthgate, a self-hosted gateway that is \
the only internet entry point of a home server. It has public interfaces (domains with automatic \
TLS certificates, and TCP or UDP ports), internal services (local ports or managed Docker \
containers and stacks), routes from interfaces to services with load balancing, an nftables \
firewall with blocklists and bans, UPnP port forwarding, a WireGuard VPN with internal DNS, and \
an admin dashboard reachable only from the LAN or VPN.

You get the desired configuration as JSON together with what the gateway sees now. Data was \
redacted before it reached you: text such as <PUBLIC_IP_1>, <PRIVATE_IP_2>, <DOMAIN_1> or \
<SECRET_3> stands for a real value. The same placeholder always means the same value. Keep \
placeholders exactly as written when you mention or reuse those values, and never guess what \
they hide.

Report only real problems that you can point to in the material: security exposure, \
misconfiguration, failing health, certificates close to expiry, noisy or risky firewall rules, \
or likely causes of errors. Don't report matters of taste. Rank each finding high, medium or \
low. When nothing needs attention, return no findings and say so in the summary.

A finding may carry a fix: changes to the desired configuration JSON as JSON Patch operations \
(add, remove or replace) with JSON Pointer paths into the configuration you were given, such as \
/firewall/rules/0. Give each value as JSON text in value_json (use \"null\" for remove). Only \
suggest a fix you are confident in; otherwise leave changes empty and explain what to check. \
The user reviews every change before it is applied.

Write plainly for someone who runs a home server. Keep titles short. Answer with JSON only.";

/// The JSON schema the answer must follow.
#[must_use]
pub fn schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["summary", "findings"],
        "properties": {
            "summary": {"type": "string"},
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["severity", "area", "title", "detail", "fix"],
                    "properties": {
                        "severity": {"type": "string", "enum": ["high", "medium", "low"]},
                        "area": {"type": "string"},
                        "title": {"type": "string"},
                        "detail": {"type": "string"},
                        "fix": {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["summary", "changes"],
                            "properties": {
                                "summary": {"type": "string"},
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
                    }
                }
            }
        }
    })
}

/// The question for `scope`, about `material`, which must be redacted
/// already.
#[must_use]
pub fn prompt(scope: &Scope, material: &Value) -> Prompt {
    let task = match scope {
        Scope::All => "Review the whole configuration and the current state.".to_owned(),
        Scope::Firewall => "Review the firewall: its rules, blocklists, bans and the recent \
            drops and attacks. Look for rules that expose more than needed, rules that never \
            match or shadow each other, and attackers that should be banned."
            .to_owned(),
        Scope::Service { service } => format!(
            "The service \"{service}\" is unhealthy or failing. From its health checks, the \
             proxy errors, the routes that use it and its container logs, explain the most \
             likely causes, most likely first, as findings. Suggest a fix where the \
             configuration is the cause."
        ),
    };
    let material = serde_json::to_string_pretty(material).unwrap_or_default();
    Prompt {
        system: SYSTEM.to_owned(),
        user: format!("{task}\n\nMaterial:\n{material}"),
        schema: schema(),
    }
}

#[derive(Deserialize)]
struct RawAnswer {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    findings: Vec<RawFinding>,
}

#[derive(Deserialize)]
struct RawFinding {
    severity: Severity,
    #[serde(default)]
    area: String,
    title: String,
    #[serde(default)]
    detail: String,
    #[serde(default)]
    fix: Option<RawFix>,
}

#[derive(Deserialize)]
pub(crate) struct RawFix {
    #[serde(default)]
    pub(crate) summary: String,
    #[serde(default)]
    pub(crate) changes: Vec<RawChange>,
}

#[derive(Deserialize)]
pub(crate) struct RawChange {
    op: Op,
    path: String,
    #[serde(default)]
    value_json: Option<String>,
    /// Some models give the value itself.
    #[serde(default)]
    value: Option<Value>,
}

/// The JSON object in `text`: models sometimes wrap it in a code fence or a
/// sentence.
pub(crate) fn json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    (end > start).then(|| &text[start..=end])
}

/// Reads an answer, maps placeholders back to real values, and sorts the
/// findings, most severe first. A fix whose values can't be read is
/// dropped.
///
/// # Errors
/// When the answer is not the requested JSON.
pub fn parse(answer: &str, redactor: &Redactor) -> Result<(String, Vec<Finding>), Error> {
    let body = json_object(answer).ok_or_else(|| Error::Answer("no JSON object in it".into()))?;
    let raw: RawAnswer = serde_json::from_str(body).map_err(|e| Error::Answer(e.to_string()))?;
    let mut findings: Vec<Finding> = raw
        .findings
        .into_iter()
        .map(|f| Finding {
            severity: f.severity,
            area: redactor.unredact(&f.area),
            title: redactor.unredact(&f.title),
            detail: redactor.unredact(&f.detail),
            fix: f.fix.and_then(|fix| read_fix(fix, redactor)),
        })
        .collect();
    findings.sort_by_key(|f| f.severity);
    Ok((redactor.unredact(&raw.summary), findings))
}

pub(crate) fn read_fix(fix: RawFix, redactor: &Redactor) -> Option<Fix> {
    if fix.changes.is_empty() {
        return None;
    }
    let mut changes = Vec::with_capacity(fix.changes.len());
    for c in fix.changes {
        let mut value = match (c.op, c.value_json, c.value) {
            (Op::Remove, _, _) => Value::Null,
            (_, Some(text), _) => serde_json::from_str(&text).ok()?,
            (_, None, Some(v)) => v,
            (_, None, None) => return None,
        };
        redactor.unredact_json(&mut value);
        changes.push(PatchOp {
            op: c.op,
            path: c.path,
            value,
        });
    }
    Some(Fix {
        summary: redactor.unredact(&fix.summary),
        changes,
    })
}

/// The state `fix` would produce from `state`, checked like any edit.
///
/// # Errors
/// When the changes don't apply, or the result isn't a valid configuration.
pub fn propose(state: &DesiredState, fix: &Fix) -> Result<DesiredState, String> {
    let mut doc = serde_json::to_value(state).map_err(|e| e.to_string())?;
    patch::apply(&mut doc, &fix.changes).map_err(|e| e.to_string())?;
    let next: DesiredState = serde_json::from_value(doc).map_err(|e| e.to_string())?;
    next.validate().map_err(|e| e.to_string())?;
    Ok(next)
}

/// Drops fixes that don't apply to `state`, saying so in the finding.
pub fn check_fixes(state: &DesiredState, findings: &mut [Finding]) {
    for f in findings {
        if let Some(fix) = &f.fix
            && let Err(why) = propose(state, fix)
        {
            tracing::debug!("AI fix for {:?} dropped: {why}", f.title);
            f.fix = None;
            f.detail.push_str(
                "\n\n(A change was suggested but it isn't valid for this configuration, so it \
                 isn't offered.)",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_fenced_answers_and_unredacts() {
        let mut r = Redactor::new(&[]).unwrap();
        let red = r.redact("allow 203.0.113.9 on example.com");
        assert!(!red.contains("203.0.113.9"));
        let answer = format!(
            "Here you go:\n```json\n{}\n```",
            json!({
                "summary": "One issue.",
                "findings": [
                    {"severity": "low", "area": "firewall", "title": "minor", "detail": "", "fix": {"summary": "", "changes": []}},
                    {"severity": "high", "area": "firewall", "title": format!("{red} is wide open"), "detail": "d",
                     "fix": {"summary": "ban it", "changes": [
                        {"op": "add", "path": "/firewall/bans/-", "value_json": format!("\"{}\"", red.split(' ').nth(1).unwrap())}
                     ]}}
                ]
            })
        );
        let (summary, findings) = parse(&answer, &r).unwrap();
        assert_eq!(summary, "One issue.");
        assert_eq!(findings[0].severity, Severity::High);
        assert_eq!(
            findings[0].title,
            "allow 203.0.113.9 on example.com is wide open"
        );
        let fix = findings[0].fix.as_ref().unwrap();
        assert_eq!(fix.changes[0].value, json!("203.0.113.9"));
        assert!(findings[1].fix.is_none());
    }

    #[test]
    fn refuses_non_json() {
        let r = Redactor::new(&[]).unwrap();
        assert!(parse("I can't help with that.", &r).is_err());
    }

    #[test]
    fn invalid_fixes_are_dropped() {
        let state = DesiredState::default();
        let mut findings = vec![Finding {
            severity: Severity::Medium,
            area: "x".into(),
            title: "t".into(),
            detail: "d".into(),
            fix: Some(Fix {
                summary: "s".into(),
                changes: vec![PatchOp {
                    op: Op::Replace,
                    path: "/nope/0".into(),
                    value: json!(1),
                }],
            }),
        }];
        check_fixes(&state, &mut findings);
        assert!(findings[0].fix.is_none());
    }
}
