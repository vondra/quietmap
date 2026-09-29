// Lint: typescript-eslint's recommended rules for the server.
import { defineConfig } from 'eslint/config'
import tseslint from 'typescript-eslint'

export default defineConfig([
  {
    files: ['**/*.ts'],
    extends: [tseslint.configs.recommended],
  },
])
