#!/bin/sh
# 汎用 CLI 役の手動スモーク用フィクスチャ(M3-3 / 契約 §14 への追加)。
#
# fake-agent.sh との違い: hooks を一切使わない。relay プロセスもセッション ID 環境変数も参照しない。
# これにより「hooks が無い CLI をアプリがどう見るか」だけを検証できる。
#
# 引数: なし
# 挙動: OSC タイトル設定(BEL 終端子を含む=誤検知の踏み台)→ 進捗 2 行 →
#       BEL 付きプロンプト → stdin 1 行待ち → 完了 1 行 → 沈黙して待機(自力で再開) →
#       終了コード 0 (Ctrl-C か stop_session まで待ち続ける)
set -eu

# OSC 0(ウィンドウタイトル)。終端子の BEL をベルと誤検知したらここで落ちる
printf '\033]0;fake-generic-cli\007fake-generic-cli started\n'

printf '\033[32m[1/3]\033[0m building\n'
printf '\033[32m[2/3]\033[0m linking\n'

# 本物のベル。ここで waiting_input(黄・破線)になるのが期待値
printf 'continue? [y/N] \007'
read -r _answer

printf '\033[32m[3/3]\033[0m done\n'

# 沈黙して待機。silence_timeout_secs 経過後に idle(白・破線)になるのが期待値。
# 契約 §118.5: スモークの設定(silence_timeout_secs = 5)より長く待ってから、
# 入力を読まずに自力で 1 行印字する。これが手動スモーク項目 5 の唯一の駆動源である
# -- 人間が Enter を押すと UserInput で緑になり、OutputActivity を 1 度も測らない。
# BEL を含めないこと(含めると項目 5 の観測に黄が混ざる)
sleep 12
printf '\033[32m[post]\033[0m resumed after idle\n'

# 再び沈黙して待機。手動で Ctrl-C するか、アプリから stop_session するまで生き続ける
while true; do
    sleep 3600
done
