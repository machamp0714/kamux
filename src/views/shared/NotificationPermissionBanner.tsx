import { useEffect, useState } from 'react';

import { notificationPermission, openNotificationSettings } from '../../ipc/commands';
import type { NotifyPermission } from '../../types/model';

/**
 * 通知が拒否されているときだけ出る案内。
 *
 * 一度 Denied になったアプリに再プロンプトを出しても macOS は何も表示しないため、
 * 「再要求」ボタンではなくシステム設定への導線を出す。
 */
export function NotificationPermissionBanner() {
  const [permission, setPermission] = useState<NotifyPermission>('unknown');

  useEffect(() => {
    let alive = true;
    void notificationPermission().then((p) => {
      if (alive) setPermission(p);
    });
    return () => {
      alive = false;
    };
  }, []);

  if (permission !== 'denied') return null;

  return (
    <div role="status" className="notification-permission-banner">
      <span>
        通知が許可されていません。要対応セッションは Dock バッジとカードのバッジで確認できます。
      </span>
      <button type="button" onClick={() => void openNotificationSettings()}>
        システム設定を開く
      </button>
    </div>
  );
}
