---
name: kamux-lane
description: kamux の実装フェーズ（M1-1〜M3-4）を lane-controller エージェントとして起動する。「ステージ0 を起動」「M1-1 を始めて」「stage4」「M2-2 のレーンを立てて」などで起動する。フェーズ ID かステージ番号を引数に取る。
argument-hint: "<M1-1 | M1-2 | … | M3-4 | stage0 | … | stage6>"
---

# kamux レーン起動

`docs/superpowers/plans/2026-08-01-kamux/` の実装フェーズを `lane-controller` エージェントとして起動する。**あなた（team-lead）はレーンを起動して監督するだけで、コードは書かない。**

引数の形は 2 つ。

| 引数 | 動作 |
|---|---|
| `M1-1` 〜 `M3-4` | そのフェーズを 1 本だけ起動する |
| `stage0` 〜 `stage6` | そのステージのレーンを**1 メッセージ内でまとめて**起動する（= 並列実行） |

引数が無い場合は、ledger（`.superpowers/sdd/*/progress.md`）と `git log` から現在地を推定し、次に来るステージを提案してから確認を取る。

---

## 1. 起動前チェック（毎回、必ず）

```bash
# 1. エージェント定義が認識されているか（5 種）
ls .claude/agents/

# 2. 作業ツリーが汚れていないか
git status --short

# 3. 直前のステージの成果がマージ済みか
git log --oneline -5
```

`lane-controller` が利用可能エージェント一覧に出ていない場合は**セッション再起動が必要**。フォールバックとして `subagent_type: "claude"` で起動し、`.claude/agents/lane-controller.md` の本文をプロンプト冒頭に貼る。

---

## 2. ステージの定義

依存グラフから導出済み（`TEAM.md` §1）。**並列度は最大 3。** クリティカルパスは 7 フェーズ直列なので、レーンを増やしても縮まない。

| ステージ | レーン | isolation | マージ順 | 備考 |
|---|---|---|---|---|
| stage0 | M1-1 | 不要 | — | 全フェーズをブロックする土台 |
| stage1 | M1-2 ∥ M1-3 | **両方 worktree** | **M1-2 → M1-3** | `App.tsx` / `store/index.ts` / `useKeymap.ts` / `package.json` が重衝突 |
| stage2 | M1-4 | 不要 | — | M1-1〜M1-3 すべてが前提 |
| stage3 | M2-1 ∥ M3-1 | 両方 worktree | **M2-1 → M3-1** | 衝突は `main.rs` / `App.tsx` の追記のみ。M3-1 の前提は M1-3 だけなのでここに前倒し |
| stage4 | M2-2 ∥ M2-3 ∥ M3-2 | **M2-2 / M2-3 のみ** | **M2-2 → M2-3 → M3-2** | M3-2 は純フロントで衝突ゼロ（隔離不要） |
| stage5 | M3-3 → M2-4 | 不要 | 直列 | **並列にしない。** Rust/フロント両側で 10 ファイル衝突する |
| stage6 | M3-4 | 不要 | — | M1-1〜M3-3 すべてが前提 |

**stage5 は 1 メッセージ 1 起動（直列）。** M3-3 の完了を待ってから M2-4 を起動する。

---

## 3. フェーズ表

| ID | 計画ファイル | タスク | ブランチ | ゴール | 前提 | 主な実装者 |
|---|---|---|---|---|---|---|
| M1-1 | `M1-1-foundation.md` | 18 | `feat/m1-1-foundation` | アプリ再起動後もプロジェクトとセッションが復元される | なし | rust + web |
| M1-2 | `M1-2-kanban.md` | 17 | `feat/m1-2-kanban` | セッションをカンバン上で管理し、手動で状態を動かせる | M1-1 | web |
| M1-3 | `M1-3-pty-terminal.md` | 16 | `feat/m1-3-pty-terminal` | shell セッションを起動し、ターミナルで対話できる | M1-1 | rust + web |
| M1-4 | `M1-4-worktree-cli.md` | 11 | `feat/m1-4-worktree-cli` | カードから claude が選択した作業ツリーで起動し、該当ペインに直行できる | M1-1〜M1-3 | rust + web |
| M2-1 | `M2-1-runtime-state.md` | 10 | `feat/m2-1-runtime-state` | カード/タブを見るだけで各セッションの実行状態が分かる | M1-1〜M1-4 | rust + web |
| M2-2 | `M2-2-hooks-relay.md` | 19 | `feat/m2-2-hooks-relay` | Claude Code の入力待ち・完了・session_id を決定的に捕捉できる | M1-4, M2-1 | rust |
| M2-3 | `M2-3-macos-notification.md` | 16 | `feat/m2-3-notification` | アプリを見ていなくても要対応セッションに戻れる | M2-1 | rust + web |
| M2-4 | `M2-4-resume.md` | 12 | `feat/m2-4-resume` | アプリ終了後も同じ作業ツリーと会話に戻れる | M1-4, M2-1, M2-2 | rust + web |
| M3-1 | `M3-1-nvim-editor.md` | 8 | `feat/m3-1-nvim-editor` | フォーカス中セッションの作業ツリーを nvim で閲覧/編集できる | M1-3 | rust + web |
| M3-2 | `M3-2-split-layout.md` | 11 | `feat/m3-2-split-layout` | 2 つのエージェントを同時に監視し、片方へ即座に入力できる | M1-3 | web |
| M3-3 | `M3-3-generic-cli.md` | 19 | `feat/m3-3-generic-cli` | Claude Code 以外の CLI でも最低限の状態把握ができる | M1-3, M2-1, M2-2 | rust + web |
| M3-4 | `M3-4-ops-ux.md` | 15 | `feat/m3-4-ops-ux` | 要件1〜10 を満たし、日常運用で破綻しない | M1-1〜M3-3 | rust + web |

計画ファイルはすべて `docs/superpowers/plans/2026-08-01-kamux/` 配下。

---

## 4. 人間ゲート（該当フェーズを起動するときに必ず伝える）

契約 **§26** により、フロント単体 E2E（Playwright + IPC モック）を採用している。基盤は **M1-2 Task 17** が作り、M1-3 以降は各フェーズが自分のスモーク項目を仕分ける（DOM で検証できるものは `e2e/*.spec.ts` へ）。

下に残るのは **§26.4 の 4 カテゴリ = エージェントには物理的に実行できない検証**だけである。**該当フェーズの dispatch に「BLOCKED で上げること」と明記する。**

| フェーズ | 内容 | 根拠 |
|---|---|---|
| M1-4 | ビルドした `.app` を **Finder からダブルクリック**して PATH 解決を確認 | 契約 §18。`npm run tauri dev` では再現しない |
| M2-2 | 実 PTY の**対話モード**で `Notification` / `PermissionRequest` の payload をキャプチャ。hook の stdout がターミナル表示を汚さないか実測 | 契約 §12.4 は未確認事実と明記 |
| M2-3 | 通知の**クリック応答**を実機で確認 | 契約 §21（`notify-rust` を選んだ唯一の理由） |
| M3-2 | `Cmd+D` / `Cmd+[` / `Cmd+]` が WebView に奪われないか実機確認 | 奪われる場合は `Cmd+Shift+D` / `Cmd+Alt+←→` |
| M3-4 | 起動 1 秒未満 / セッション 5 個で 300MB 未満 / アイドル CPU の実測 | 契約 §0 |

---

## 5. dispatch テンプレート

`Agent` ツールを次の形で呼ぶ。**`model` は必ず明示する**（省略すると親を継承する）。

```
subagent_type: "lane-controller"
name:          "lane-<フェーズ ID 小文字>"     ← SendMessage の宛先になる
model:         "opus"
isolation:     "worktree"                      ← §2 の表で必要な場合のみ
description:   "<フェーズ ID> lane"
run_in_background: true
```

プロンプトの中身（`<>` を表の値で埋める）:

```
あなたは kamux の <ID> レーンを担当する。

計画: docs/superpowers/plans/2026-08-01-kamux/<計画ファイル>（<タスク数> タスク）
契約: docs/superpowers/plans/2026-08-01-kamux/00-contracts.md
ゴール: <ゴール>
作業ブランチ: <ブランチ>（main から切ること。main で直接作業しない）

<このフェーズが全体のどこに位置するか / 後続が何に依存するか を 2〜3 行>

実装は src-tauri/ を rust-implementer、src/ を web-implementer に投げること。
model は sonnet を明示する（設計判断を伴うタスクのみ opus）。

<人間ゲートがあれば: 「<内容> は実機確認が必要なので、着手せず BLOCKED で返すこと」>

契約の不足に気づいたら、自分で 00-contracts.md を編集せず、契約変更要求として上げること。

完了したら完了報告フォーマットで返す。
```

**過去タスクの要約や、このセッションの会話履歴を貼らない。** サブエージェントは会話を一切継承しないが、だからといって履歴を貼り付けるのは逆効果（実セッションで dispatch が 42k 文字に膨れ、99% が貼り付けた履歴だった事例がある）。渡すのは計画ファイルのパスと、上の枠内の情報だけでよい。

並列起動するステージでは、**1 回の応答の中に `Agent` 呼び出しを複数書く**。1 メッセージ 1 呼び出しは直列になる。

---

## 6. 起動後の監督

| 状況 | 対応 |
|---|---|
| 進捗を聞かれた | `TaskList` で一覧、`TaskOutput` で途中経過を覗いて要約する |
| BLOCKED が返った | 人間ゲートならユーザーに提示し、結果を `SendMessage` でレーンへ返す |
| 契約変更要求が返った | `contract-owner` を起動して裁定させる。**並列中の他レーンにも結果を周知する** |
| レーンが完了した | §2 のマージ順に従ってマージ → 下のゲートを実行 → 次のステージへ |

### ステージ完了ゲート

```bash
cd src-tauri && cargo test && cd ..
npx vitest run && npx tsc --noEmit
npm run e2e            # M1-2 Task 17 以降
```

加えて次の 2 つを確認してから次のステージに進む。

1. `contract-owner` にドリフト検査（契約 §0 の防護節 + §25.4）を走らせ、**出力が空であること**
2. lane-controller の完了報告に **「E2E に移した項目 / 手動に残した項目」** が含まれていること（契約 §26.5）。手動に残した項目は §26.4 のどのカテゴリに当たるかが書かれていること

---

## 7. やってはいけないこと

- **実装 implementer を並列に起動する**（同一ファイル衝突。レーン内は必ず直列）
- **team-lead 自身がコードを直す**（レビューを迂回し、あなたのコンテキストが汚れる）
- **`00-contracts.md` を lane-controller や implementer に編集させる**（章番号がずれると 12 計画の参照が一斉に陳腐化する）
- **人間ゲートを「たぶん通る」で ✅ にする**（headless で再現しない項目を通ったことにするのが、この計画で最も危険な失敗）
- **stage5 を並列にする**（10 ファイル衝突。得られる時間より統合コストが上回る）
