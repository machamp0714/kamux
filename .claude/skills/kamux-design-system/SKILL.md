---
name: kamux-design-system
description: kamux のフロントエンドで見た目を作るときに使う。CSS ファイルを新規に書くとき、既存コンポーネントの見た目を変えるとき、色・余白・文字サイズ・角丸・影・フォント・テーマ切替を決める必要があるとき、破壊的操作のボタンや状態表示の色を選ぶときに起動する。
---

# kamux デザインシステム

kamux の見た目は `docs/design/kamux-ui.pen`（Pencil）で確定済みで、その値は `src/styles/tokens.css` に落ちている。**あなたの仕事は値を決めることではなく、決まっている値を当てはめること。**

## 核心

CSS を書く前に `tokens.css` を読む。**色・余白・文字サイズ・角丸・影・フォントの literal を書かない。**

この 1 行を守れば残りはほぼ付いてくる。破られ方が 3 通りあるので、それぞれ下で潰す。

## CSS ファイルの形

コンポーネントの `.css` は**この形をしている**。

```css
/* 1. トークンを参照するだけ。定義しない */
.kanban-card {
  display: flex;
  flex-direction: column;
  gap: var(--space-4);
  padding: var(--space-6);
  background: var(--bg-elevated);
  border: 1px solid var(--border);
  border-radius: var(--radius-card);
  color: var(--text-primary);
  font-family: var(--font-ui);
}

/* 2. 状態変化もトークンで表す */
.kanban-card:hover {
  background: var(--bg-hover);
  border-color: var(--border-strong);
}

/* 3. テーマ分岐は書かない。tokens.css が var の中身を差し替える */
```

含まれないものが 3 つある。

| 書かないもの | 理由 |
|---|---|
| `--kc-bg` のようなコンポーネント固有の色変数 | 部品ごとに孤立したパレットができる。3 体の実装者が独立にこれをやって、互換性のないトークン島が 3 つできた実績がある |
| `@media (prefers-color-scheme: ...)` | テーマ機構は `tokens.css` が 1 箇所で持つ。コンポーネント側に書くと切替方式が部品ごとに散る |
| `-apple-system` などのフォントスタック | `var(--font-ui)` / `var(--font-mono)` に日本語フォールバックが入っている。直書きすると日本語が別フォントになる |

## トークン早見表

| 用途 | トークン |
|---|---|
| 画面の地 / パネル / カード / ホバー | `--bg-app` / `--bg-surface` / `--bg-elevated` / `--bg-hover` |
| 区切り線 / 強い境界 / **入力欄の境界** | `--border` / `--border-strong` / `--border-input` |
| 本文 / 副次 / 補足 | `--text-primary` / `--text-secondary` / `--text-muted` |
| 主操作 | `--accent`（文字）+ `--accent-soft`（背景） |
| 実行状態 6 値 | `--state-running` `--state-waiting` `--state-idle` `--state-exited` `--state-interrupted` `--state-error` |
| ターミナル面（テーマ非依存） | `--bg-terminal` `--text-term` `--text-term-dim` `--term-*` |
| 余白 | `--space-1`(2px) 〜 `--space-10`(24px) |
| 文字 | `--text-2xs`(10) 〜 `--text-2xl`(22)、`--leading-tight/normal/term` |
| 角丸 | `--radius-sm`(4) `--radius-md`(6) `--radius-lg`(10) `--radius-card`(8) `--radius-modal`(12) |
| モーダルの背面 | `--scrim` |
| 重なり順 | `--z-scrim` `--z-toast` `--z-tooltip`。`z-index` に数値を直接書かない |

**色の系統は 2 つしかない —— 実行状態（6 色）と accent。3 つ目を作らない。**
CLI 種別ごとの色分け、成功/注意を表す緑や黄、破壊的操作専用の赤を新設すると、必ず実行状態の 6 色と衝突する。破壊的操作は `--state-error` を使う。

## 既に設計されている部品

`components.md` に、`.pen` の各画面から起こした寸法と役割の対応がある。**カード・バッジ・タブ・ボタン・入力欄・モーダル・トースト・空状態を作るなら、自分で決める前にそれを読む。**

読まずに決めると、既に解いてある問題を解き直して劣化させる。実例: 主ボタンをアクセント色の塗り+白文字にしようとしてコントラストが 2.4:1 にしかならず、主/副の区別自体を諦めた実装があった。設計は `--accent-soft` 背景 + `--accent` 文字で 4.9:1 を取っている。

## 設計に無い部品を作るとき

この順で決める。**自分の美意識を持ち込む余地は最後の 1 段しかない。**

1. `components.md` で**似た役割の部品**を探す。確認ダイアログなら「モーダル」、通知バーなら「トースト」
2. その寸法と構造をそのまま使う
3. 足りない部分だけ、トークン早見表の中から役割が一致するものを選ぶ
4. **トークンに無い値が要ると判断したら、そこで止めて相談する。** 勝手に足さない

## 実行状態バッジ

契約 §2 の 6 値。**5 値ではない**（`error` を落とさない）。

- 色だけで状態を示さない。**必ずドット + テキストラベル**
- `runtime-badge--estimated`（ヒューリスティクス推定）は中空ドット + `~` 前置。色は変えない
- `runtimeStates[sessionId]` が `undefined`（未起動セッション）なら**何も描画しない**

## 検証

CSS を書き終えたら 2 つ走らせる。

```bash
# 1. トークン以外の色・サイズ literal が残っていないか
grep -rnE '#[0-9a-fA-F]{3,8}|rgba?\(|[0-9]+px|[0-9.]+rem' src --include='*.css' | grep -v 'tokens.css'

# 2. トークン自体のコントラスト（トークンを更新したときだけ）
node .claude/skills/kamux-design-system/verify-contrast.mjs
```

1 でヒットしてよいのは次の 2 つだけ。**色は 1 つも残らない。**

- `border` / `outline` プロパティ上の幅（`1px` `1.5px` `2px`）。線の太さは尺度ではないのでトークン化しない
- `components.md` が px で直接指定している部品固有の寸法（アイコン枠 32×32、パネル幅 520px、ペインヘッダ高さ 38px など）。これらも尺度ではなく部品の寸法

角丸・スクリム・余白・文字サイズがヒットしたら、対応するトークンがある。2 は全 88 組み合わせが 4.5:1（UI 要素は 3:1）を満たすことを確認する。

## やってしまいがちなこと

| やること | 実際 |
|---|---|
| 「CSS 変数を使ったから準拠している」 | 自分で定義した `--ssd-*` は準拠ではない。`tokens.css` の変数だけが準拠 |
| 「この部品には専用の色が要る」 | 要らない。系統は 2 つだけ |
| 「ダークとライトで別の色にしたい」 | `tokens.css` が既に両方持っている。コンポーネント側で分岐しない |
| 「トークンに無いから近い値を書く」 | 止めて相談する。近い値を書くと次の人がまた別の近い値を書く |
| 「設計ファイルは見られないから自分で決める」 | `components.md` に落ちている。`.pen` を開く必要はない |

## トークンを更新する

`tokens.css` を手で書き換えない。`.pen` が正典。

1. Pencil で `docs/design/kamux-ui.pen` を開く
2. `mcp__pencil__get_variables` で 34 変数を取得
3. `tokens.css` の該当ブロックに反映（`--space-*` `--text-*` `--shadow-*` `--dur-*` は `.pen` に無い集約値なので手を触れない）
4. `node .claude/skills/kamux-design-system/verify-contrast.mjs` が通ることを確認

**コントラストを割る値は入れない。** ライトテーマは `.pen` の値をそのまま使うと 23 組み合わせが 4.5:1 を割ったため、色相と彩度を保ったまま明度だけ落として補正してある。`.pen` 側の値とは意図的に差がある。
