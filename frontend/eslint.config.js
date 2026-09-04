import globals from 'globals'
import reactHooks from 'eslint-plugin-react-hooks'
import tseslint from 'typescript-eslint'

// Guardrail config — the point is to catch React hook-order violations
// (Rules-of-Hooks, e.g. hooks after an early return) and flag stale-closure
// effect deps, NOT a full style lint. The noisy TS/JS recommended rules stay
// OFF so the hooks signal isn't drowned out; `tsc` already enforces types.
export default [
  { ignores: ['dist'] },
  {
    files: ['src/**/*.{ts,tsx}'],
    languageOptions: {
      parser: tseslint.parser,
      ecmaVersion: 2022,
      sourceType: 'module',
      globals: globals.browser,
      parserOptions: { ecmaFeatures: { jsx: true } },
    },
    plugins: { 'react-hooks': reactHooks },
    rules: {
      'react-hooks/rules-of-hooks': 'error',
      'react-hooks/exhaustive-deps': 'error',
    },
  },
]
