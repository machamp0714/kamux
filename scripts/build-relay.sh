#!/bin/sh
# kamux-relay をビルドし、Tauri の externalBin が要求する
# {name}-{target-triple} という名前で src-tauri/bin/ へ配置する。
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
PROFILE_ARG=${1:-}

if [ "$PROFILE_ARG" = "--release" ]; then
  CARGO_PROFILE_ARG="--release"
  PROFILE_DIR="release"
else
  CARGO_PROFILE_ARG=""
  PROFILE_DIR="debug"
fi

TRIPLE=$(rustc -vV | sed -n 's/^host: //p')
if [ -z "$TRIPLE" ]; then
  echo "failed to determine host target triple" >&2
  exit 1
fi

# shellcheck disable=SC2086
cargo build --manifest-path "$ROOT/Cargo.toml" -p kamux-relay $CARGO_PROFILE_ARG

mkdir -p "$ROOT/src-tauri/bin"
cp "$ROOT/target/$PROFILE_DIR/kamux-relay" "$ROOT/src-tauri/bin/kamux-relay-$TRIPLE"
chmod +x "$ROOT/src-tauri/bin/kamux-relay-$TRIPLE"

echo "placed src-tauri/bin/kamux-relay-$TRIPLE"
