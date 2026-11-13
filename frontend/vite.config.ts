import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  // 整页由 Tauri 壳在 tauri://localhost 托管（发布）或 vite dev server（开发），
  // 资源用绝对路径；产物文件名带 hash。数据面走 IPC，dev 不再需要 /api 代理。
  base: "/",
  plugins: [react(), tailwindcss()],
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
});