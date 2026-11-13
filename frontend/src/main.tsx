import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { Workbench } from "./Workbench";
import { initTheme, reconcileThemeFromProfile } from "./theme";
import "./index.css";

// 先应用主题再渲染，避免暗色用户在首帧看到亮色闪烁（设计规范 D-1）。
initTheme();
// ADR-012：最近的 localStorage 写入可能没随 WebView 销毁刷盘，ui.json 才是权威。
void reconcileThemeFromProfile();

// 壳加固（ADR-009）：屏蔽默认右键菜单与浏览器刷新键（Ctrl+R / Ctrl+Shift+R / F5）。
// native 层（WebView2 settings）是权威；本层兜底首帧与非 Windows 平台，
// 并让走查断言可测。编辑键（Ctrl+C/V 等）不受影响。
window.addEventListener("contextmenu", (event) => event.preventDefault());
window.addEventListener(
  "keydown",
  (event) => {
    if ((event.ctrlKey || event.metaKey) && (event.key === "r" || event.key === "R")) {
      event.preventDefault();
    }
    if (event.key === "F5") {
      event.preventDefault();
    }
  },
  true,
);

const container = document.getElementById("root");

if (!container) {
  throw new Error("#root not found");
}

createRoot(container).render(
  <StrictMode>
    <Workbench />
  </StrictMode>,
);
