import type { ReactNode } from "react";
import {
  AlertDialog,
  Button,
  Chip,
  Modal,
  ScrollShadow,
  Separator,
} from "@heroui/react";

import { IconAlert, IconInfo } from "./icons";
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
  onConfirm,
}: {
  isOpen: boolean;
  onOpenChange: (isOpen: boolean) => void;
  title: string;
  description: string;
  confirmLabel?: string;
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
                    <p className="text-base font-semibold text-danger">此操作不可撤销。</p>
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
  onConfirm,
}: {
  isOpen: boolean;
  onOpenChange: (isOpen: boolean) => void;
  title: string;
  note?: string;
  children?: ReactNode;
  confirmLabel?: string;
  onConfirm: () => void;
}) {
  return (
    <Modal>
      <Modal.Backdrop isOpen={isOpen} onOpenChange={onOpenChange}>
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
                  <ScrollShadow className="max-h-96">
                    <div className="flex flex-col gap-4">
                      {children}
                      {note ? (
                        <>
                          <Separator />
                          <Hint>{note}</Hint>
                        </>
                      ) : null}
                    </div>
                  </ScrollShadow>
                </Modal.Body>

                <Modal.Footer>
                  <span className="mr-auto" />
                  <Button slot="close" variant="ghost">
                    取消
                  </Button>
                  <Button
                    variant="primary"
                    onPress={() => {
                      onConfirm();
                      close();
                    }}
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

/** 弹层页脚徽标用：说明当前动作的性质 */
export function ActionBadge({ children }: { children: ReactNode }) {
  return (
    <Chip size="sm" variant="soft">
      <Chip.Label>{children}</Chip.Label>
    </Chip>
  );
}
