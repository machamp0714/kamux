//! 汎用 CLI 向けベストエフォート検知（設計書 §9.2 / §12）。
//!
//! hooks が使えない CLI、および hooks が届かない claude セッションに対して、
//! BEL 検知と沈黙タイムアウトから `RuntimeState` を推定する。
//! ここで導出される状態は必ず `gate::heuristic_transition` を通り、
//! hook 由来の権威ある遷移を上書きしない。

pub mod bel;
pub mod clock;
pub mod silence;

/// 沈黙タイムアウトの既定値（秒）。設計書 §9.2「既定 30 秒」
pub const DEFAULT_SILENCE_TIMEOUT_SECS: u32 = 30;
/// ユーザーが設定できる下限。0 を許すとウォッチャが busy loop になるため構造的に禁じる
pub const MIN_SILENCE_TIMEOUT_SECS: u32 = 5;
/// ユーザーが設定できる上限（1 時間）
pub const MAX_SILENCE_TIMEOUT_SECS: u32 = 3600;
/// この窓の中で連続した BEL は 1 件に丸める（ms）
pub const BEL_DEBOUNCE_MS: i64 = 1_000;
/// claude セッションで hook を待つ猶予（ms）。これを過ぎたら hooks 不達と判定する。
/// `DEFAULT_SILENCE_TIMEOUT_SECS` より短いことが重要（設計 §4.7）
pub const HOOK_GRACE_MS: i64 = 20_000;

// `HOOK_GRACE_MS` は `DEFAULT_SILENCE_TIMEOUT_SECS` より短くなければならない
// （設計 §4.7）。この順序が崩れると「猶予切れ → 沈黙推定の発火」という
// hooks 不達判定の前提が壊れるため、値の変更時にビルドで検知する。
const _: () = assert!(HOOK_GRACE_MS < DEFAULT_SILENCE_TIMEOUT_SECS as i64 * 1000);
