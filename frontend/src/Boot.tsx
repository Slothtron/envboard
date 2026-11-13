import { useEffect, useState } from "react";

/**
 * 方向C 签名时刻：boot sequence（[ OK ] 行 staggered 揭示）。
 * 自动进入（~1s），任意键/点击跳过；reduced-motion 立即进入。
 * 完成后从 DOM 卸载，不残留遮罩挡点击。
 *
 * 只在**进程的首次窗口**挂载：托盘重建的窗口带壳注入的
 * `window.__envboardRecreated`，`Workbench` 据此从第一帧起就不渲染它（ADR-012）。
 */
export function Boot({ onDone }: { onDone: () => void }) {
  const [lines] = useState(() => [
    { tone: "ph-bootcyan", text: "envboard-engine · 本地环境代理工作台" },
    { tone: "ph-bootok", text: "[ OK ] 状态文件载入 %LOCALAPPDATA%\\envboard\\state" },
    { tone: "ph-bootok", text: "[ OK ] 装配环境账本 · 规则集 · 上游代理" },
    { tone: "ph-bootok", text: "[ OK ] 根 CA 就绪 · ECDSA P-256" },
    { tone: "ph-bootok", text: "[ OK ] 事件流 /api/events 接线" },
    { tone: "ph-bootwarn", text: "[WARN] 需处理环境见状态栏装配警告" },
    { tone: "ph-bootok", text: "[ OK ] 端口区间 16000-16999" },
    { tone: "ph-bootcyan", text: "> 进入工作台" },
  ]);

  useEffect(() => {
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    const timer = window.setTimeout(onDone, reduce ? 0 : 1000);
    const skip = () => onDone();
    window.addEventListener("keydown", skip);
    window.addEventListener("pointerdown", skip);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener("keydown", skip);
      window.removeEventListener("pointerdown", skip);
    };
  }, [onDone]);

  return (
    <div className="ph-boot" role="status" aria-label="启动自检">
      <pre>
        {lines.map((l, i) => (
          <div key={i} className={`ph-bootline ${l.tone}`}>
            {l.text}
          </div>
        ))}
      </pre>
    </div>
  );
}
