// vitest の setupFiles（vite.config.ts の test.setupFiles）から読まれる。
// toHaveAttribute / toHaveAccessibleName / toHaveTextContent などの
// jest-dom マッチャを expect に登録する。
import '@testing-library/jest-dom/vitest';
