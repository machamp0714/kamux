#!/bin/sh
# 無効な --resume ID を渡された claude を模すフィクスチャ。
# 契約 §14 の fake-agent.sh と違い、SessionStart を発火せず即座に非ゼロ終了する。
#
# 引数 $1 を stderr のメッセージへ埋め込む(未指定なら <none>)。
# KAMUX_SESSION_ID は呼び出し側が環境変数として渡すが、このスクリプトは
# 読まない(リレーも呼ばない)。
# 終了コード: 3
set -eu

echo "No conversation found with session ID: ${1:-<none>}" >&2
exit 3
