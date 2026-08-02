import { describe, expect, it } from 'vitest';
import { KANBAN_KEYBOARD_CODE_STRINGS, KANBAN_POINTER_ACTIVATION_DISTANCE } from './sensors';

describe('KANBAN_KEYBOARD_CODE_STRINGS', () => {
  it('ドラッグの開始・終了は Space のみ', () => {
    expect(KANBAN_KEYBOARD_CODE_STRINGS.start).toEqual(['Space']);
    expect(KANBAN_KEYBOARD_CODE_STRINGS.end).toEqual(['Space']);
  });

  it('Escape でキャンセルできる', () => {
    expect(KANBAN_KEYBOARD_CODE_STRINGS.cancel).toEqual(['Escape']);
  });

  it('Enter を DnD に使わない（契約 §11 で M1-4 の focusSession に予約済み）', () => {
    expect(KANBAN_KEYBOARD_CODE_STRINGS.start).not.toContain('Enter');
    expect(KANBAN_KEYBOARD_CODE_STRINGS.end).not.toContain('Enter');
  });
});

describe('KANBAN_POINTER_ACTIVATION_DISTANCE', () => {
  it('クリックがドラッグに飲まれないよう距離のしきい値を持つ', () => {
    expect(KANBAN_POINTER_ACTIVATION_DISTANCE).toBe(5);
  });
});
