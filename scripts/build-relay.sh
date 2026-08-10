#!/bin/sh
# kamux-relay をビルドし、Tauri の externalBin が要求する
# {name}-{target-triple} という名前で src-tauri/bin/ へ配置する。
set -eu

ROOT=$(cd "$(dirname "$0")/.." && pwd)
PROFILE_ARG=${1:-}

if [ "$PROFILE_ARG" = "--release" ]; then
  CARGO_PROFILE_ARG="--release"
  SRC="$ROOT/target/release/kamux-relay"
else
  CARGO_PROFILE_ARG=""
  SRC="$ROOT/target/debug/kamux-relay"
fi

TRIPLE=$(rustc -vV | sed -n 's/^host: //p')
if [ -z "$TRIPLE" ]; then
  echo "failed to determine host target triple" >&2
  exit 1
fi

# shellcheck disable=SC2086
cargo build --manifest-path "$ROOT/Cargo.toml" -p kamux-relay $CARGO_PROFILE_ARG

DEST="$ROOT/src-tauri/bin/kamux-relay-$TRIPLE"
mkdir -p "$ROOT/src-tauri/bin"
cp "$SRC" "$DEST"
chmod +x "$DEST"

# 配置されたバイナリが実際に要求したプロファイルの成果物であることを照合する。
# 期待値は $SRC / $PROFILE_ARG から再導出せず、プロファイルごとにリテラルのパスを
# 直書きする（同じ変数から導出すると、その変数自体の取り違えと一緒に動いて
# 検査が恒真になってしまうため）。
if [ "$PROFILE_ARG" = "--release" ]; then
  cmp -s "$ROOT/target/release/kamux-relay" "$DEST" || {
    echo "placed binary is not the release build" >&2
    exit 1
  }
else
  cmp -s "$ROOT/target/debug/kamux-relay" "$DEST" || {
    echo "placed binary is not the debug build" >&2
    exit 1
  }
fi

echo "placed src-tauri/bin/kamux-relay-$TRIPLE"
