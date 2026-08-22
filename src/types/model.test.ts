import { describe, expect, it } from 'vitest';

import { KANBAN_STATUSES, surfaceId, VIEW_KINDS } from './model';

describe('types/model', () => {
  it('surfaceId が "{sessionId}:{kind}" を返す（契約 §5）', () => {
    expect(surfaceId('3f2a-9c1e', 'agent')).toBe('3f2a-9c1e:agent');
    expect(surfaceId('3f2a-9c1e', 'editor')).toBe('3f2a-9c1e:editor');
  });

  it('KANBAN_STATUSES が契約どおりの順序で 4 列を保持する', () => {
    expect(KANBAN_STATUSES).toEqual(['backlog', 'in_progress', 'review', 'done']);
  });

  it('VIEW_KINDS が Rust ViewKind（契約 §7.2 / policy.rs の Kanban/Terminal/Editor）と同じ3値を保持する', () => {
    expect(VIEW_KINDS).toEqual(['kanban', 'terminal', 'editor']);
  });
});
