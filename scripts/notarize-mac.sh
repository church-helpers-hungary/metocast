#!/usr/bin/env bash
# Builds the Mac app, signs it and every helper with the Developer ID certificate and the hardened
# runtime, has Apple notarize it, staples the ticket and zips it for direct download:
# target/apple-release/Metocast-<version>-macOS.zip. See docs/features/install-mac.md.
#
# Needs, once: a "Developer ID Application" certificate in the login keychain, and notarytool
# credentials stored as the keychain profile $NOTARY_PROFILE (default metocast-notary).
# Build the web pages and the bridge first, as for any Mac build (pnpm build, scripts/build-apple.sh).
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"
identity="${DEVELOPER_ID:-Developer ID Application}"
profile="${NOTARY_PROFILE:-metocast-notary}"
tag=$(git describe --tags --abbrev=0 --match 'v*')
version="${tag#v}"
derived=target/apple-release-derived
out=target/apple-release
rm -rf "$derived" "$out"
mkdir -p "$out"

xcodebuild -project apple-app/Metocast.xcodeproj -scheme Metocast -configuration Release \
  -destination 'platform=macOS,arch=arm64' -derivedDataPath "$derived" \
  MARKETING_VERSION="$version" CURRENT_PROJECT_VERSION="$(date +%Y%m%d.%H%M)" build

app="$derived/Build/Products/Release/Metocast.app"
main="$app/Contents/MacOS/Metocast"
sign() {
  codesign --force --options runtime --timestamp --sign "$identity" "$@"
}

# Inside out: every nested Mach-O first, the app last, so its seal covers the new signatures.
while IFS= read -r -d '' file; do
  [ "$file" = "$main" ] && continue
  file -b "$file" | grep -q "Mach-O" || continue
  if [ "$(basename "$file")" = braw-decode ]; then
    sign --entitlements braw-decode/braw-decode.entitlements "$file"
  else
    sign --entitlements apple-app/Metocast-mac.entitlements "$file"
  fi
done < <(find "$app/Contents" -type f -perm +111 -print0)
sign --entitlements apple-app/Metocast-mac.entitlements "$app"
codesign --verify --deep --strict "$app"

ditto -c -k --keepParent "$app" "$out/notarize.zip"
result=$(xcrun notarytool submit "$out/notarize.zip" --keychain-profile "$profile" --wait --output-format json)
status=$(printf '%s' "$result" | plutil -extract status raw -)
if [ "$status" != Accepted ]; then
  echo "error: notarization $status" >&2
  xcrun notarytool log "$(printf '%s' "$result" | plutil -extract id raw -)" --keychain-profile "$profile" >&2
  exit 1
fi
rm "$out/notarize.zip"

xcrun stapler staple "$app"
spctl --assess --type execute --verbose "$app"
ditto -c -k --keepParent "$app" "$out/Metocast-$version-macOS.zip"
echo "$out/Metocast-$version-macOS.zip"
