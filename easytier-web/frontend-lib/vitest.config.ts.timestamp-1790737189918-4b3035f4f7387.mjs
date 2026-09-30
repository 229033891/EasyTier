// vitest.config.ts
import { defineConfig } from "file:///D:/EasyTier/node_modules/.pnpm/vitest@2.1.9_@types+node@22.18.1_happy-dom@16.8.1/node_modules/vitest/dist/config.js";
import vue from "file:///D:/EasyTier/node_modules/.pnpm/@vitejs+plugin-vue@5.2.4_vite@5.4.21_@types+node@22.18.1__vue@3.5.21_typescript@5.6.3_/node_modules/@vitejs/plugin-vue/dist/index.mjs";
import ViteYaml from "file:///D:/EasyTier/node_modules/.pnpm/@modyfi+vite-plugin-yaml@1.1.1_rollup@4.50.1_vite@5.4.21_@types+node@22.18.1_/node_modules/@modyfi/vite-plugin-yaml/dist/index.js";
var vitest_config_default = defineConfig({
  plugins: [vue(), ViteYaml()],
  test: {
    environment: "happy-dom",
    include: ["tests/**/*.spec.ts"],
    setupFiles: ["./tests/setup.ts"]
  }
});
export {
  vitest_config_default as default
};
//# sourceMappingURL=data:application/json;base64,ewogICJ2ZXJzaW9uIjogMywKICAic291cmNlcyI6IFsidml0ZXN0LmNvbmZpZy50cyJdLAogICJzb3VyY2VzQ29udGVudCI6IFsiY29uc3QgX192aXRlX2luamVjdGVkX29yaWdpbmFsX2Rpcm5hbWUgPSBcIkQ6XFxcXEVhc3lUaWVyXFxcXGVhc3l0aWVyLXdlYlxcXFxmcm9udGVuZC1saWJcIjtjb25zdCBfX3ZpdGVfaW5qZWN0ZWRfb3JpZ2luYWxfZmlsZW5hbWUgPSBcIkQ6XFxcXEVhc3lUaWVyXFxcXGVhc3l0aWVyLXdlYlxcXFxmcm9udGVuZC1saWJcXFxcdml0ZXN0LmNvbmZpZy50c1wiO2NvbnN0IF9fdml0ZV9pbmplY3RlZF9vcmlnaW5hbF9pbXBvcnRfbWV0YV91cmwgPSBcImZpbGU6Ly8vRDovRWFzeVRpZXIvZWFzeXRpZXItd2ViL2Zyb250ZW5kLWxpYi92aXRlc3QuY29uZmlnLnRzXCI7aW1wb3J0IHsgZGVmaW5lQ29uZmlnIH0gZnJvbSAndml0ZXN0L2NvbmZpZydcbmltcG9ydCB2dWUgZnJvbSAnQHZpdGVqcy9wbHVnaW4tdnVlJ1xuaW1wb3J0IFZpdGVZYW1sIGZyb20gJ0Btb2R5Zmkvdml0ZS1wbHVnaW4teWFtbCdcblxuZXhwb3J0IGRlZmF1bHQgZGVmaW5lQ29uZmlnKHtcbiAgcGx1Z2luczogW3Z1ZSgpLCBWaXRlWWFtbCgpXSxcbiAgdGVzdDoge1xuICAgIGVudmlyb25tZW50OiAnaGFwcHktZG9tJyxcbiAgICBpbmNsdWRlOiBbJ3Rlc3RzLyoqLyouc3BlYy50cyddLFxuICAgIHNldHVwRmlsZXM6IFsnLi90ZXN0cy9zZXR1cC50cyddLFxuICB9LFxufSlcbiJdLAogICJtYXBwaW5ncyI6ICI7QUFBNlMsU0FBUyxvQkFBb0I7QUFDMVUsT0FBTyxTQUFTO0FBQ2hCLE9BQU8sY0FBYztBQUVyQixJQUFPLHdCQUFRLGFBQWE7QUFBQSxFQUMxQixTQUFTLENBQUMsSUFBSSxHQUFHLFNBQVMsQ0FBQztBQUFBLEVBQzNCLE1BQU07QUFBQSxJQUNKLGFBQWE7QUFBQSxJQUNiLFNBQVMsQ0FBQyxvQkFBb0I7QUFBQSxJQUM5QixZQUFZLENBQUMsa0JBQWtCO0FBQUEsRUFDakM7QUFDRixDQUFDOyIsCiAgIm5hbWVzIjogW10KfQo=
