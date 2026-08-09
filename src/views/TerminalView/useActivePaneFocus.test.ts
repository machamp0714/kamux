import { describe, it, expect } from 'vitest';
import { focusTargetSurfaceId } from './useActivePaneFocus';
import type { PaneState } from '../../store/paneLogic';

const S = (
  layout: PaneState['layout'],
  paneAssignment: PaneState['paneAssignment'],
  activePane: PaneState['activePane'],
): PaneState => ({ layout, paneAssignment, activePane });

describe('focusTargetSurfaceId', () => {
  it('アクティブペインの agent サーフェスを返す', () => {
    expect(focusTargetSurfaceId(S('split2', ['a', 'b'], 1))).toBe('b:agent');
  });

  it('single では activePane のスロットを見る', () => {
    expect(focusTargetSurfaceId(S('single', ['a', 'b'], 0))).toBe('a:agent');
  });

  it('editor サーフェスは返さない', () => {
    expect(focusTargetSurfaceId(S('single', ['a', null], 0))).not.toContain('editor');
  });

  it('未割当なら null', () => {
    expect(focusTargetSurfaceId(S('single', [null, null], 0))).toBeNull();
  });
});
