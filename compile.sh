#!/bin/sh
set -eu
cd "$(dirname "$0")"

npm install
npm --prefix ui install
npm run build

app="target/release/bundle/macos/VanTranscribe.app"
if [ ! -d "$app" ]; then
  echo "build did not produce $app" >&2
  exit 1
fi

rm -rf dist
mkdir -p dist
cp -R "$app" dist/
echo "built dist/VanTranscribe.app"
