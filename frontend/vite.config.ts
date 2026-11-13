import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig, loadEnv } from "vite";

// mock 传输（dev-tools 方案，.agents/docs/dev-tools-mock-transport.md）：
// VITE_TRANSPORT=mock 时把 @tauri-apps/api/core 换成浏览器侧假引擎，不起 Tauri 壳
// 单独验收前端。发布构建不设该变量，shim 物理不可达（artifact 测试另钉了断言）。
// 用 loadEnv 读变量、alias 走根相对路径——不引 node:url/process，免加 @types/node。
export default defineConfig(({ mode }) => {
  const mockTransport = loadEnv(mode, "", "").VITE_TRANSPORT === "mock";
  return {
    // 整页由 Tauri 壳在 tauri://localhost 托管（发布）或 vite dev server（开发），
    // 资源用绝对路径；产物文件名带 hash。数据面走 IPC，dev 不再需要 /api 代理。
    base: "/",
    plugins: [react(), tailwindcss()],
    resolve: mockTransport
      ? { alias: { "@tauri-apps/api/core": "/dev-tools/tauri-shim.ts" } }
      : undefined,
    build: {
      // 严格 CSP（无 unsafe-inline）：禁止 Vite 注入任何内联 preload polyfill。
      modulePreload: false,
    },
    server: {
      host: "127.0.0.1",
      port: 5199,
      // Tauri dev 的 devUrl 指向这里；IPC 由壳注入，与 HTTP 无关。
      strictPort: true,
    },
    preview: {
      host: "127.0.0.1",
      port: 5199,
    },
  };
});
