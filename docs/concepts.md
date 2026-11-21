# How it fits together

Hearthgate has three building blocks, and everything you configure is stored as a numbered revision.

- A **public interface** is what the internet can reach: one or more domains and the ports they listen on.
- A **service** is something inside your network that Hearthgate can forward to: a program on the host (such as sshd) or an existing Docker container.
- A **route** connects a port of an interface to one or more service ports. One target forwards. Two or more are load-balanced.

Nothing listens on any port until an interface asks for it.

Everything you configure is the **desired state**. It is stored in a SQLite database as numbered **revisions**. Every change creates a new revision, so you always have the history and can roll back. A running gateway applies a new revision within a second, without a restart, and without dropping open connections on ports that did not change.

There are two files:

| File | What it holds | How you change it |
|---|---|---|
| `/etc/hearthgate/hearthgate.toml` | Bootstrap settings: data directory, log level, admin API, Docker socket | Edit it and restart the service |
| A desired-state file, for example `state.toml` | Interfaces, services, routes, certificates settings | The dashboard, or `hearthgate apply state.toml` |

The desired-state file is only an input. After `apply`, the database is the source of truth, and `hearthgate export` prints it back. You don't need a file at all if you use the dashboard.

## Desired state


A complete example is in [`packaging/examples/state.toml`](https://github.com/krotovic-com/hearthgate/blob/main/packaging/examples/state.toml). Check a file without applying it with `hearthgate validate state.toml`. Every problem is reported at once, with where it is.

Names of interfaces, services, routes and certificates use lowercase letters, digits and dashes.
