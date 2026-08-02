import { defineConfig } from '@playwright/test';

// 契約 §26.2 / §26.3: フロント単体 E2E。Vite dev server を実ブラウザ（chromium 1 種）で駆動し、
// Tauri IPC は e2e/support/tauriMock.ts でモックする。複数エンジンを回す価値は無い
// （chromium はどのみち実機の WKWebView とは別エンジン）。
export default defineConfig({
  testDir: './e2e',
  timeout: 15_000,
  fullyParallel: true,
  use: { baseURL: 'http://localhost:1420', trace: 'retain-on-failure' },
  projects: [{ name: 'chromium', use: { browserName: 'chromium' } }],
  webServer: {
    command: 'npm run dev',
    url: 'http://localhost:1420',
    // 開発中に手元の dev server と衝突しないよう既存サーバの再利用を許す
    reuseExistingServer: true,
    timeout: 60_000,
  },
});
