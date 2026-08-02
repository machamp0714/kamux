import { describe, expect, it } from 'vitest';
import { moveCardInOrder, type SessionOrder } from '../../store/kanbanOrder';
import {
  COLUMN_DROPPABLE_PREFIX,
  columnDroppableId,
  parseColumnDroppableId,
  resolveDragEnd,
  type DragEndResult,
} from './dragEnd';

/** テスト対象が null でないことを型ガードする。`!` を使わないための小さな補助。 */
function assertResolved(result: DragEndResult | null): DragEndResult {
  if (result === null) throw new Error('expected resolveDragEnd to return a result');
  return result;
}

function order(partial: Partial<SessionOrder>): SessionOrder {
  return {
    backlog: partial.backlog ?? [],
    in_progress: partial.in_progress ?? [],
    review: partial.review ?? [],
    done: partial.done ?? [],
  };
}

describe('columnDroppableId / parseColumnDroppableId', () => {
  it('列 id を往復変換できる', () => {
    expect(COLUMN_DROPPABLE_PREFIX).toBe('column:');
    expect(columnDroppableId('in_progress')).toBe('column:in_progress');
    expect(parseColumnDroppableId('column:in_progress')).toBe('in_progress');
  });

  it('列 id でない文字列には null を返す', () => {
    expect(parseColumnDroppableId('3f2a-9c1e')).toBeNull();
    expect(parseColumnDroppableId('column:nope')).toBeNull();
    expect(parseColumnDroppableId('')).toBeNull();
  });
});

describe('resolveDragEnd', () => {
  const board = order({ backlog: ['a', 'b', 'c'], in_progress: ['x', 'y'] });

  it('over が null なら移動しない', () => {
    expect(resolveDragEnd('a', null, board)).toBeNull();
  });

  it('別の列のカードの上に落とすとそのカードの位置へ入る', () => {
    expect(resolveDragEnd('a', 'y', board)).toEqual({ to: 'in_progress', index: 1 });
    expect(resolveDragEnd('a', 'x', board)).toEqual({ to: 'in_progress', index: 0 });
  });

  it('同じ列のカードの上に落とすとそのカードの位置へ入る', () => {
    expect(resolveDragEnd('a', 'c', board)).toEqual({ to: 'backlog', index: 2 });
    expect(resolveDragEnd('c', 'a', board)).toEqual({ to: 'backlog', index: 0 });
  });

  it('列の背景に落とすと末尾へ入る', () => {
    expect(resolveDragEnd('a', 'column:review', board)).toEqual({ to: 'review', index: 0 });
    expect(resolveDragEnd('a', 'column:in_progress', board)).toEqual({
      to: 'in_progress',
      index: 2,
    });
  });

  it('自分がいる列の背景に落とすと L.len() を超える index を返す（両側のクランプで末尾になる）', () => {
    // 契約 §49.3.2。backlog は ['a','b','c'] なので index = 3 だが、
    // 移動対象 'a' を除いた L = ['b','c'] の長さは 2。
    // moveCardInOrder（Math.min）と Store::move_session（to_index >= L.len() の枝）が
    // それぞれクランプして末尾になる。ここでクランプしないのは意図的である
    // —— 「over カードの位置」という規約を resolveDragEnd 側で崩さないため。
    expect(resolveDragEnd('a', 'column:backlog', board)).toEqual({ to: 'backlog', index: 3 });
  });

  it('同列の 3 ケース（上方向・下方向・別列）で moveCardInOrder と規約が一致する', () => {
    // 契約 §49.3.2。resolveDragEnd の index をそのまま moveCardInOrder に渡して
    // arrayMove と同じ結果になること。ここがずれると DnD が 1 つずれる。
    expect(resolveDragEnd('a', 'b', board)).toEqual({ to: 'backlog', index: 1 }); // 下方向
    expect(resolveDragEnd('c', 'b', board)).toEqual({ to: 'backlog', index: 1 }); // 上方向
    expect(resolveDragEnd('a', 'x', board)).toEqual({ to: 'in_progress', index: 0 }); // 別列
  });

  it('resolveDragEnd の index を moveCardInOrder にそのまま渡すと arrayMove と同じ並びになる', () => {
    // 契約 §49.3.2。数値の一致だけでなく、実際に moveCardInOrder へ合成した結果まで検証する。
    const down = assertResolved(resolveDragEnd('a', 'b', board)); // 下方向
    expect(moveCardInOrder(board, 'a', down.to, down.index).backlog).toEqual(['b', 'a', 'c']);

    const up = assertResolved(resolveDragEnd('c', 'b', board)); // 上方向
    expect(moveCardInOrder(board, 'c', up.to, up.index).backlog).toEqual(['a', 'c', 'b']);

    const cross = assertResolved(resolveDragEnd('a', 'x', board)); // 別列
    const afterCross = moveCardInOrder(board, 'a', cross.to, cross.index);
    expect(afterCross.backlog).toEqual(['b', 'c']);
    expect(afterCross.in_progress).toEqual(['a', 'x', 'y']);

    const own = assertResolved(resolveDragEnd('a', 'column:backlog', board)); // 自分がいる列の背景
    expect(moveCardInOrder(board, 'a', own.to, own.index).backlog).toEqual(['b', 'c', 'a']);
  });

  it('自分自身の上に落としたら移動しない', () => {
    expect(resolveDragEnd('a', 'a', board)).toBeNull();
  });

  it('どの列にも属さない over id なら移動しない', () => {
    expect(resolveDragEnd('a', 'unknown-id', board)).toBeNull();
  });
});
