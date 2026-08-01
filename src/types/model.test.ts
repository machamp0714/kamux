import { describe, expect, it } from 'vitest';

import { KANBAN_STATUSES, surfaceId } from './model';

describe('types/model', () => {
  it('surfaceId が "{sessionId}:{kind}" を返す（契約 §5）', () => {
    expect(surfaceId('3f2a-9c1e', 'agent')).toBe('3f2a-9c1e:agent');
    expect(surfaceId('3f2a-9c1e', 'editor')).toBe('3f2a-9c1e:editor');
  });

  it('KANBAN_STATUSES が契約どおりの順序で 4 列を保持する', () => {
    expect(KANBAN_STATUSES).toEqual(['backlog', 'in_progress', 'review', 'done']);
  });
});
