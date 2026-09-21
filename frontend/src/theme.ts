/**
 * 主题应用（设计规范 D-1）：暗色只靠 `<html class="dark" data-theme="dark">` 切换，
 * 组件层不写任何 `dark:` 覆盖；跟随系统用 prefers-color-scheme 监听。
 */

export type ThemeChoice = "light" | "dark" | "system";

const STORAGE_KEY = "envbo…heme";

const systemDark = () =>
  window.matchMedia("(prefers-color-scheme: dark)").matches;

export function storedChoice(): ThemeChoice {
  const raw = localStorage.getItem(STORAGE_KEY);
  return raw === "light" || raw === "dark" || raw === "system" ? raw : "system";
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
}
