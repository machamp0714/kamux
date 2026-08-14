#!/bin/sh
# 無効な --resume ID を渡された claude を模すフィクスチャ。
# 契約 §14 の fake-agent.sh と違い、SessionStart を発火せず即座に非ゼロ終了する。
#
# 環境変数: KAMUX_SESSION_ID(読むだけ。リレーは呼ばない)
# 終了コード: 3
set -eu

echo "No conversation found with session ID: ${1:-<none>}" >&2
exit 3
