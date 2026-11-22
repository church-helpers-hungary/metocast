# Releasing the Mac app

The Mac app ships as a **Developer ID download**: it's signed with the team's Developer ID certificate,
notarized by Apple and zipped, so it opens on any Apple Silicon Mac without the **Open Anyway**
step. It can't go through the Mac App Store. Store apps must run in the App Sandbox, and server
mode can't: it downloads and runs PostgreSQL, and runs `osascript`, `lsof`, `kill`, FFmpeg and the
Blackmagic RAW framework from `/Applications`.

CI's `Metocast-macOS.zip` is ad-hoc signed and is not notarized. Use the script below for a release
people download.

## Once: certificate and notary credentials

1. Xcode → Settings → Accounts → the team → **Manage Certificates…** → **+** →
   **Developer ID Application**. Only the Account Holder can create one. The certificate lands in
   the login keychain.
2. At [account.apple.com](https://account.apple.com) → Sign-In and Security → App-Specific
   Passwords, make a password for notarytool.
3. Store it in the keychain under the profile name the script uses. The command prompts for the
   Apple ID and the password, so nothing ends up in shell history:

   ```bash
   xcrun notarytool store-credentials metocast-notary --team-id 4RQDZKD3ZG
   ```

## Each release

Build the web pages and the bridge first, as for any Mac build (see the README), then:

```bash
scripts/notarize-mac.sh
```

The script:

1. Builds the Release app with the version from the latest `v*` tag.
2. Signs every helper in `Contents/MacOS` (`metocast-server`, `ffmpeg`, `ffprobe`, `whisper-cli`,
   `lip-reader`, `braw-decode`) and then the app. Each one gets the hardened runtime, a secure
   timestamp and [`Metocast-mac.entitlements`](../../apple-app/Metocast-mac.entitlements), which
   allows microphone recording and Apple Events for Keynote. `braw-decode` keeps its own
   entitlements file so it can load Blackmagic's framework.
3. Sends the app to Apple's notary service and waits. If Apple rejects it, the script prints Apple's
   log and stops.
4. Staples the ticket, checks the app with Gatekeeper and writes
   `target/apple-release/Metocast-<version>-macOS.zip`.

Attach that zip to the GitHub release. `DEVELOPER_ID` picks a different certificate when the
keychain holds more than one, and `NOTARY_PROFILE` picks a different profile.

On a Mac that installs it: unzip it, move **Metocast** to Applications and open it. The first time
the server records audio or controls Keynote, macOS asks for the microphone and Automation once.
