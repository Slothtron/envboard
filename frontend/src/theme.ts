/**
 * 主题应用（设计规范 D-1）：暗色只靠 `<html class="dark" data-theme="dark">` 切换，
 * 组件层不写任何 `dark:` 覆盖；跟随系统用 prefers-color-scheme 监听。
 *
 * 持久化（ADR-012）：**权威在 `<state_dir>/ui.json`**（壳侧 `ui_prefs_*` 命令）。
 * 关窗即销毁 WebView 时，WebView2 不保证把最近的 localStorage 写入刷盘（实测
 * 写入 3s 后关窗、重开读回 null），所以 localStorage 只当首帧快速路径，
 * 每次改主题都写穿到 ui.json；启动后再与 profile 对账一次。
 */

import { api } from "./api/client";

export type ThemeChoice = "light" | "dark" | "system";

const STORAGE_KEY = "envbo…heme";

const systemDark = () =>
  window.matchMedia("(prefers-color-scheme: dark)").matches;

export function storedChoice(): ThemeChoice {
  const raw = localStorage.getItem(STORAGE_KEY);
  // 方向C 默认磷光深色；未存选择时不再跟随系统浅色。
  return raw === "light" || raw === "dark" || raw === "system" ? raw : "dark";
}

/** 把选择写进 DOM（class 与 data-theme 同步，HeroUI 与自定义 CSS 各认其一）。 */
export function applyTheme(choice: ThemeChoice): void {
  const dark = choice === "dark" || (choice === "system" && systemDark());
  const root = document.documentElement;
  root.classList.toggle("dark", dark);
  root.dataset.theme = dark ? "dark" : "light";
}

/** 启动时接线一次：应用已存选择，并在「跟随系统」时响应系统切换。 */
export function initTheme(): void {
  applyTheme(storedChoice());
  window
    .matchMedia("(prefers-color-scheme: dark)")
    .addEventListener("change", () => {
      if (storedChoice() === "system") applyTheme("system");
    });
}

export function rememberTheme(choice: ThemeChoice): void {
  localStorage.setItem(STORAGE_KEY, choice);
  applyTheme(choice);
  // 写穿到 ui.json：WebView 销毁不保证 localStorage 刷盘，profile 才是权威。
  void api.uiPrefsSet(choice).catch(() => undefined);
}

/** 与 ui.json 对账：profile 有值就以它为准；没有则把当前选择写进去。 */
export async function reconcileThemeFromProfile(): Promise<void> {
  try {
    const stored = await api.uiPrefsGet();
    if (stored === "light" || stored === "dark" || stored === "system") {
      localStorage.setItem(STORAGE_KEY, stored);
      applyTheme(stored);
    } else {
      void api.uiPrefsSet(storedChoice()).catch(() => undefined);
    }
  } catch {
    // 偏好读失败不挡界面（IPC 未就绪等）。
  }
}
