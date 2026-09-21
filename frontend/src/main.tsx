import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { Workbench } from "./Workbench";
import { initTheme } from "./theme";
import "./index.css";

// 先应用主题再渲染，避免暗色用户在首帧看到亮色闪烁（设计规范 D-1）。
initTheme();

const container = document.getElementById("root");

if (!container) {
  throw new Error("#root not found");
}

createRoot(container).render(
  <StrictMode>
    <Workbench />
  </StrictMode>,
);
