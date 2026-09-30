import { resolve } from 'path'
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import dts from "vite-plugin-dts"
import ViteYaml from '@modyfi/vite-plugin-yaml';

// https://vite.dev/config/
export default defineConfig({
  plugins: [vue(), dts({
    tsconfigPath: './tsconfig.app.json',
  }), ViteYaml()],
  build: {
    lib: {
      entry: resolve(__dirname, 'src/easytier-frontend-lib.ts'),
      name: 'easytier-frontend-lib',
      fileName: 'easytier-frontend-lib',
      formats: ["es", "umd", "cjs"],
    },
    rollupOptions: {
      // make sure to externalize deps that shouldn't be bundled
      // into your library
      external: ['vue', 'primevue'],
      output: {
        // Provide global variables to use in the UMD build
        // for externalized deps
        globals: {
          vue: 'Vue',
          primevue: 'primevue',
        },
        exports: "named"
      },
    },
  },
})
