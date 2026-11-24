#!/bin/sh
# Installs the latest home-feeling release for this user. Needs curl and unzip.
set -eu
base=https://github.com/an-dr/home-feeling/releases/latest/download
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
curl -fsSL "$base/home-feeling-linux-x64.json" -o "$work/manifest.json"
url=$(sed -n 's/.*"url": *"\([^"]*\)".*/\1/p' "$work/manifest.json")
sha=$(sed -n 's/.*"sha256": *"\([^"]*\)".*/\1/p' "$work/manifest.json")
curl -fsSL "$url" -o "$work/home-feeling.zip"
echo "$sha  $work/home-feeling.zip" | sha256sum -c - >/dev/null
unzip -q "$work/home-feeling.zip" -d "$work/app"
"$work/app/home-feeling-app" --install
echo "home-feeling is installed. Start it from your application list."
