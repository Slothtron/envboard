import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

declare const process: { env: Record<string, string | undefined> };

/* dev 代理目标：默认本机实跑的管理器（回环免 token 档）；
   ENVBOARD_PROXY 可覆盖（如联调临时实例 http://127.0.0.1:8901）。 */
const proxyTarget = process.env["ENVBOARD_PROXY"] ?? "http://127.0.0.1:8900";

export default defineConfig({
  // 整页由 envboard web 形态在 `/` 托管，资源用绝对路径；
  // 产物文件名带 hash，web 层 serve 时回 immutable 缓存头。
  base: "/",
  plugins: [react(), tailwindcss()],
  build: {
    // 严格 CSP（无 unsafe-inline）：禁止 Vite 注入任何内联 preload polyfill。
    modulePreload: false,
  },
  server: {
    host: "127.0.0.1",
    port: 5199,
    // dev 直连本机实跑的管理器（envboard --listen 127.0.0.1:8900，回环免 token 档）。
    proxy: {
      "/api": {
        target: proxyTarget,
        changeOrigin: true,
      },
    },
  },
  preview: {
    host: "127.0.0.1",
    port: 5199,
  },
});
