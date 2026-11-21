# Command line

Run these commands as root or as the `hearthgate` user, because they open the database in the data directory.

| Command | What it does |
|---|---|
| `hearthgate run` | Run the gateway (what the service starts) |
| `hearthgate validate [FILE]` | Check a desired-state file, or the current state |
| `hearthgate apply FILE [-m MESSAGE] [--dry-run]` | Store a file as a new revision and show the diff |
| `hearthgate export [-o FILE] [-r N]` | Print the current (or an older) state as TOML |
| `hearthgate import FILE` | Restore an exported file (same as `apply`) |
| `hearthgate status` | Current revision and interfaces |
| `hearthgate revisions` | History, newest first |
| `hearthgate diff A [B]` | What changed between two revisions |
| `hearthgate rollback N` | Make revision N current again, as a new revision |
| `hearthgate cert import/list/remove` | Uploaded certificates |
| `hearthgate admin-token` | Print the admin API token |
| `hearthgate setup-code` | Print the code for creating the first dashboard account |
| `hearthgate admin-access [--reset]` | Show who can reach the dashboard, or reset that to the default |
| `hearthgate firewall status` | Whether the firewall is installed, and whether a change waits for confirmation |
| `hearthgate firewall confirm` | Keep the waiting firewall change (run it from a new SSH session) |
| `hearthgate firewall off` | Turn the firewall off as a new revision (a running Hearthgate then removes its table) |
| `hearthgate vpn status` | VPN settings, devices and their last handshake |
| `hearthgate vpn enable [--endpoint HOST]`, `hearthgate vpn disable` | Turn the VPN on or off as a new revision |
| `hearthgate vpn add NAME [--access all\|SVC,SVC] [-o FILE]` | Add a device and print its configuration and QR code once |
| `hearthgate vpn remove NAME` | Remove a device; it can't connect any more |
| `hearthgate docker status` | Managed containers and stacks |
| `hearthgate docker logs NAME [-n N]` | A managed container's recent output |
| `hearthgate docker update NAME` | Pull the newest image of a container or stack and recreate it if it changed |
| `hearthgate docker check-updates` | Whether newer images exist, without changing anything |
| `hearthgate backup create [-o FILE]` | Write a backup now |
| `hearthgate backup list` | Backups in the backup directory |
| `hearthgate backup restore FILE [--force]` | Restore a backup (stop the service first) |
| `hearthgate openapi` | Print the admin API's OpenAPI document |

`--data-dir` (or `HEARTHGATE_DATA_DIR`) points any command at another data directory, which is handy for trying things out without root.

`apply` and `rollback` never rewrite history: rolling back to revision 3 creates a new revision with revision 3's content. Applying a file equal to the current state creates nothing.
