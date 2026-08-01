import { describe, expect, it } from 'vitest';
import { KANBAN_STATUSES } from '../../types/model';
import { COLUMN_LABELS, isKanbanStatus } from './columns';

describe('COLUMN_LABELS', () => {
  it('契約 §2 の 4 値すべてに表示ラベルを持つ', () => {
    expect(COLUMN_LABELS).toEqual({
      backlog: 'Backlog',
      in_progress: 'In Progress',
      review: 'Review',
      done: 'Done',
    });
  });

  it('KANBAN_STATUSES と過不足なく対応する', () => {
    expect(Object.keys(COLUMN_LABELS).sort()).toEqual([...KANBAN_STATUSES].sort());
  });
});

describe('isKanbanStatus', () => {
  it('契約 §2 の値を受理する', () => {
    for (const status of KANBAN_STATUSES) {
      expect(isKanbanStatus(status)).toBe(true);
    }
  });

  it('契約 §22 の禁止名や未知の値を拒否する', () => {
    expect(isKanbanStatus('column')).toBe(false);
    expect(isKanbanStatus('inProgress')).toBe(false);
    expect(isKanbanStatus('')).toBe(false);
  });
});
