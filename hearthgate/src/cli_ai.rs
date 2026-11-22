//! `hearthgate ai`: the optional AI assistant's settings and key.

use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use hearthgate_model::AiProvider;

use crate::cli::{AiCommand, author, open_store};
use crate::config::Bootstrap;

fn provider(name: &str) -> Result<AiProvider> {
    serde_json::from_value(serde_json::Value::String(name.to_owned())).with_context(|| {
        format!("unknown provider {name}: use anthropic, openai, gemini or openai-compatible")
    })
}

pub(crate) fn run(bootstrap: &Bootstrap, action: AiCommand) -> Result<ExitCode> {
    let store = open_store(bootstrap)?;
    let mut state = store.current_state()?;
    let key_path = state.ai.api_key_file.as_ref().map_or_else(
        || bootstrap.data_dir.join(hearthgate_api::ai::KEY_FILE),
        std::path::PathBuf::from,
    );
    match action {
        AiCommand::Status => {
            let ai = &state.ai;
            let name = serde_json::to_value(ai.provider)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            println!(
                "The AI assistant is {}. Provider {name}, model {}, endpoint {}.",
                if ai.enabled { "on" } else { "off" },
                ai.model_or_default().unwrap_or("(not set)"),
                ai.endpoint_or_default().unwrap_or("(not set)"),
            );
            let key = std::fs::read_to_string(&key_path).is_ok_and(|k| !k.trim().is_empty());
            println!(
                "API key: {} ({}).",
                if key { "stored" } else { "none" },
                key_path.display()
            );
            println!(
                "Weekly review: {}. Extra redaction patterns: {}.",
                if ai.weekly_review { "on" } else { "off" },
                ai.redact.len()
            );
        }
        AiCommand::Enable {
            provider: name,
            model,
            endpoint,
            weekly,
        } => {
            let before = state.ai.clone();
            state.ai.enabled = true;
            if let Some(name) = name {
                let p = provider(&name)?;
                if p != state.ai.provider {
                    state.ai.provider = p;
                    state.ai.model = None;
                    state.ai.endpoint = None;
                }
            }
            if model.is_some() {
                state.ai.model = model;
            }
            if endpoint.is_some() {
                state.ai.endpoint = endpoint;
            }
            if weekly {
                state.ai.weekly_review = true;
            }
            if state.ai == before {
                println!("The AI assistant is already on with these settings.");
                return Ok(ExitCode::SUCCESS);
            }
            let applied = store.apply(&state, &author(), "Turn the AI assistant on")?;
            println!(
                "Revision {}: the AI assistant is on. Nothing is sent until you start a \
                 review{}.",
                applied.meta().id,
                if state.ai.weekly_review {
                    " or the weekly review runs"
                } else {
                    ""
                }
            );
            if state.ai.provider.needs_key()
                && !std::fs::read_to_string(&key_path).is_ok_and(|k| !k.trim().is_empty())
            {
                println!("Store the provider's API key with `hearthgate ai key`.");
            }
        }
        AiCommand::Disable => {
            if !state.ai.enabled {
                println!("The AI assistant is already off.");
                return Ok(ExitCode::SUCCESS);
            }
            state.ai.enabled = false;
            let applied = store.apply(&state, &author(), "Turn the AI assistant off")?;
            println!("Revision {}: the AI assistant is off.", applied.meta().id);
        }
        AiCommand::Key { remove } => {
            if state.ai.api_key_file.is_some() {
                bail!(
                    "the key is read from {} (ai.api_key_file); change it there",
                    key_path.display()
                );
            }
            if remove {
                match std::fs::remove_file(&key_path) {
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                        return Err(e).context("cannot remove the key");
                    }
                    _ => println!("The API key is removed."),
                }
                return Ok(ExitCode::SUCCESS);
            }
            eprintln!("Paste the API key and press Enter:");
            let mut key = String::new();
            std::io::stdin()
                .read_line(&mut key)
                .context("cannot read the key")?;
            let key = key.trim();
            if key.is_empty() || key.chars().any(char::is_control) {
                bail!("that doesn't look like an API key");
            }
            if let Some(dir) = key_path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            hearthgate_api::write_private(&key_path, &format!("{key}\n"))
                .with_context(|| format!("cannot write {}", key_path.display()))?;
            println!("The API key is stored in {}.", key_path.display());
        }
    }
    Ok(ExitCode::SUCCESS)
}
