---
name: verify
summary: Verify workmux CLI behavior against an isolated tmux server.
---

# Verify workmux CLI

1. Build the current binary with `cargo build`.
2. Start an isolated server with `tmux -L <unique-name> new-session -d -s verify`.
3. Create the panes and tmux options needed by the flow under test.
4. Run `target/debug/workmux` inside the isolated session with `tmux send-keys` so the process receives the server's `TMUX` environment.
5. Redirect stdout, stderr, and the exit code to `/tmp` files, then inspect pane state with `list-panes -F` and window state with `list-windows -F`.
6. Probe at least one adjacent error or repeated-state path.
7. Stop the isolated server with `tmux -L <unique-name> kill-server`.

Direct subprocess execution does not target the isolated server. Execute the CLI from a pane.
