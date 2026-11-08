# Web presenter and network links

Other devices on the church network open the server by its address: a browser shows the slides
at `…/presenter?token=…`, and the Mac and phone apps connect to the plain server address. Metocast
shows **both** addresses this computer has, best first, each with its own **Copy** button:

1. **The LAN IP address**, e.g. `http://192.0.2.10:3737`. Every device can open it. This is the
   one to hand out, and the one the Raspberry Pi installer and the connect QR code use.
2. **The Bonjour name**, e.g. `http://studio-mac.local:3737`. It keeps working when the router hands
   the computer a new IP, but only on devices that resolve `.local` names (Apple devices; many
   Android phones, TVs and Windows PCs don't).

If the IP link stops working after a restart, the router gave the computer a new address: copy
the link again, or give the computer a fixed address (a DHCP reservation) on the router.

## Slow screens (Raspberry Pi receiver)

The Raspberry Pi `presenter-receiver` connects with `/ws?presenter=frames`. The server then
sends it each deck without its SVG slides (the photos in a song deck make those hundreds of
kilobytes), and draws the slide it shows as a JPEG on request (`presenter.frame`). When the
server is slow to answer, the Pi draws that one slide itself and asks the server again for the
next one. A screen that falls behind gets only the newest deck: one still waiting to be sent
when the operator moves on is dropped, so the screen jumps to what is on now instead of showing
each step it missed. See `presenter-receiver/PRESENTER_RECEIVER.md` for the messages.

## Where the links are

| Where                                         | What it shows                               |
| --------------------------------------------- | ------------------------------------------- |
| Mac/iPhone app: **Presentation → Settings**   | Web Presenter Link: the presenter page URLs |
| Mac app: **Settings → Server → Address**      | The server addresses                        |
| Sanctum: **Presentations** (web presenter)    | The presenter page URLs                     |
| Classic: **Connect**, home, **Presentations** | The server and presenter page URLs          |

A device that is a _client_ of another computer's server shows the one address it connected with.

## Entry points

The addresses are found by the app that hosts the server, not by the server itself, so there is
no REST, WebSocket or MCP call for them:

- Desktop (Tauri) host: the `get_local_hosts` command, `getLocalHosts()` in `@metocast/core-client`.
- Mac app: `LocalNetwork.hosts` and `ServerHost.networkURLs`.

The presenter page itself is the UI route `/presenter?token=<token>`. With
`-MetocastRedact YES` (Mac app) the links show placeholder addresses and a masked token.
