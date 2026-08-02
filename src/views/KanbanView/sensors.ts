import type { KeyboardCodes } from '@dnd-kit/core';

/**
 * dnd-kit KeyboardSensor のキー割り当て（第1部 判断 7）。
 *
 * Enter は契約 §11 で「カード上の Enter = focusSession(id, 'terminal')」（M1-4）に
 * 予約済み。ライブラリの既定値に委ねると M1-4 で無言の衝突が起きるため、
 * start / end を Space のみに固定する。
 *
 * 値の唯一の出所はこの定数で、テストもこちらを見る。
 */
export const KANBAN_KEYBOARD_CODE_STRINGS = {
  start: ['Space'],
  cancel: ['Escape'],
  end: ['Space'],
};

// dnd-kit の KeyboardCode は string enum なので、リテラル配列からは直接代入できない。
export const KANBAN_KEYBOARD_CODES = KANBAN_KEYBOARD_CODE_STRINGS as unknown as KeyboardCodes;

/** この距離だけポインタが動いて初めてドラッグを開始する。 */
export const KANBAN_POINTER_ACTIVATION_DISTANCE = 5;
