#!/bin/sh
# 契約 §14 の fake-agent。
# 引数: なし。環境変数 KAMUX_SESSION_ID / KAMUX_HOOKS_SOCK / KAMUX_RELAY_BIN を読む。
# 挙動: 起動出力 → SessionStart 発火 → 3 行出力 → Notification 発火 →
#       PermissionRequest 発火 → stdin 1 行待ち → 出力 → Stop 発火 → 終了コード 0
#
# PermissionRequest は契約 §12.4 で追加された権限系 hook。payload 構造が未確認なので、
# 中身が空のオブジェクトでも kind が argv から決まることをここで担保する。
set -u

relay() {
  # hook 種別は argv 第 1 引数（設計 §6-2）。stdin に payload を渡す。
  printf '%s' "$2" | "$KAMUX_RELAY_BIN" "$1"
}

echo "fake-agent starting"

relay SessionStart "{\"session_id\":\"fake-cc-0001\",\"transcript_path\":\"/tmp/t\",\"cwd\":\"$PWD\",\"hook_event_name\":\"SessionStart\",\"source\":\"startup\"}"

echo "line 1"
echo "line 2"
echo "line 3"

relay Notification "{\"session_id\":\"fake-cc-0001\",\"hook_event_name\":\"Notification\"}"

# payload 構造が未知でも argv で種別が決まることの確認（意図的に空オブジェクト）
relay PermissionRequest "{}"

# 入力待ち
read -r _line

echo "got input"

relay Stop "{\"session_id\":\"fake-cc-0001\",\"transcript_path\":\"/tmp/t\",\"cwd\":\"$PWD\",\"hook_event_name\":\"Stop\",\"stop_hook_active\":false}"

exit 0
