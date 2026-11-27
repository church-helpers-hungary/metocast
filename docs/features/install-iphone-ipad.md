# Installing Metocast on an iPhone or iPad

The iPhone and iPad app (`apple-app/Metocast.xcodeproj`, bundle ID `com.metocast.native`, iOS 26
or later) is a client of the Metocast server running on a Mac. There are two ways to put it on a
device:

| Route | Account | How long it runs | For services? |
| --- | --- | --- | --- |
| **App Store** ([below](#app-store-release)) | none for users; the team's Developer Program to publish | until removed | yes |
| **TestFlight** (below) | Apple Developer Program (paid) | each build 90 days; upload a new one before then | yes |
| **Xcode, Run on the device** | Apple Developer Program (paid) | 1 year (the profile's life) | yes, as a fallback |
| **Xcode, Run on the device** | free Personal Team | **7 days**, then the app stops opening | no |

A free Personal Team build stops launching after a week, possibly mid-service, so use it only to
try the app.

## Once: signing and App Store Connect

1. In Xcode → Settings → Accounts, sign in with the Apple ID of the Developer Program team.
2. Open `apple-app/Metocast.xcodeproj`, select the **Metocast** target → Signing & Capabilities,
   and pick the team. Signing stays **Automatic**; Xcode writes the team ID into the project as
   `DEVELOPMENT_TEAM`.
3. In [App Store Connect](https://appstoreconnect.apple.com) → Apps → **+** → New App: platform
   iOS, bundle ID `com.metocast.native` (register it under Certificates, Identifiers & Profiles
   first if it is not in the list). Internal TestFlight testers install builds straight away,
   without App Review; the App Store needs review (see [App Store release](#app-store-release)).
4. App Store Connect → the app → TestFlight → Internal Testing: add a group and add yourself (and
   anyone else who runs the services) as testers.
5. Install **TestFlight** from the App Store on the iPhone and the iPad and sign in with a tester's
   Apple ID.

## Each release: archive and upload

The version on the device follows the release tags: the marketing version is the latest `v*` tag
(as CI stamps the Mac app), and the build number is the upload time, which App Store Connect needs
to be new for every upload. Nothing in the project file changes, so there is nothing to commit.

Build the web pages and the bridge first, as for any build (see the README), then:

```bash
TAG=$(git describe --tags --abbrev=0 --match 'v*')
xcodebuild -project apple-app/Metocast.xcodeproj -scheme Metocast -configuration Release \
  -destination 'generic/platform=iOS' -archivePath target/Metocast-iOS.xcarchive \
  MARKETING_VERSION="${TAG#v}" CURRENT_PROJECT_VERSION="$(date +%Y%m%d.%H%M)" \
  -allowProvisioningUpdates archive
xcodebuild -exportArchive -archivePath target/Metocast-iOS.xcarchive \
  -exportOptionsPlist apple-app/ExportOptions.plist -exportPath target/Metocast-iOS-export \
  -allowProvisioningUpdates
```

**Keep an Apple Distribution certificate in this Mac's keychain** (Xcode → Settings → Apple
Accounts → the team → Manage Certificates… → **+** → Apple Distribution). Without one, Xcode signs
the upload with a cloud-managed certificate. For a team name with an accented letter such as
"ÁRON EDELY", it then writes the app's designated requirement with the letter decomposed (`A` and a
combining accent) while the certificate holds the single character. App Store Connect then refuses
the upload: *Invalid Signature … "Metocast.app/Metocast" is not properly signed*. A local
certificate makes `codesign` build the requirement from the certificate itself, so the two match.
Before an upload, `codesign --verify --strict` on the exported app must say it *satisfies its
Designated Requirement*.

[`ExportOptions.plist`](../../apple-app/ExportOptions.plist) exports for App Store Connect and
uploads in the same step, signed with the Xcode account from step 1. The same can be done by hand:
Xcode → Product → Archive (with "Any iOS Device" as the destination; set Version and Build on the
target's General tab first), then Organizer → Distribute App → **TestFlight Internal Only**.

After a few minutes of processing the build shows in TestFlight on each tester's devices; tap
**Install** (or turn on automatic updates in TestFlight). The app declares that it uses no
non-exempt encryption, so no export-compliance question holds the build back.

### From GitHub Actions, on every release tag

[`.github/workflows/testflight.yml`](../../.github/workflows/testflight.yml) does the same for each
`v*` tag, and for the newest tag when run by hand (Actions → TestFlight → Run workflow). It builds
the bridge, archives and exports with automatic signing (an App Store Connect API key stands in for
the Xcode account), and uploads with the same key. It uploads nothing unless
`codesign --verify --strict` passes and the signer is an Apple Distribution certificate. Turn on
**automatic distribution** for the TestFlight internal group, and each upload reaches the testers'
devices by itself.

It imports the team's own Apple Distribution certificate, so Xcode signs with it and not with a
cloud-managed one. That is also why Xcode Cloud can't do this for an accented team name: it only
signs in the cloud (see the note above; Apple tracks it as FB24633162). Set these repository
secrets once (Settings → Secrets and variables → Actions):

| Secret | What it holds | How to make it |
| --- | --- | --- |
| `APPSTORE_CERTIFICATE_P12` | the Apple Distribution certificate with its private key, base64 | Keychain Access → My Certificates → *Apple Distribution: …* → Export as `.p12` with a password, then `base64 -i cert.p12 \| pbcopy` |
| `APPSTORE_CERTIFICATE_PASSWORD` | that `.p12`'s password | |
| `APPSTORE_API_KEY` | the API key's `.p8` file, as text | App Store Connect → Users and Access → Integrations → App Store Connect API → Team Keys → **+**, access *Admin* (automatic signing creates the App Store profile) → Download (only once), then `pbcopy < AuthKey_XXXX.p8` |
| `APPSTORE_API_KEY_ID` | the key's ID | the Key ID column |
| `APPSTORE_API_ISSUER_ID` | the team's issuer ID | above the keys table |

When the certificate is replaced (it expires after a year, or after a team rename the old one is
revoked), export the new one and update the first two secrets.

## App Store release

The App Store uses the same archive and upload as TestFlight above. After you test a build on
TestFlight, submit it from App Store Connect. The app already has what Apple checks in the build:
a privacy manifest ([`PrivacyInfo.xcprivacy`](../../apple-app/Metocast/PrivacyInfo.xcprivacy):
no tracking, no collected data, `UserDefaults` for its own settings), the purpose strings for
camera and local network, and the export-compliance answer. The rest is filled in once in App Store
Connect → the app:

1. **App Information:** name, subtitle, category (Productivity), and the **Privacy Policy URL**,
   which is required. A short page is enough. It says that the app sends data only to the church's
   own Metocast server and that the developer collects nothing.
2. **App Privacy:** answer **Data Not Collected**. That matches the manifest. The server belongs to
   the user, not the developer, so Apple doesn't count what the app sends there as collection.
3. **Pricing and Availability:** price (Free) and countries.
4. **The version page:** screenshots for the 6.9" iPhone and the 13" iPad (the app runs on
   iPad), description, keywords, support URL, and the build from TestFlight.
5. **App Review Information:** without a server the app shows only its setup screen, and Apple
   rejects apps the reviewer can't use (guideline 2.1). Run a review server (below) and put its
   address and token under **Sign-In Information**, with notes like:

   > Metocast is the remote control for a church's livestream server. Tap the address field and
   > enter the server address `https://metocast-review.example.org`, then the token from the
   > Password field, and tap ✓. The server has sample events but no camera, mixer or streaming
   > equipment attached, so those controls show "Off".

   Also link a short screen recording of the app controlling a real service.
6. **Submit for Review.** When it's approved, release it by hand or automatically.

### The review server

The reviewer needs a Metocast server that stays reachable from the internet for the whole review,
so run a second, empty one beside the real server. It's the headless `metocast-server` (from
the notarized app's `Contents/MacOS`) with its own data folder, its own ports and its own token, so
it never touches the real database:

```bash
METOCAST_AUTH_TOKEN="$(openssl rand -hex 24)" METOCAST_PORT=3838 METOCAST_PG_PORT=55438 \
  METOCAST_DATA_DIR="$HOME/Library/Application Support/Metocast Review/data" METOCAST_REDACT=1 \
  ./metocast-server
```

- `METOCAST_PG_PORT` keeps its PostgreSQL off the real server's port. A failed start kills
  whatever holds that port, and that must not be the real server's database.
- `METOCAST_REDACT=1` hides the Mac's name, folders and LAN addresses from the reviewer.
- Give it no connector credentials and add a few events (`POST /api/events`), so the reviewer
  sees a working app.
- Publish it over HTTPS with a Cloudflare tunnel (`cloudflared tunnel create`,
  `cloudflared tunnel route dns`, and an ingress rule to `http://localhost:3838`). Run both the
  server and the tunnel as LaunchAgents, so they come back after a restart.

Stop both after approval and delete the folder. The token is then useless.

Each later release is the same: archive and upload with a higher marketing version, test it on
TestFlight, attach the build to a new version in App Store Connect and submit it.

## Xcode direct install

Connect the device by cable (or pair it over Wi-Fi in Window → Devices and Simulators), turn on
Developer Mode on it (Settings → Privacy & Security), choose it as the run destination and press
Run. A paid team's build runs for a year; a Personal Team's for 7 days (see the table above).

## Connecting to the Mac server

On the Mac that runs the server, open Settings (the toolbar's Settings button, or Metocast →
Settings…). While the server runs it shows the address, the token and a **Connect a Device** QR
code.

- **QR code:** point the iPhone or iPad Camera app at it and tap the Metocast banner. The
  `metocast://connect` link carries the address and token, and the app connects.
- **Address and token:** on the app's setup screen type the server address (for example
  `192.168.1.10:3737` or `the-mac.local:3737`) and the token, both shown in the Mac's Settings.

The device must be on the same network as the Mac. The first time, iOS asks for Local Network
access: allow it, or the app cannot reach the server (Settings → Privacy & Security → Local
Network → Metocast turns it back on).
