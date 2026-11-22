# AI assistant

Hearthgate can ask an AI model to look over your setup: it reviews the whole configuration, checks the firewall rules, and explains why a service is failing. Every suggestion arrives as a change you review in your draft, so nothing is ever changed behind your back.

The assistant is **off by default**. While it is off, Hearthgate never connects to an AI provider; a test checks this on every build.

## Providers

| Provider | `provider` | Default model | Key |
|---|---|---|---|
| Claude (Anthropic API) | `anthropic` (default) | `claude-opus-5-5` | needed |
| OpenAI | `openai` | none, set `model` | needed |
| Google Gemini | `gemini` | none, set `model` | needed |
| Ollama, LM Studio, vLLM and other OpenAI-compatible servers | `openai-compatible` | none, set `model` | optional |

A local model keeps everything on your network and works without internet access. Point `endpoint` at it, such as `http://192.168.1.20:11434/v1` for Ollama. Larger models give better reviews; small ones may miss things or answer in the wrong format.

## Turning it on

In the dashboard, open Settings, AI assistant: switch it on, choose the provider and model, store the API key, and apply the draft. Or in the state:

```toml
[ai]
enabled = true
provider = "anthropic"
# model = "claude-opus-5-5"
# endpoint = "https://api.anthropic.com"
# api_key_file = "/etc/hearthgate/ai-key"   # instead of a key stored from the dashboard
# redact = ["smith", "project-\\w+"]        # more text to hide, as regular expressions
# weekly_review = true
# timeout = "2m"
```

or on the command line:

```sh
sudo -u hearthgate hearthgate ai enable --provider openai-compatible --model llama3.1 --endpoint http://127.0.0.1:11434/v1
sudo -u hearthgate hearthgate ai key        # paste the key, for providers that need one
```

**The API key** never goes into the configuration, its history, exports or the API. The dashboard and `hearthgate ai key` store it in the data directory (`secrets/ai-api-key`, readable only by Hearthgate), or set `api_key_file` to read it from a file you manage (`chown root:hearthgate`, `chmod 0640`).

## What is sent

A review sends the configuration (without the assistant's own settings and the notification targets), the gateway's status, certificate expiry dates, the firewall's counters and recent drops, and recent warnings and errors from the log. Troubleshooting a service adds its routes, its log lines and, for containers, their recent output.

Before anything leaves the machine, it is **redacted**:

- IP addresses become `<PUBLIC_IP_1>`, `<PRIVATE_IP_2>`, `<LOOPBACK_IP_1>` and so on, so the model still knows what kind of address it is.
- Domains become `<DOMAIN_1>`, keeping subdomains: `photos.example.com` becomes `photos.<DOMAIN_1>`.
- URLs, email addresses, private keys, tokens, passwords and other values that look random become `<URL_1>`, `<EMAIL_1>` or `<SECRET_1>`, as do all container environment values.
- Anything matching your `redact` patterns becomes `<REDACTED_1>`.

The same value always gets the same placeholder within a review, and placeholders in the answer are put back only on your machine. **Show exactly what is sent** (in Settings, AI review and the review panels) displays the exact request body without sending anything, and works while the assistant is off.

## Reviews and fixes

- **AI review** reviews the whole setup and lists findings ranked high, medium or low, or an all-clear.
- **Review rules with AI** on the Firewall page looks at the rules, blocklists, bans and recent drops.
- **Explain with AI** on a failing service explains the likely causes from its health checks, proxy errors and logs.

A finding can carry a fix. **Put fix in draft** applies it to the current configuration, checks it like any change, and loads it into your draft: review the diff, apply it, or discard it. Applied fixes are ordinary revisions, so History can roll them back. If your draft has unapplied changes, apply or discard them first.

The last 20 reviews are kept in the data directory (`ai/reviews`).

## Weekly review

With `weekly_review = true`, Hearthgate reviews the whole setup once a week. The result appears under AI review and as a notification of kind `reviews` (see [Notifications](notifications.md)). Restarts don't add extra runs.

## Privacy

Using a cloud provider means the redacted material is processed under that provider's terms. Redaction hides addresses, names and secrets, but the shape of your setup (which services exist, their ports, your firewall rules) is what the review is about and is sent. Use a local model if that is too much.
