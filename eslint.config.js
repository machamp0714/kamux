import js from '@eslint/js';
import reactHooks from 'eslint-plugin-react-hooks';
import tseslint from 'typescript-eslint';

export default tseslint.config(
  {
    ignores: [
      'dist/',
      'src-tauri/target/',
      'playwright-report/',
      'test-results/',
      // ESLint 9 の flat config は .gitignore を読まず、既定でドットディレクトリも
      // 走査対象にする。このリポジトリ直下には docs/（契約・計画）、.claude/、.agents/、
      // .remember/ などリンター対象外のツリーがあるため明示的に除外する。
      'docs/',
      '.claude/',
      '.agents/',
      '.remember/',
      'CLAUDE.md',
      'AGENTS.md',
      'skills-lock.json',
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  {
    files: ['**/*.{ts,tsx}'],
    plugins: { 'react-hooks': reactHooks },
    rules: {
      ...reactHooks.configs.recommended.rules,
      // 契約 §0「unwrap() を使った panic 経路を作らない」のフロント版。
      // 非 null アサーションは型の嘘であり、実行時に silent に壊れる。
      '@typescript-eslint/no-non-null-assertion': 'error',
      '@typescript-eslint/no-explicit-any': 'error',
      // 未使用引数は _ 始まりで明示的に許可する
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_' }],
    },
  },
);
