# Containers and stacks

Hearthgate can also run containers for you, replacing tools like Portainer for a typical home server:

```toml
[[containers]]
name = "whoami"                    # its name in Docker and in the internal DNS
image = "traefik/whoami:v1.10"
# running = true
# command = ["--port", "8080"]
env = { TZ = "Europe/Prague" }
volumes = ["/srv/whoami:/data", "whoami-cache:/cache:ro"]
# publish = ["127.0.0.1:8081:80"]  # host ports; rarely needed
# restart = "unless-stopped"       # or "always", "on-failure", "no"

[[stacks]]
name = "notes"                     # the Compose project name
env = { POSTGRES_PASSWORD = "change-me" }   # written to the stack's .env
compose = """
services:
  app:
    image: ghcr.io/example/notes:2
  db:
    image: postgres:17
    environment:
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD}
    volumes: ["db:/var/lib/postgresql/data"]
volumes:
  db: {}
"""
```

Containers and the containers of stacks join a Docker network called `hearthgate`. Hearthgate reaches them there, so to publish one, add a `docker` service with `network = "hearthgate"` and a route, with no host ports published. The dashboard's Add container form can add that service for you. Stack containers are named `STACK-SERVICE-1`, such as `notes-app-1`.

- A changed container is recreated; one removed from the state is removed (its volumes stay). A container that exits on its own is left to its restart policy.
- Stacks run with the `docker compose` command (install the Compose plugin), from files Hearthgate keeps in `/var/lib/hearthgate/stacks/NAME`. Removing a stack runs `docker compose down`, which keeps named volumes.
- Hearthgate never touches containers it didn't create. If a container with the same name already exists, it reports a problem instead.
- Images are pulled when missing. **Update** in the dashboard (or `hearthgate docker update NAME`) pulls the newest image and recreates the container if it changed. **Check for updates** asks the registries without changing anything; nothing is ever updated without you asking.
- Containers, Logs shows a container's output with search, CPU, memory and restarts.
- VPN devices reach containers too: devices with full access reach the `hearthgate` network, and managed containers get names like `whoami.home.internal`.

To only follow existing containers and never create any, set `docker.manage = false` in the bootstrap file; `docker.enabled = false` turns Docker off entirely.
