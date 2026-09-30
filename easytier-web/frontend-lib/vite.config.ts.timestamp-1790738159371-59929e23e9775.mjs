// vite.config.ts
import { resolve } from "path";
import { defineConfig } from "file:///D:/EasyTier/node_modules/.pnpm/vite@5.4.21_@types+node@22.18.1/node_modules/vite/dist/node/index.js";
import vue from "file:///D:/EasyTier/node_modules/.pnpm/@vitejs+plugin-vue@5.2.4_vite@5.4.21_@types+node@22.18.1__vue@3.5.21_typescript@5.6.3_/node_modules/@vitejs/plugin-vue/dist/index.mjs";
import dts from "file:///D:/EasyTier/node_modules/.pnpm/vite-plugin-dts@4.5.4_@types+node@22.18.1_rollup@4.50.1_typescript@5.6.3_vite@5.4.21_@types+node@22.18.1_/node_modules/vite-plugin-dts/dist/index.mjs";
import ViteYaml from "file:///D:/EasyTier/node_modules/.pnpm/@modyfi+vite-plugin-yaml@1.1.1_rollup@4.50.1_vite@5.4.21_@types+node@22.18.1_/node_modules/@modyfi/vite-plugin-yaml/dist/index.js";
var __vite_injected_original_dirname = "D:\\EasyTier\\easytier-web\\frontend-lib";
var vite_config_default = defineConfig({
  plugins: [vue(), dts({
    tsconfigPath: "./tsconfig.app.json"
  }), ViteYaml()],
  build: {
    lib: {
      // Could also be a dictionary or array of multiple entry points
      entry: resolve(__vite_injected_original_dirname, "src/index.ts"),
      name: "easytier-frontend-lib",
      // the proper extensions will be added
      fileName: "easytier-frontend-lib",
      formats: ["es", "umd", "cjs"]
    },
    rollupOptions: {
      input: {
        main: resolve(__vite_injected_original_dirname, "src/easytier-frontend-lib.ts")
      },
      // make sure to externalize deps that shouldn't be bundled
      // into your library
      external: ["vue", "primevue"],
      output: {
        // Provide global variables to use in the UMD build
        // for externalized deps
        globals: {
          vue: "Vue",
          primevue: "primevue"
        },
        exports: "named"
      }
    }
  }
});
export {
  vite_config_default as default
};
//# sourceMappingURL=data:application/json;base64,ewogICJ2ZXJzaW9uIjogMywKICAic291cmNlcyI6IFsidml0ZS5jb25maWcudHMiXSwKICAic291cmNlc0NvbnRlbnQiOiBbImNvbnN0IF9fdml0ZV9pbmplY3RlZF9vcmlnaW5hbF9kaXJuYW1lID0gXCJEOlxcXFxFYXN5VGllclxcXFxlYXN5dGllci13ZWJcXFxcZnJvbnRlbmQtbGliXCI7Y29uc3QgX192aXRlX2luamVjdGVkX29yaWdpbmFsX2ZpbGVuYW1lID0gXCJEOlxcXFxFYXN5VGllclxcXFxlYXN5dGllci13ZWJcXFxcZnJvbnRlbmQtbGliXFxcXHZpdGUuY29uZmlnLnRzXCI7Y29uc3QgX192aXRlX2luamVjdGVkX29yaWdpbmFsX2ltcG9ydF9tZXRhX3VybCA9IFwiZmlsZTovLy9EOi9FYXN5VGllci9lYXN5dGllci13ZWIvZnJvbnRlbmQtbGliL3ZpdGUuY29uZmlnLnRzXCI7aW1wb3J0IHsgcmVzb2x2ZSB9IGZyb20gJ3BhdGgnXG5pbXBvcnQgeyBkZWZpbmVDb25maWcgfSBmcm9tICd2aXRlJ1xuaW1wb3J0IHZ1ZSBmcm9tICdAdml0ZWpzL3BsdWdpbi12dWUnXG5pbXBvcnQgZHRzIGZyb20gXCJ2aXRlLXBsdWdpbi1kdHNcIlxuaW1wb3J0IFZpdGVZYW1sIGZyb20gJ0Btb2R5Zmkvdml0ZS1wbHVnaW4teWFtbCc7XG5cbi8vIGh0dHBzOi8vdml0ZS5kZXYvY29uZmlnL1xuZXhwb3J0IGRlZmF1bHQgZGVmaW5lQ29uZmlnKHtcbiAgcGx1Z2luczogW3Z1ZSgpLCBkdHMoe1xuICAgIHRzY29uZmlnUGF0aDogJy4vdHNjb25maWcuYXBwLmpzb24nLFxuICB9KSwgVml0ZVlhbWwoKV0sXG4gIGJ1aWxkOiB7XG4gICAgbGliOiB7XG4gICAgICAvLyBDb3VsZCBhbHNvIGJlIGEgZGljdGlvbmFyeSBvciBhcnJheSBvZiBtdWx0aXBsZSBlbnRyeSBwb2ludHNcbiAgICAgIGVudHJ5OiByZXNvbHZlKF9fZGlybmFtZSwgJ3NyYy9pbmRleC50cycpLFxuICAgICAgbmFtZTogJ2Vhc3l0aWVyLWZyb250ZW5kLWxpYicsXG4gICAgICAvLyB0aGUgcHJvcGVyIGV4dGVuc2lvbnMgd2lsbCBiZSBhZGRlZFxuICAgICAgZmlsZU5hbWU6ICdlYXN5dGllci1mcm9udGVuZC1saWInLFxuICAgICAgZm9ybWF0czogW1wiZXNcIiwgXCJ1bWRcIiwgXCJjanNcIl0sXG4gICAgfSxcbiAgICByb2xsdXBPcHRpb25zOiB7XG4gICAgICBpbnB1dDoge1xuICAgICAgICBtYWluOiByZXNvbHZlKF9fZGlybmFtZSwgXCJzcmMvZWFzeXRpZXItZnJvbnRlbmQtbGliLnRzXCIpXG4gICAgICB9LFxuICAgICAgLy8gbWFrZSBzdXJlIHRvIGV4dGVybmFsaXplIGRlcHMgdGhhdCBzaG91bGRuJ3QgYmUgYnVuZGxlZFxuICAgICAgLy8gaW50byB5b3VyIGxpYnJhcnlcbiAgICAgIGV4dGVybmFsOiBbJ3Z1ZScsICdwcmltZXZ1ZSddLFxuICAgICAgb3V0cHV0OiB7XG4gICAgICAgIC8vIFByb3ZpZGUgZ2xvYmFsIHZhcmlhYmxlcyB0byB1c2UgaW4gdGhlIFVNRCBidWlsZFxuICAgICAgICAvLyBmb3IgZXh0ZXJuYWxpemVkIGRlcHNcbiAgICAgICAgZ2xvYmFsczoge1xuICAgICAgICAgIHZ1ZTogJ1Z1ZScsXG4gICAgICAgICAgcHJpbWV2dWU6ICdwcmltZXZ1ZScsXG4gICAgICAgIH0sXG4gICAgICAgIGV4cG9ydHM6IFwibmFtZWRcIlxuICAgICAgfSxcbiAgICB9LFxuICB9LFxufSlcbiJdLAogICJtYXBwaW5ncyI6ICI7QUFBeVMsU0FBUyxlQUFlO0FBQ2pVLFNBQVMsb0JBQW9CO0FBQzdCLE9BQU8sU0FBUztBQUNoQixPQUFPLFNBQVM7QUFDaEIsT0FBTyxjQUFjO0FBSnJCLElBQU0sbUNBQW1DO0FBT3pDLElBQU8sc0JBQVEsYUFBYTtBQUFBLEVBQzFCLFNBQVMsQ0FBQyxJQUFJLEdBQUcsSUFBSTtBQUFBLElBQ25CLGNBQWM7QUFBQSxFQUNoQixDQUFDLEdBQUcsU0FBUyxDQUFDO0FBQUEsRUFDZCxPQUFPO0FBQUEsSUFDTCxLQUFLO0FBQUE7QUFBQSxNQUVILE9BQU8sUUFBUSxrQ0FBVyxjQUFjO0FBQUEsTUFDeEMsTUFBTTtBQUFBO0FBQUEsTUFFTixVQUFVO0FBQUEsTUFDVixTQUFTLENBQUMsTUFBTSxPQUFPLEtBQUs7QUFBQSxJQUM5QjtBQUFBLElBQ0EsZUFBZTtBQUFBLE1BQ2IsT0FBTztBQUFBLFFBQ0wsTUFBTSxRQUFRLGtDQUFXLDhCQUE4QjtBQUFBLE1BQ3pEO0FBQUE7QUFBQTtBQUFBLE1BR0EsVUFBVSxDQUFDLE9BQU8sVUFBVTtBQUFBLE1BQzVCLFFBQVE7QUFBQTtBQUFBO0FBQUEsUUFHTixTQUFTO0FBQUEsVUFDUCxLQUFLO0FBQUEsVUFDTCxVQUFVO0FBQUEsUUFDWjtBQUFBLFFBQ0EsU0FBUztBQUFBLE1BQ1g7QUFBQSxJQUNGO0FBQUEsRUFDRjtBQUNGLENBQUM7IiwKICAibmFtZXMiOiBbXQp9Cg==
