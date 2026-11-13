/**
 * 错误呈现的唯一定义处（envboard-engine::api::error 模块文档「界面呈现」）。
 *
 * 三层缺一不可：错误码（排障）+ 英文原文（唯一可行动细节）+ 中文结论（使用者
 * 第一句读到的）。界面只翻译不判定：能不能重试来自 code 的 retryability，
 * 本文件不做任何重试决策。
 *
 * 每个契约码都有中文结论，原文不翻译、不截断、不删除。
 */

import { ApiError } from "./api/types";

/** 错误码 → 中文结论（覆盖后端错误码契约表里的每个码）。 */
export const ERROR_HINTS: Record<string, string> = {
  invalid_config: "配置非法，请检查后重试",
  not_found: "目标不存在",
  conflict: "状态冲突",
  port_conflict: "端口已被其他程序占用",
  port_range_exhausted: "端口区间内没有可用端口",
  store_failure: "状态文件读写失败",
  internal_error: "服务内部错误",
};

export function errorHint(code: string): string {
  return ERROR_HINTS[code] ?? "操作失败";
}

/**
 * 三层错误呈现：中文主句 + `[code] 原文 · field` 细节。
 * `action` 点名"哪个动作 + 哪个对象"（`启动 dev`）；没有对象名的动作
 * （列表加载、导入）传自解释短语或不传。
 */
export function formatErrorNotice(
  error: unknown,
  action?: string,
): { title: string; detail?: string } {
  if (error instanceof ApiError) {
    const subject = action ? `${action}失败：` : "";
    return {
      title: `${subject}${errorHint(error.code)}`,
      detail: `[${error.code}] ${error.message}${error.field ? ` · field: ${error.field}` : ""}`,
    };
  }
  // 不是服务端回执（本地表单校验、网络断开）：只给中文一句，不编造码与原文。
  return { title: action ? `${action}失败：请求失败，请重试` : "请求失败，请重试" };
}

/** 单行版（反馈位只能传一句时）：主句 + 原文拼接，三层仍全在。 */
export function errorTitle(error: unknown, action?: string): string {
  const { title, detail } = formatErrorNotice(error, action);
  return detail ? `${title}（${detail}）` : title;
}