import { useMemo, useState, type ReactNode } from "react";

import { formatBytes } from "./data";
import { FilterButton, Hint } from "./shared";
import type { BodyEncoding } from "./api/types";
import { ScrollShadow } from "@heroui/react";

/**
 * 抓包正文格式化 + 语法高亮（零依赖手写，遵循仓库「依赖克制」纪律）。
 * 按 content-type 选语言：JSON 美化（JSON.parse→stringify 2 空格）+ 精确高亮；
 * HTML/XML 缩进美化 + 标签/属性高亮；CSS/JS 高亮（不重排）；其余纯文本。
 * 所有视图支持 pretty/raw 切换；raw 永远展示原始字节文本。
 */

export type BodyLanguage = "json" | "html" | "xml" | "css" | "js" | "text";

const HIGHLIGHT_LIMIT = 200_000; // 超大正文跳过高亮，避免逐字符扫描卡顿

export function contentTypeOf(headers: Array<[string, string]>): string | undefined {
  return headers.find(([k]) => k.toLowerCase() === "content-type")?.[1];
}

/** 选语言 + 生成美化文本。formattable=false 时 pretty 与 raw 相同。 */
export function prepareBody(
  content: string,
  contentType?: string,
): { language: BodyLanguage; pretty: string; formattable: boolean } {
  const ct = (contentType ?? "").toLowerCase();
  const trimmed = content.trimStart();
  if (ct.includes("json") || (!ct && (trimmed.startsWith("{") || trimmed.startsWith("[")))) {
    try {
      return { language: "json", pretty: JSON.stringify(JSON.parse(content), null, 2), formattable: true };
    } catch {
      /* 不是合法 JSON：落到后续嗅探 */
    }
  }
  if (ct.includes("html") || /^<!doctype html|^<html/i.test(trimmed)) {
    return { language: "html", pretty: formatMarkup(content), formattable: true };
  }
  if (ct.includes("xml") || trimmed.startsWith("<?xml")) {
    return { language: "xml", pretty: formatMarkup(content), formattable: true };
  }
  if (ct.includes("css")) return { language: "css", pretty: content, formattable: false };
  if (ct.includes("javascript") || ct.includes("ecmascript")) {
    return { language: "js", pretty: content, formattable: false };
  }
  if (trimmed.startsWith("<")) return { language: "html", pretty: formatMarkup(content), formattable: true };
  return { language: "text", pretty: content, formattable: false };
}

/* ------------------------- 美化 ------------------------- */

const VOID_TAGS = new Set([
  "area", "base", "br", "col", "embed", "hr", "img", "input",
  "link", "meta", "param", "source", "track", "wbr",
]);

/** HTML/XML 缩进美化：每个标签独占一行，文本节点缩进；自闭合/void 不增深度。 */
function formatMarkup(src: string): string {
  const tokens = src.match(/<!--[\s\S]*?-->|<!\[CDATA\[[\s\S]*?\]\]>|<[^>]+>|[^<]+/g);
  if (!tokens) return src;
  const pad = (n: number) => "  ".repeat(n);
  const out: string[] = [];
  let depth = 0;
  for (const raw of tokens) {
    const t = raw.trim();
    if (!t) continue;
    if (t.startsWith("</")) {
      depth = Math.max(0, depth - 1);
      out.push(pad(depth) + t);
    } else if (t.startsWith("<")) {
      out.push(pad(depth) + t);
      const name = /^<\/?\s*([a-zA-Z][\w:-]*)/.exec(t)?.[1]?.toLowerCase() ?? "";
      const selfClosing = /\/>$/.test(t) || t.startsWith("<?") || t.startsWith("<!") || VOID_TAGS.has(name);
      if (!selfClosing) depth += 1;
    } else {
      out.push(pad(depth) + t);
    }
  }
  return out.join("\n");
}

/* ------------------------- 高亮 ------------------------- */

let keySeq = 0;
const span = (text: string, cls: string): ReactNode => (
  <span key={keySeq++} className={`tok-${cls}`}>{text}</span>
);
const plain = (text: string): ReactNode => <span key={keySeq++}>{text}</span>;

/** 通用扫描：re 全局正则，classify 返回令牌类名（null=纯文本）。 */
function scan(code: string, re: RegExp, classify: (m: RegExpExecArray) => string | null): ReactNode[] {
  const nodes: ReactNode[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  re.lastIndex = 0;
  while ((m = re.exec(code))) {
    if (m.index > last) nodes.push(plain(code.slice(last, m.index)));
    const cls = classify(m);
    nodes.push(cls ? span(m[0], cls) : plain(m[0]));
    last = re.lastIndex;
    if (m[0].length === 0) re.lastIndex += 1; // 防零宽死循环
  }
  if (last < code.length) nodes.push(plain(code.slice(last)));
  return nodes;
}

const JSON_RE = /("(?:\\.|[^"\\])*")(\s*:)?|\b(?:true|false|null)\b|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|[{}[\],:]/g;
function highlightJson(code: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  JSON_RE.lastIndex = 0;
  while ((m = JSON_RE.exec(code))) {
    if (m.index > last) nodes.push(plain(code.slice(last, m.index)));
    if (m[1] !== undefined) {
      nodes.push(span(m[1], m[2] ? "key" : "string"));
      if (m[2]) nodes.push(span(m[2], "punct"));
    } else if (/^(true|false|null)$/.test(m[0])) {
      nodes.push(span(m[0], "literal"));
    } else if (/^-?\d/.test(m[0])) {
      nodes.push(span(m[0], "number"));
    } else {
      nodes.push(span(m[0], "punct"));
    }
    last = JSON_RE.lastIndex;
  }
  if (last < code.length) nodes.push(plain(code.slice(last)));
  return nodes;
}

const TAG_INNER = /("[^"]*"|'[^']*')|([a-zA-Z][\w:-]*)|([<>/=])/g;
function highlightTag(tag: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  let last = 0;
  let firstWord = true;
  let m: RegExpExecArray | null;
  TAG_INNER.lastIndex = 0;
  while ((m = TAG_INNER.exec(tag))) {
    if (m.index > last) nodes.push(plain(tag.slice(last, m.index)));
    if (m[1]) nodes.push(span(m[1], "string"));
    else if (m[2]) {
      nodes.push(span(m[2], firstWord ? "tag" : "attr"));
      firstWord = false;
    } else nodes.push(span(m[3], "punct"));
    last = TAG_INNER.lastIndex;
  }
  if (last < tag.length) nodes.push(plain(tag.slice(last)));
  return nodes;
}

const HTML_RE = /<!--[\s\S]*?-->|<!\[CDATA\[[\s\S]*?\]\]>|<\/?[a-zA-Z][^>]*>|[^<]+|</g;
function highlightMarkup(code: string): ReactNode[] {
  const nodes: ReactNode[] = [];
  let last = 0;
  let m: RegExpExecArray | null;
  HTML_RE.lastIndex = 0;
  while ((m = HTML_RE.exec(code))) {
    if (m.index > last) nodes.push(plain(code.slice(last, m.index)));
    const t = m[0];
    if (t.startsWith("<!--") || t.startsWith("<![CDATA[")) nodes.push(span(t, "comment"));
    else if (t.startsWith("<") && t.length > 1) nodes.push(...highlightTag(t));
    else nodes.push(plain(t));
    last = HTML_RE.lastIndex;
  }
  if (last < code.length) nodes.push(plain(code.slice(last)));
  return nodes;
}

const CSS_RE = /\/\*[\s\S]*?\*\/|("[^"]*"|'[^']*')|(@[\w-]+)|([\w-]+)(?=\s*:)|(-?\d+(?:\.\d+)?(?:px|em|rem|%|vh|vw|s|ms|deg|fr)?)|([{}:;,])/g;
const highlightCss = (code: string) =>
  scan(code, CSS_RE, (m) => {
    if (m[0].startsWith("/*")) return "comment";
    if (m[1]) return "string";
    if (m[2]) return "literal";
    if (m[3]) return "key";
    if (m[4]) return "number";
    return "punct";
  });

const JS_RE = /(\/\/[^\n]*|\/\*[\s\S]*?\*\/)|("(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`)|\b(const|let|var|function|return|if|else|for|while|new|class|import|export|from|default|async|await|try|catch|throw|typeof|instanceof|true|false|null|undefined|this)\b|(-?\d+(?:\.\d+)?)/g;
const highlightJs = (code: string) =>
  scan(code, JS_RE, (m) => {
    if (m[1]) return "comment";
    if (m[2]) return "string";
    if (m[3]) return "literal";
    return "number";
  });

export function highlight(code: string, language: BodyLanguage): ReactNode[] {
  keySeq = 0;
  switch (language) {
    case "json": return highlightJson(code);
    case "html":
    case "xml": return highlightMarkup(code);
    case "css": return highlightCss(code);
    case "js": return highlightJs(code);
    default: return [plain(code)];
  }
}

/* ------------------------- 视图组件 ------------------------- */

/** utf8 正文：pretty/raw 切换 + 按语言高亮。hooks 只在本组件内（父级早返回不涉 hooks）。 */
function Utf8Body({ content, size, contentType }: { content: string; size: number; contentType?: string }) {
  const [mode, setMode] = useState<"pretty" | "raw">("pretty");
  const prepared = useMemo(() => prepareBody(content, contentType), [content, contentType]);
  const code = mode === "pretty" ? prepared.pretty : content;
  // raw = 原始字节文本，不高亮；pretty 才格式化+高亮（超大正文仅格式化不高亮）。
  const canHighlight = mode === "pretty" && prepared.language !== "text" && code.length <= HIGHLIGHT_LIMIT;
  return (
    <div className="flex h-full min-h-0 flex-col gap-2">
      <div className="flex flex-none flex-wrap items-center gap-2">
        <div aria-label="正文视图" className="flex items-center gap-1" role="group">
          <FilterButton isActive={mode === "pretty"} onPress={() => setMode("pretty")}>Pretty</FilterButton>
          <FilterButton isActive={mode === "raw"} onPress={() => setMode("raw")}>Raw</FilterButton>
        </div>
        <span className="text-xs text-muted">
          {prepared.language} · {formatBytes(size)}
          {prepared.language !== "text" && code.length > HIGHLIGHT_LIMIT ? " · 超大正文，仅格式化不高亮" : ""}
        </span>
      </div>
      <ScrollShadow className="wb-body-block min-h-0 flex-1">
        <pre className="whitespace-pre-wrap break-all font-mono text-sm leading-relaxed text-foreground">
          {canHighlight ? highlight(code, prepared.language) : code}
        </pre>
      </ScrollShadow>
    </div>
  );
}

/** 请求 / 响应体入口：null/omitted/base64 给提示，utf8 进 Utf8Body。 */
export function BodyBlock({ body, contentType }: { body: BodyEncoding | null; contentType?: string }) {
  if (body === null || body === undefined) return <Hint>该消息没有正文。</Hint>;
  if ("omitted" in body) return <Hint>正文过大（{formatBytes(body.size)}），已省略存储。</Hint>;
  if (body.encoding === "base64") {
    return (
      <Hint>
        二进制正文（{formatBytes(body.size)}），base64 预览：
        <span className="mt-1 block truncate font-mono">{body.content.slice(0, 120)}…</span>
      </Hint>
    );
  }
  return <Utf8Body content={body.content} size={body.size} contentType={contentType} />;
}
