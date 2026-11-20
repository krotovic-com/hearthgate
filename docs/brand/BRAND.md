# Hearthgate brand guide

Hearthgate is a secure-by-default gateway for home servers, written in Rust. It is the only way in from the internet. Everything else stays private and is reachable over the built-in VPN.

**Tagline:** Expose what you choose. Reach the rest over VPN.
**Short tagline:** One way in.

## Name

- Written as **Hearthgate** in prose, with a capital H and as one word.
- The wordmark is always lowercase: `hearthgate`.
- Binary, crate, systemd unit and repository name: `hearthgate` (`hearthgate.service`). Use `hg` only as an informal CLI alias. Never use it in docs.

## Logo

The mark is a closed ring with one opening and one route going in. The route reaches the core, and the services sit inside the walls. It says what the product does: one entrypoint, and everything else stays sealed.

| File | Use |
|---|---|
| `logo-on-light.svg` / `logo-on-dark.svg` | Primary horizontal lockup (README header, website, dashboard login) |
| `mark-on-light.svg` / `mark-on-dark.svg` | Mark alone (dashboard sidebar, avatars) |
| `mark-mono-black.svg` / `mark-mono-white.svg` | Single-color print, stickers, terminal docs |
| `wordmark-on-light.svg` / `wordmark-on-dark.svg` | Wordmark alone, when the mark is already nearby |
| `app-icon.svg`, `app-icon-512.png` | App/PWA icon, GitHub org avatar |
| `favicon.svg`, `favicon-32.png` | Browser tab. A simplified mark that stays readable at 16 px |
| `social-preview.svg`, `social-preview.png` | GitHub social preview (1280×640) |

The wordmark is set in Sora SemiBold with −3% tracking and converted to outlines, so the files need no fonts installed.

**Rules**
- Keep clear space around the logo equal to the height of the mark's inner core (the blue dot) × 2.
- Minimum size is 16 px for the mark and 96 px wide for the horizontal lockup. Below that, use `favicon.svg`.
- The route (the blue line) always enters from the right, so don't rotate or mirror the mark.
- Don't recolor the route or core to anything other than Route blue, white or black.

## Color

| Token | Hex | Role |
|---|---|---|
| Ink | `#0E1A2B` | Primary dark, text on light, dark UI background |
| Route | `#3B6EF5` | Brand accent, primary actions, links, public routes |
| Sky | `#DCE6FF` | Accent tint, selected rows, focus halo on light |
| Tunnel | `#2BC6A0` | VPN and internal-only state, healthy status |
| Paper | `#F6F8FB` | Light background, text on dark |

Supporting neutrals for the UI (cool, biased toward Ink): `#16263D` (dark raised surface), `#8A9BB8` (muted text on dark), `#5A6B85` (muted text on light), `#E3E8F0` (borders on light).

Status colors are separate from the brand accent: healthy = Tunnel `#2BC6A0`, warning `#F2A93B`, critical `#E5484D`. Route blue never means "error" or "warning".

## Typography

| Role | Typeface | Notes |
|---|---|---|
| Display / headings | **Sora** (600, 700) | Tight tracking (−2 to −3%). Headings only. |
| Interface and docs | **Inter** (400, 500, 600) | All body and UI text |
| Technical | **JetBrains Mono** (400, 500) | Ports, IPs, domains, config, logs, CLI output |

All three are free (SIL Open Font License) and on Google Fonts. Use tabular numbers for any column of figures.

## Voice

Hearthgate is guarding someone's home, so it talks like a careful sysadmin friend: exact, calm and never alarmist.

- **State the fact, then the next step.** "UPnP is off, so open 443/tcp on your router yourself, or turn UPnP on in Settings."
- **Name real things.** Ports, domains, service names and counts, not "something" or "an issue".
- **Show secure-by-default rather than shout it.** Say what is closed and why: "Port 8443 is closed. Nothing is mapped to it."
- **Make AI suggestions clearly suggestions.** "The assistant suggests restarting jellyfin. Review the command before running it."
- **Avoid** fear-selling, exclamation marks, emoji in product copy, "Oops!", and military or fortress bravado.

| Write this | Not this |
|---|---|
| Blocked 42 requests from 203.0.113.7 to port 3389. Nothing listens there. | Threat neutralized! Your fortress is impenetrable! |
| Certificate for cloud.example.com renewed. Next renewal in 60 days. | SSL magic done ✨ |
| Couldn't reach the router over UPnP. Check that UPnP is enabled on the router. | Oops! Something went wrong. |

## Vocabulary

Use these terms consistently in the UI, docs and code:

- **Public interface**: a domain plus public ports (TCP, UDP or both) with its certificate and firewall rules.
- **Service**: something running inside. It is either a **system service** (managed outside Hearthgate, such as sshd) or a **container** / **stack** (Docker, managed by Hearthgate).
- **Route**: the link from a public interface to one or more services. Two or more targets means load-balanced.
- **VPN-only**: a service with no public interface, reachable by peers over the VPN at `name.home.internal`.
- **Peer**: a device connected to the VPN.
