import { useEffect, useState, type ReactNode } from "react";
import {
  AlertDialog,
  Button,
  Label,
  Modal,
  ScrollShadow,
  Separator,
  TextArea,
  TextField,
} from "@heroui/react";

import { IconAlert, IconCopy, IconInfo } from "./icons";
import { errorTitle, formatErrorNotice } from "./errors";
import { api } from "./api/client";
import { Hint } from "./shared";

/* ------------------------------------------------------------------ */
/* 破坏性操作二次确认（不可逆 → AlertDialog）                           */
/* ------------------------------------------------------------------ */

export function ConfirmDialog({
  isOpen,
  onOpenChange,
  title,
  description,
  confirmLabel = "确认",
  undoable = false,
  onConfirm,
}: {
  isOpen: boolean;
  onOpenChange: (isOpen: boolean) => void;
  title: string;
  description: string;
  confirmLabel?: string;
  /** 可逆操作（如信任库安装/卸载）：不印「不可撤销」，改可逆说明。 */
  undoable?: boolean;
  onConfirm: () => void;
}) {
  return (
    <AlertDialog>
      <AlertDialog.Backdrop isOpen={isOpen} onOpenChange={onOpenChange}>
        <AlertDialog.Container placement="center" size="sm">
          <AlertDialog.Dialog>
            {({ close }) => (
              <>
                <AlertDialog.Header>
                  <AlertDialog.Icon>
                    <IconAlert className="size-5" />
                  </AlertDialog.Icon>
                  <AlertDialog.Heading>{title}</AlertDialog.Heading>
                </AlertDialog.Header>

                <AlertDialog.Body>
                  <div className="flex flex-col gap-3">
                    <p className="text-base leading-relaxed text-foreground">{description}</p>
                    {undoable ? (
                      <p className="text-base text-muted">此操作可逆：之后可以再改回来。</p>
                    ) : (
                      <p className="text-base font-semibold text-danger">此操作不可撤销。</p>
                    )}
                  </div>
                </AlertDialog.Body>

                <AlertDialog.Footer>
                  <Button slot="close" variant="ghost">
                    取消
                  </Button>
                  {/* 弹层确认键用实心红（危险档） */}
                  <Button
                    variant="danger"
                    onPress={() => {
                      onConfirm();
                      close();
                    }}
                  >
                    {confirmLabel}
                  </Button>
                </AlertDialog.Footer>
              </>
            )}
          </AlertDialog.Dialog>
        </AlertDialog.Container>
      </AlertDialog.Backdrop>
    </AlertDialog>
  );
}

/* ------------------------------------------------------------------ */
/* 通用模态窗（需要输入或确认的动作）                                    */
/* ------------------------------------------------------------------ */

export function FormDialog({
  isOpen,
  onOpenChange,
  title,
  note,
  children,
  confirmLabel = "确定",
  confirmDisabled = false,
  onConfirm,
}: {
  isOpen: boolean;
  onOpenChange: (isOpen: boolean) => void;
  title: string;
  note?: string;
  children?: ReactNode;
  confirmLabel?: string;
  /** 校验未通过时禁用确认键：非法输入不能静默关窗（调用方接各自的 invalid）。 */
  confirmDisabled?: boolean;
  /** 返回非空字符串 = 展示在对话框内的错误文案（原样）；null/undefined/void = 成功并关窗。 */
  onConfirm: () => void | string | null | Promise<void | string | null>;
}) {
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  // 重开时清空：上一次的失败原因不得串到下一次输入。
  useEffect(() => {
    if (isOpen) {
      setError(null);
      setSubmitting(false);
    }
  }, [isOpen]);

  async function handleConfirm(close: () => void) {
    if (submitting) return;
    setError(null);
    setSubmitting(true);
    try {
      const result = await onConfirm();
      if (typeof result === "string" && result.length > 0) {
        setError(result);
        return;
      }
      close();
    } catch (err: unknown) {
      // 只有抛错才走统一错误呈现；返回字符串由调用方决定文案。
      setError(errorTitle(err, title));
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Modal>
      <Modal.Backdrop
        isOpen={isOpen}
        onOpenChange={(open) => {
          if (!open) setError(null);
          onOpenChange(open);
        }}
      >
        <Modal.Container placement="center" size="md">
          <Modal.Dialog>
            {({ close }) => (
              <>
                <Modal.CloseTrigger />
                <Modal.Header>
                  <Modal.Icon>
                    <IconInfo className="size-5" />
                  </Modal.Icon>
                  <Modal.Heading>{title}</Modal.Heading>
                </Modal.Header>

                <Modal.Body>
                  <ScrollShadow className="max-h-[60vh]">
                    {/* form：输入框内 Enter 直接提交（G-12） */}
                    <form
                      className="@container flex flex-col gap-4"
                      onSubmit={(e) => {
                        e.preventDefault();
                        void handleConfirm(close);
                      }}
                    >
                      {children}
                      {error ? (
                        <p
                          className="text-sm leading-relaxed text-danger"
                          data-slot="form-error"
                          role="alert"
                        >
                          {error}
                        </p>
                      ) : null}
                      {note ? (
                        <>
                          <Separator />
                          <Hint>{note}</Hint>
                        </>
                      ) : null}
                    </form>
                  </ScrollShadow>
                </Modal.Body>

                <Modal.Footer>
                  <span className="mr-auto" />
                  <Button slot="close" variant="ghost">
                    取消
                  </Button>
                  <Button
                    isDisabled={confirmDisabled || submitting}
                    variant="primary"
                    onPress={() => void handleConfirm(close)}
                  >
                    {confirmLabel}
                  </Button>
                </Modal.Footer>
              </>
            )}
          </Modal.Dialog>
        </Modal.Container>
      </Modal.Backdrop>
    </Modal>
  );
}

/** 端口校验：1–65535 的十进制整数（界面唯一口径，D4）。空串不合法。 */
export function portError(value: string): string | null {
  if (!/^\d{1,5}$/.test(value)) return "端口必须是 1–65535 的整数。";
  const port = Number(value);
  if (port < 1 || port > 65535) return "端口必须是 1–65535 的整数。";
  return null;
}

/* ------------------------------------------------------------------ */
/* 规则正文：查看 / 复制 / 就地编辑                                      */
/* ------------------------------------------------------------------ */

/**
 * 桌面形态没有浏览器新标签页，规则正文进模态；模态内是**可编辑 textarea**：
 * - 可选中、Ctrl+A/Ctrl+C 复制（右键菜单被壳禁用 ADR-009，故另给「复制」按钮）；
 * - 「保存」走 `rules_import` 同名覆盖（后端语义：同名即替换 + 对绑定环境热应用），
 *   保存后回读账本里的规范化正文，用户能看到引擎重排后的真实内容；
 * - 有未保存修改时关闭需二次确认，避免手滑丢编辑。
 */
export function RuleEditorDialog({
  isOpen,
  onOpenChange,
  name,
  onSaved,
}: {
  isOpen: boolean;
  onOpenChange: (isOpen: boolean) => void;
  name: string;
  /** 保存成功后回调（宿主的只读预览/计数需要重读）。 */
  onSaved?: () => void;
}) {
  /** 账本里的权威正文；null = 还没读到（或读失败）。 */
  const [saved, setSaved] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [copied, setCopied] = useState<"idle" | "done" | "failed">("idle");
  const [error, setError] = useState<string | null>(null);
  const [confirmDiscard, setConfirmDiscard] = useState(false);

  // 每次打开重读账本：同名的正文可能已被别处导入覆盖。
  useEffect(() => {
    if (!isOpen) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    setCopied("idle");
    setConfirmDiscard(false);
    api
      .rulesGet(name)
      .then((result) => {
        if (cancelled) return;
        setSaved(result.text);
        setDraft(result.text);
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        const f = formatErrorNotice(err, "读取规则正文");
        setSaved(null);
        setDraft("");
        setError(f.detail ? `${f.title}（${f.detail}）` : f.title);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [isOpen, name]);

  const dirty = saved !== null && draft !== saved;
  const entryCount = draft.split("\n").filter((line) => {
    const t = line.trim();
    return t.length > 0 && !t.startsWith("#");
  }).length;

  function requestClose() {
    if (dirty) {
      setConfirmDiscard(true);
      return;
    }
    onOpenChange(false);
  }

  async function handleSave() {
    if (saving || !dirty) return;
    setSaving(true);
    setError(null);
    try {
      await api.rulesImport({ name, text: draft });
      const reread = await api.rulesGet(name);
      // 账本里的 rendered 就是权威：导入会重排/规范化，回读后草稿与基线重新对齐。
      setSaved(reread.text);
      setDraft(reread.text);
      onSaved?.();
    } catch (err: unknown) {
      const f = formatErrorNotice(err, "保存规则正文");
      setError(f.detail ? `${f.title}（${f.detail}）` : f.title);
    } finally {
      setSaving(false);
    }
  }

  async function handleCopy() {
    try {
      await navigator.clipboard.writeText(draft);
      setCopied("done");
    } catch {
      setCopied("failed");
    }
  }

  return (
    <>
      <Modal>
        <Modal.Backdrop
          isOpen={isOpen}
          onOpenChange={(open) => {
            if (open) {
              onOpenChange(true);
            } else {
              requestClose();
            }
          }}
        >
          <Modal.Container placement="center" size="lg">
            <Modal.Dialog>
              <Modal.CloseTrigger />
              <Modal.Header>
                <Modal.Heading>规则集原文 · {name}</Modal.Heading>
              </Modal.Header>
              <Modal.Body>
                <TextField
                  className="w-full"
                  value={draft}
                  onChange={(value) => {
                    setDraft(value);
                    // 编辑后回到「条数 / 有未保存修改」提示：复制反馈不常驻。
                    if (copied !== "idle") setCopied("idle");
                  }}
                >
                  <Label className="sr-only">规则正文</Label>
                  <TextArea
                    className="max-h-[52vh] font-mono text-sm leading-relaxed"
                    placeholder={loading ? "加载中…" : ""}
                    rows={12}
                    spellCheck={false}
                  />
                </TextField>
                {error ? (
                  <p className="text-sm leading-relaxed text-danger" role="alert">
                    {error}
                  </p>
                ) : null}
                <Hint>
                  hosts 风格：IP + 完整域名，每行一条；空行与 # 注释不计入条数。
                </Hint>
                <Hint>
                  编辑后点「保存」即同名覆盖写入，并对绑定该规则集的环境热应用。
                </Hint>
              </Modal.Body>
              <Modal.Footer>
                <span className="mr-auto flex min-w-0 items-center gap-2">
                  <Button
                    isDisabled={saved === null || loading}
                    variant="secondary"
                    onPress={() => void handleCopy()}
                  >
                    <IconCopy className="size-4" />
                    复制
                  </Button>
                  <span className="truncate text-xs text-muted">
                    {copied === "done"
                      ? "已复制到剪贴板"
                      : copied === "failed"
                        ? "复制失败：剪贴板不可用"
                        : `${entryCount} 条规则${dirty ? " · 有未保存修改" : ""}`}
                  </span>
                </span>
                <Button slot="close" variant="ghost">
                  关闭
                </Button>
                <Button
                  isDisabled={!dirty || saving}
                  isPending={saving}
                  variant="primary"
                  onPress={() => void handleSave()}
                >
                  保存
                </Button>
              </Modal.Footer>
            </Modal.Dialog>
          </Modal.Container>
        </Modal.Backdrop>
      </Modal>

      <ConfirmDialog
        confirmLabel="放弃修改"
        description="规则正文有未保存的修改，关闭后这些修改会丢失。"
        isOpen={confirmDiscard}
        title="放弃未保存的修改？"
        onConfirm={() => {
          setConfirmDiscard(false);
          onOpenChange(false);
        }}
        onOpenChange={(open) => {
          if (!open) setConfirmDiscard(false);
        }}
      />
    </>
  );
}
