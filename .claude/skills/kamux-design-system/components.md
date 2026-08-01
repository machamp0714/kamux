# 部品ごとの寸法と役割

`docs/design/kamux-ui.pen` の各画面から起こしたもの。`px` で書いてある箇所は対応する `--space-*` / `--text-*` トークンに置き換えて実装する。

クラス名は契約 `00-contracts.md` §25.3 が正典。ここに書いてある名前と食い違ったら**契約が勝つ**。

---

## カード（`kanban-card`）

縦積み。`gap: var(--space-4)` / `padding: var(--space-6)` / `background: var(--bg-elevated)` / `border: 1px solid var(--border)` / `border-radius: 8px`。

| 子要素 | 中身 | トークン |
|---|---|---|
| `kanban-card__head` | 横並び・両端寄せ。左に CLI チップ、右にバッジ | `gap: var(--space-4)` |
| `kanban-card__cli` | アイコン 11px + 名前 | `--text-2xs` / `--font-mono` / `--text-secondary` / 背景 `--bg-hover` / `--radius-sm` / padding `2px 6px` |
| `kanban-card__badge` | `<RuntimeBadge sessionId>` を置くだけ | — |
| `kanban-card__title` | 2 行で折り返す | `--text-lg` / 600 / `--leading-tight` / `--text-primary` |
| `kanban-card__description` | 補足 | `--text-sm` / `--leading-normal` / `--text-muted` |
| `kanban-card__branch` | git アイコン 12px + ブランチ名。worktree のときだけ | `--text-2xs` / `--font-mono` / `--text-muted` |
| `kanban-card__actions` | ホバーで出す。主 + 副 + overflow | 下の「ボタン」参照 |
| `kanban-card__error` | `runtime_state === 'error'` のときだけ。生 stderr を原文で | `--text-2xs` / `--font-mono` / `--state-error` / 枠 `1px solid var(--state-error)` / 背景 `--bg-app` |

**ホバー**: `background: var(--bg-hover)` + `border-color: var(--border-strong)`。影は付けない。
**エラー**: カード自体の `border-color` を `--state-error` にする。

---

## 実行状態バッジ（`runtime-badge`）

横並び・`gap: var(--space-3)`・背景なし・枠なし。ピル型にしない。

| 子要素 | 仕様 |
|---|---|
| `runtime-badge__dot` | 8×8 円。`background: var(--state-*)` |
| `runtime-badge__label` | `--text-xs` / 500 / `letter-spacing: 0.2px` / 色は同じ `--state-*` |

| 状態 | トークン | ラベル |
|---|---|---|
| `running` | `--state-running` | `running` |
| `waiting_input` | `--state-waiting` | `waiting` |
| `idle` | `--state-idle` | `idle` |
| `exited` | `--state-exited` | `exited` |
| `interrupted` | `--state-interrupted` | `interrupted` |
| `error` | `--state-error` | `error` |

`runtime-badge--estimated`: ドットを中空（`background: transparent` + `border: 1.5px solid var(--state-*)`）にし、ラベルに `~` を前置。**色は変えない。** アニメーションは付けない（契約 §0「アイドル時 CPU ほぼ 0%」）。

---

## セッションタブ（`kamux-tab`）

縦 2 段。`padding: var(--space-4) var(--space-5)` / `border-radius: var(--radius-md)` / 幅は親いっぱい。

- 1 段目 `kamux-tab__title`: `--text-sm` / 500 / `--leading-tight` / `--text-primary`。タスク名がそのままタブ名（設計書 要件6）
- 2 段目: 左に `runtime-badge` と `kamux-tab__cli`（`--text-2xs` / `--font-mono` / `--text-muted`）、右に `kamux-tab__pane-badge`

`kamux-tab__pane-badge`: `P1` / `P2`。`--text-2xs` / `--font-mono` / 枠 `1px solid var(--border-strong)` / `border-radius: 3px`。割当先ペインがフォーカス中なら枠と文字を `--accent` に。

**選択中のタブ**: `background: var(--accent-soft)` + 左端に幅 3px・高さ 30px・`--accent` のバー（絶対配置）。

グループ見出し（`kamux-tablist__group-label`、契約 §29.7 の SESSIONS / SCRATCH）: `--text-sm` / 600 / `letter-spacing: 0.6px` / `--text-secondary`。

---

## ボタン

| 種別 | 背景 | 文字 | 枠 | 用途 |
|---|---|---|---|---|
| 主 | `--accent` | `--bg-surface` | なし | 画面に 1 つだけ。「作成して即開始」など |
| 主（控えめ） | `--accent-soft` | `--accent` | なし | カード内の「開く」など、周囲より一段軽い主操作 |
| 副 | `--bg-hover` | `--text-primary` | `1px solid var(--border)` | 「やめる」「Create」 |
| ゴースト | なし | `--text-muted` | なし | 「Cancel」 |
| 破壊的 | `--bg-app` | `--state-error` | `1px solid var(--state-error)` | 「削除する」「停止する」 |
| 無効 | `--bg-app` | `--text-muted` | `1px solid var(--border)` | 前提条件が未達のとき |

共通: `padding: var(--space-4) var(--space-7)` / `border-radius: var(--radius-md)` / `--text-sm` / 500〜600。
キーヒント（`⌘⏎` など）を添えるときは `--font-mono` / `--text-2xs` / `opacity: 0.75`。

**破壊的ボタンを塗り + 白文字にしない。** 白文字 4.5:1 とパネル面 3:1 を同時に満たす赤の帯域が狭く、テーマ間で必ずどちらかを割る。枠 + 文字色で表す。

---

## 入力欄

`height: 36px` / `padding: 0 var(--space-5)` / `border-radius: var(--radius-md)` / `background: var(--bg-app)` / `border: 1px solid var(--border-input)`。
**`--border` ではなく `--border-input` を使う。** 入力欄の境界は装飾ではなく操作可能であることの唯一の手がかりなので、3:1 が要る（WCAG 1.4.11）。

フォーカス: `border: 2px solid var(--accent)`。
値は `--text-md`、`--text-primary`。パスやブランチ名は `--font-mono`。
ラベル: `--text-xs` / 600 / `letter-spacing: 0.3px` / `--text-secondary`。補足は右端に `--text-2xs` / `--text-muted`。

---

## モーダル・ダイアログ

スクリム: `#0a0b12b3`（テーマ非依存）。

パネル: `background: var(--bg-elevated)` / `border: 1px solid var(--border-strong)` / `border-radius: 12px` / `box-shadow: var(--shadow-modal)`。幅は 500〜600px。

3 段構成。

| 段 | 仕様 |
|---|---|
| header | `padding: var(--space-8) var(--space-9)` / 下に `1px solid var(--border)` / 見出し `--text-xl` 600 |
| body | `padding: var(--space-9)` / `gap: var(--space-9)` |
| footer | `padding: var(--space-7) var(--space-9)` / `background: var(--bg-surface)` / 上に `1px solid var(--border)` / ボタンは右寄せ `gap: var(--space-4)` |

**破壊的な確認ダイアログ**: 左肩に 32×32 のアイコン枠（`--bg-app` / `1px solid var(--state-error)` / `--radius-md`、アイコンは `--state-error`）。危険の内訳は `--bg-app` 背景 + `1px solid var(--state-error)` のブロックに入れる。
「安心情報」を緑で出さない（`--state-running` と衝突する）。`--text-muted` の通常テキストで足りる。
不可逆操作は明示的なチェックボックスを通す。チェックが入るまで実行ボタンは無効。

---

## トースト

幅 468px、右下に縦積み `gap: var(--space-5)`。
`background: var(--bg-elevated)` / `border: 1px solid var(--state-error)`（重要度で `--state-waiting` にも）/ `border-radius: var(--radius-lg)` / `box-shadow: var(--shadow-toast)`。

stderr は**原文のまま**、`background: var(--bg-terminal)` のブロックに `--font-mono` / `--text-2xs` / `--text-term-dim` で出す（契約 §12）。

---

## パネル・表

パネル: `background: var(--bg-surface)` / `border: 1px solid var(--border)` / `border-radius: var(--radius-lg)` / `overflow: hidden`。
行の区切りは `border-bottom: 1px solid var(--border)`（最終行には付けない）。行の padding は `var(--space-5) var(--space-7)`。
表ヘッダ: `background: var(--bg-app)` / `--text-2xs` / 600 / `letter-spacing: 0.5px` / `--text-muted`。

---

## 空状態

列やパネルの中央に縦積み `gap: var(--space-7)`。
アイコン 28px（`--text-muted`）→ 見出し `--text-md` 600 → 説明 `--text-xs` / `--leading-normal` / `--text-muted` → 主ボタン。
説明は「何ができるか」を書く。「データがありません」で終わらせない。

---

## ターミナル面

`background: var(--bg-terminal)`（**テーマ非依存。ライトテーマでも暗いまま**）。
`--font-mono` / `--text-xs`〜`--text-sm` / `--leading-term` / 既定色 `--text-term`、副次 `--text-term-dim`。
出力の色分けは `--term-cyan`（ツール実行）/ `--term-green`（追加・成功）/ `--term-red`（削除・失敗）/ `--term-yellow`（確認待ち）/ `--term-magenta`（進行中）/ `--term-comment`（メタ情報）。

ペインヘッダ: 高さ 38〜42px / `background: var(--bg-surface)` / 下に `1px solid var(--border)`。
**フォーカス中のペイン**: ペイン枠を `2px solid var(--accent)`、ヘッダ背景を `--bg-elevated` に。分割時にどちらへキー入力が行くかを示す唯一の手がかりなので省略しない。
