#!/bin/bash
# Build, install and launch only the deployed macOS app with a stable identity.
set -euo pipefail
repo_root=$(cd "$(dirname "$0")/.." && pwd)
cd "$repo_root"
export PATH="$HOME/.bun/bin:$HOME/.cargo/bin:$PATH"

identity=$(bash scripts/setup-local-signing.sh)
export APPLE_SIGNING_IDENTITY="$identity"
export CMAKE_POLICY_VERSION_MINIMUM="${CMAKE_POLICY_VERSION_MINIMUM:-3.5}"
export CARGO_PROFILE_RELEASE_STRIP="${CARGO_PROFILE_RELEASE_STRIP:-true}"

# A private target directory keeps the build bundle outside Spotlight/app lists.
export CARGO_TARGET_DIR="$repo_root/src-tauri/.local-build"
config=$(mktemp "${TMPDIR:-/tmp}/parler-deploy-config.XXXXXX")
trap 'rm -f "$config"' EXIT
cat > "$config" <<EOF
{"bundle":{"createUpdaterArtifacts":false,"macOS":{"signingIdentity":"$identity"}}}
EOF
bun run tauri build --bundles app --config "$config"

source_app="$CARGO_TARGET_DIR/release/bundle/macos/Parler.app"
installed_app="/Applications/Parler.app"
codesign --verify --deep --strict "$source_app"
signature=$(codesign -d -r- "$source_app" 2>&1)
if [[ "$signature" == *'designated => cdhash '* ]]; then
  echo "Refusing to deploy an ad-hoc build: it would invalidate macOS permissions." >&2
  exit 1
fi

staged_app="/Applications/.Parler-install-$$.app"
previous_app="/Applications/.Parler-previous-$$.app"
committed=0
cleanup() {
  rm -f "$config"
  if [[ "$committed" == 0 && -d "$previous_app" ]]; then
    if [[ -d "$installed_app" ]]; then rm -rf "$installed_app"; fi
    mv "$previous_app" "$installed_app"
  fi
  if [[ -d "$staged_app" ]]; then rm -rf "$staged_app"; fi
}
trap cleanup EXIT
ditto "$source_app" "$staged_app"
codesign --verify --deep --strict "$staged_app"

# Stop only this app's deployed executable, never other apps or dev instances.
app_pids=$(pgrep -f '^/Applications/Parler.app/Contents/MacOS/parler([[:space:]]|$)' || true)
if [[ -n "$app_pids" ]]; then
  kill -TERM $app_pids
  for _ in {1..50}; do
    if ! pgrep -f '^/Applications/Parler.app/Contents/MacOS/parler([[:space:]]|$)' >/dev/null; then break; fi
    sleep 0.1
  done
  if pgrep -f '^/Applications/Parler.app/Contents/MacOS/parler([[:space:]]|$)' >/dev/null; then
    echo "Parler is still running. Deployment stopped before replacing it." >&2
    exit 1
  fi
fi

if [[ -d "$installed_app" ]]; then mv "$installed_app" "$previous_app"; fi
mv "$staged_app" "$installed_app"
codesign --verify --deep --strict "$installed_app"
committed=1
if [[ -d "$previous_app" ]]; then rm -rf "$previous_app"; fi

lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister
"$lsregister" -u "$source_app" >/dev/null 2>&1 || true
"$lsregister" -f "$installed_app"
# No runnable build bundle is left to compete with the installed app.
if [[ -d "${source_app}.built" ]]; then rm -rf "${source_app}.built"; fi
mv "$source_app" "${source_app}.built"
# LaunchServices can briefly return -600 while the replaced app is exiting.
launched=0
for attempt in {1..5}; do
  if open "$installed_app"; then
    launched=1
    break
  fi
  sleep 1
done
if [[ "$launched" != 1 ]]; then
  echo "Installed successfully, but LaunchServices could not launch $installed_app." >&2
  exit 1
fi
echo "Installed and launched: $installed_app"
codesign -d -r- "$installed_app" 2>&1
