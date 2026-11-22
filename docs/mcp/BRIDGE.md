# The stdio bridge: Claude Desktop without Node

Metocast's MCP server speaks HTTP. A client that can do that (Claude Code, Cursor, VS Code, any MCP
SDK) needs nothing else. Claude Desktop's JSON config and some other clients can only **start a
program and talk to it over stdin and stdout**. For those, the Mac app carries the program:

```
metocast-server mcp-stdio
```

It is the same `metocast-server` helper the app already runs as its core, started with a different
first argument. Nothing else is installed: no Python, Node, Docker or developer tools.

```
AI app (Claude Desktop)      metocast-server mcp-stdio                     Metocast
   stdin  ──JSON line──▶     POST /mcp  (token, Mcp-Session-Id)    ──▶     server/mcp.rs
   stdout ◀──JSON line──     200 JSON  |  202  |  text/event-stream  ◀──
                             GET /mcp (event stream)               ──▶     notifications
```

## Connect Claude Desktop

Open **Connectors → AI Agents (MCP)** in the Mac app and turn **Enabled** on first. Then either:

- **Install in Claude Desktop.** Opens the extension that ships inside the app. Claude asks for the
  access token: choose **Copy Access Token** in Metocast and paste it. Leave the address as it is
  when Metocast runs on this Mac.
- **Copy Config for Claude Desktop.** One paste into Claude Desktop's configuration (Settings,
  Developer, Edit Config). The copied config names the bridge and carries the token in `env`.

Either way, **Test Connection** in the same screen runs the check below and shows the result.

Metocast has to have been opened once on the Mac: every time the app starts it points
`~/Library/Application Support/Metocast/mcp/metocast-mcp` at its own helper, and both routes use that
path, so they keep working when the app moves or updates.

A notarized release ([releasing the Mac app](../features/install-mac.md)) opens without asking. A
build that is not notarized (CI's ad-hoc zip, or your own) makes macOS ask you to approve it the
first time it opens (System Settings, Privacy & Security, **Open Anyway**). Do that for Metocast
itself; Claude then starts the helper from inside an app you have already approved.

## How it behaves

- **A dumb pipe.** Each line on stdin is one JSON-RPC message. A request is sent with `POST /mcp` and
  its reply is written as one line; a notification gets `202` and writes nothing. The bridge does
  not look at tool calls.
- **Session.** It keeps the `Mcp-Session-Id` from `initialize` and sends it, with
  `MCP-Protocol-Version`, on every later request. When Metocast has forgotten the session (the app
  restarted, or fifteen idle minutes passed) the server answers `404`; the bridge repeats the
  client's own `initialize`, then `notifications/initialized`, and sends the failed request again.
  The AI app is not told.
- **Notifications from Metocast.** After `initialize` it opens `GET /mcp` as an event stream and
  writes each event as a line. It reconnects with a growing pause while the server is away.
- **Concurrency.** Each message is handled on its own, so a long `events_poll` does not hold up the
  calls behind it.
- **Exit.** When stdin closes it lets requests already sent finish (at most five seconds), ends the
  session so the agent leaves the connected list at once, and exits `0`.
- **Output.** Stdout carries protocol only. Everything else goes to stderr, which Claude Desktop
  keeps in its MCP log.
- **Token.** It is read from the environment or a file, never from the command line, which any user
  can see with `ps`. It is sent only to the address it was given, and redirects are not followed.
  A warning goes to stderr when the address is `http://` and not this Mac.

## Settings

| Setting             | Environment variable | Argument            | Default                     |
| ------------------- | -------------------- | ------------------- | --------------------------- |
| Address of Metocast | `METOCAST_MCP_URL`   | `--url ADDRESS`     | `http://127.0.0.1:3737/mcp` |
| Access token        | `METOCAST_MCP_TOKEN` | `--token-file PATH` | none                        |
| Check and exit      |                      | `--check`           |                             |

Exit codes: `0` ran or passed, `1` the check failed, `2` bad arguments.

## `--check`

```bash
METOCAST_MCP_TOKEN=<token> "$HOME/Library/Application Support/Metocast/mcp/metocast-mcp" mcp-stdio --check
```

It connects, initializes, lists the tools, ends the session and prints one line:

```
Metocast 0.1.0 answers at http://127.0.0.1:3737/mcp with 182 tools.
```

On failure it prints the reason below and exits `1`. This is the first thing to ask for when
something does not work. The tool count is lower on a Mac without Keynote, and in read-only mode.

## What the messages mean

An AI app shows these as the error of the call that failed.

| Message                                                                                | Cause and fix                                                                                             |
| -------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| "Metocast is not running, or the address is wrong (...)"                               | The app is closed, its server is stopped, or `--url` is wrong. Open Metocast. The address ends in `/mcp`. |
| "The access token is wrong or missing. Copy it again from Metocast ..."                | Copy the token again. Rotating it invalidates old configs.                                                |
| "MCP is turned off. Turn it on in the Metocast app under Connectors, AI Agents (MCP)." | Turn on **Enabled** and save.                                                                             |
| "MCP only accepts callers on this machine. Turn on LAN access in the Metocast app."    | The address is another device. Turn on **Allow other devices on the network**.                            |
| "Metocast answered 404 Not Found at (...)"                                             | Something else answers at that address, or it does not end in `/mcp`.                                     |
| "Metocast no longer knows this session and a new one could not be started."            | Metocast refused the new session too. Check the three lines above, then restart the AI app.               |
| "Could not talk to Metocast at (...): ..."                                             | The connection failed for another reason, for example a certificate. The reason follows.                  |
| "Metocast has not been opened on this Mac yet."                                        | From the extension's launcher: open the Metocast app once, then restart Claude.                           |

## Limits

- **macOS only.** The subcommand exists in every `metocast-server` build, but nothing packages it
  for Windows or Linux yet. There, clients that only speak stdio still need
  [`mcp-remote`](https://www.npmjs.com/package/mcp-remote) (which needs Node), and HTTP clients
  need nothing.
- The Tauri desktop app runs the core inside itself and ships no helper, so it has no bridge. Its
  Sanctum page keeps the HTTP command and the `mcp-remote` config.
- **Subscriptions are not carried over a restart.** After the session is replaced, a client that
  had subscribed to a resource must subscribe again. Tools, resources and prompts keep working.
- JSON-RPC batches are passed through, but a failure on one is reported without an id.

## For maintainers

- Code: `crates/metocast-mcp-bridge` (a library, so it is tested without the server) and a
  first-line dispatch in `crates/metocast-server/src/bin/metocast-server.rs`.
- **The dispatch comes first.** `metocast-server` normally starts embedded PostgreSQL on a fixed
  port, and a failed start kills whatever listens there, which would be the running app's database.
  `mcp-stdio` must run before logging, the token check and the data directory.
  `crates/metocast-server/tests/mcp_stdio.rs` runs the real binary with an empty environment and
  checks it creates nothing.
- Tests: `cargo test -p metocast-mcp-bridge` (a fake server), the binary test above, and
  `e2e/mcp/stdio-bridge.test.ts` (the built binary behind the SDK's stdio client, against the test
  server). CI also runs `mcp-stdio --check` with an empty `PATH` against the built Mac app and fails
  if the helper links anything outside the system.
- The extension is `scripts/mcpb/` (manifest and launcher) packed by `scripts/build-mcpb.sh`. The
  Xcode build phase puts `Metocast.mcpb` in the app's Resources. It holds no binary, only a launcher
  that runs the helper at the link the app keeps.
- A change to the MCP protocol or session handling in `server/mcp.rs` must keep this subcommand and
  its e2e test passing.

Plan and decisions: [plans/PLAN-mcp-stdio-bridge.md](../../plans/PLAN-mcp-stdio-bridge.md).
