# Operating it

- `systemctl reload hearthgate` makes the gateway re-read the current revision at once.
- Logs go to the journal: `journalctl -u hearthgate -f`. Set `log = "hearthgate=debug,info"` in the bootstrap file for more detail.
- Ports a listener cannot bind (for example because another program uses them) are reported in the log and in `/api/v1/status`, and retried every 10 seconds. Everything else keeps working.
