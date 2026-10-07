// @ts-check
import antfu from '@antfu/eslint-config'

export default antfu(
  {
    formatters: true,
    rules: {
      'style/eol-last': ['error', 'always'],
      // The WebView console is the only log sink on Android (it lands in logcat)
      // and the primary debugging channel on desktop, so the lifecycle logs in
      // src/composables/* are intentional. Keep them, but still reject the
      // rarely-used console APIs (trace/dir/table/group/time...).
      'no-console': ['error', { allow: ['log', 'info', 'debug', 'warn', 'error'] }],
    },
    ignores: [
      'src-tauri/**',
    ],
  },
  {
    // `vue-router/auto-routes` is a virtual module provided by
    // unplugin-vue-router. ESLint's node resolver maps it onto the same
    // physical file as `vue-router/auto`, so `import/no-duplicates` reports a
    // false positive and its autofix merges the two imports — which drops
    // `routes` from the bundle's import graph and breaks `createRouter`.
    files: ['src/main.ts'],
    rules: {
      'import/no-duplicates': 'off',
    },
  },
)
