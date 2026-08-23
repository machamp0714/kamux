import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const notificationPermission = vi.fn();
const openNotificationSettings = vi.fn();

vi.mock('../../ipc/commands', () => ({
  notificationPermission: () => notificationPermission(),
  openNotificationSettings: () => openNotificationSettings(),
}));

import { useAppStore } from '../../store';
import { NotificationPermissionBanner } from './NotificationPermissionBanner';

describe('NotificationPermissionBanner', () => {
  afterEach(cleanup);

  beforeEach(() => {
    notificationPermission.mockReset();
    openNotificationSettings.mockReset().mockResolvedValue(undefined);
    useAppStore.setState({ lastError: null });
  });

  it('許可されているときは何も表示しない', async () => {
    notificationPermission.mockResolvedValue('granted');
    const { container } = render(<NotificationPermissionBanner />);
    await waitFor(() => expect(notificationPermission).toHaveBeenCalled());
    // 負の主張（何も描かれない）には待つべき成立条件が無いため、権限解決の反映を
    // 素の待ちで確保してから見る（契約 §69.2）。
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(container).toBeEmptyDOMElement();
  });

  it('未確定のときも表示しない', async () => {
    notificationPermission.mockResolvedValue('unknown');
    const { container } = render(<NotificationPermissionBanner />);
    await waitFor(() => expect(notificationPermission).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(container).toBeEmptyDOMElement();
  });

  it('拒否されているときは案内とバッジの説明を出す', async () => {
    notificationPermission.mockResolvedValue('denied');
    render(<NotificationPermissionBanner />);
    expect(await screen.findByRole('status')).toHaveTextContent('通知が許可されていません');
    expect(screen.getByRole('status')).toHaveTextContent('Dock バッジ');
  });

  it('ボタンでシステム設定を開く', async () => {
    notificationPermission.mockResolvedValue('denied');
    render(<NotificationPermissionBanner />);
    fireEvent.click(await screen.findByRole('button', { name: 'システム設定を開く' }));
    await waitFor(() => expect(openNotificationSettings).toHaveBeenCalledTimes(1));
  });

  it('設定を開けなかったときは握り潰さず setError でストアへ伝える', async () => {
    notificationPermission.mockResolvedValue('denied');
    openNotificationSettings.mockReset().mockRejectedValue(new Error('open failed'));
    render(<NotificationPermissionBanner />);
    fireEvent.click(await screen.findByRole('button', { name: 'システム設定を開く' }));
    await waitFor(() =>
      expect(useAppStore.getState().lastError).toEqual({
        code: 'io',
        message: 'Error: open failed',
      }),
    );
  });
});
