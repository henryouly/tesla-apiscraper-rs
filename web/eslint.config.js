import js from '@eslint/js'
import tseslint from '@typescript-eslint/eslint-plugin'
import tsparser from '@typescript-eslint/parser'
import solid from 'eslint-plugin-solid'
import globals from 'globals'

export default [
  js.configs.recommended,
  {
    files: ['src/**/*.{ts,tsx}'],
    languageOptions: {
      parser: tsparser,
      parserOptions: { ecmaFeatures: { jsx: true } },
      globals: globals.browser,
    },
    plugins: {
      '@typescript-eslint': tseslint,
      solid,
    },
    rules: {
      ...(solid.configs['flat/recommended']?.rules ?? {}),
      'no-unused-vars': 'off',
      // TypeScript (tsc) already validates names; the base rule
      // false-positives on DOM lib types like RequestInit.
      'no-undef': 'off',
    },
  },
]
