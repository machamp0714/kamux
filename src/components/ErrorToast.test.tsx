import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';

import { APP_ERROR_LABEL, ErrorToast } from './ErrorToast';
import { useAppStore } from '../store';
import type { AppErrorCode } from '../types/model';

// AppErrorCode は src/types/model.ts の `export type AppErrorCode =` の union（7 値）。
// production の Object.keys(APP_ERROR_LABEL) を自分自身と比べる形は恒真になるため、
// ここに逐語で書き並べて突き合わせる（契約 §130.6 の観測点）。
const APP_ERROR_CODES: AppErrorCode[] = [
  'db',
  'not_found',
  'pty_spawn',
  'git',
  'cli_not_found',
  'invalid_state',
  'io',
];

afterEach(cleanup);

beforeEach(() => {
  useAppStore.setState({ lastError: null });
});

describe('APP_ERROR_LABEL', () => {
  it('AppErrorCode の 7 値すべてにラベルを持つ', () => {
    for (const code of APP_ERROR_CODES) {
      expect(typeof APP_ERROR_LABEL[code]).toBe('string');
      expect(APP_ERROR_LABEL[code].length).toBeGreaterThan(0);
    }
    expect(Object.keys(APP_ERROR_LABEL).sort()).toEqual([...APP_ERROR_CODES].sort());
  });
});

describe('ErrorToast', () => {
  it('db エラーで .error-toast__label に「データベースエラー」を描く', () => {
    useAppStore.setState({ lastError: { code: 'db', message: 'forced' } });
    render(<ErrorToast />);
    expect(screen.getByText('データベースエラー')).toHaveClass('error-toast__label');
  });

  it('.error-toast__code / .error-toast__message は 1 文字も変わらない（e2e が逐語で見ている）', () => {
    useAppStore.setState({ lastError: { code: 'db', message: 'forced' } });
    const { container } = render(<ErrorToast />);
    expect(container.querySelector('.error-toast__code')?.textContent).toBe('db');
    expect(container.querySelector('.error-toast__message')?.textContent).toBe('forced');
  });

  it('対応表に無い code のときは .error-toast__label を 1 つも描かない', () => {
    useAppStore.setState({
      lastError: { code: 'unknown_code' as AppErrorCode, message: 'forced' },
    });
    const { container } = render(<ErrorToast />);
    expect(container.querySelector('.error-toast__label')).toBeNull();
  });
});
