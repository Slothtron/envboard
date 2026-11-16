import { Button, Modal, Separator } from "@heroui/react";

import type { StatusInfo } from "./api/types";
import { FilterButton, SectionTitle } from "./shared";
import type { ThemeChoice } from "./theme";

export interface SettingsDialogProps {
  isOpen: boolean;
  onOpenChange: (isOpen: boolean) => void;
  status: StatusInfo | null;
  coreVersion: string;
  /** 主题单一来源由 Workbench 持有（V-1）。 */
  theme: ThemeChoice;
  onThemeChange: (choice: ThemeChoice) => void;
}

/**
 * 全局设置模态（ADR-011：原设置页删除，仅保留主题与「关于」）。
 * 证书独立成页（views/CertView.tsx），这里不再出现任何证书入口。
 * 只放用户真会改的东西：状态目录/事件丢弃属诊断信息，不上界面。
 */
export function SettingsDialog({
  isOpen,
  onOpenChange,
  status,
  coreVersion,
  theme: choice,
  onThemeChange,
}: SettingsDialogProps) {
  const version = status?.version ?? (coreVersion || "…");

  return (
    <Modal>
      <Modal.Backdrop isOpen={isOpen} onOpenChange={onOpenChange}>
        <Modal.Container placement="center" size="sm">
          <Modal.Dialog>
            {({ close }) => (
              <>
                <Modal.CloseTrigger />
                <Modal.Header>
                  <Modal.Heading>设置</Modal.Heading>
                </Modal.Header>

                <Modal.Body>
                  <div className="flex flex-col gap-4">
                    <section>
                      <SectionTitle>外观</SectionTitle>
                      <div
                        aria-label="主题"
                        className="flex flex-wrap items-center gap-1"
                        role="group"
                      >
                        <FilterButton
                          isActive={choice === "light"}
                          onPress={() => onThemeChange("light")}
                        >
                          浅色
                        </FilterButton>
                        <FilterButton
                          isActive={choice === "dark"}
                          onPress={() => onThemeChange("dark")}
                        >
                          深色
                        </FilterButton>
                        <FilterButton
                          isActive={choice === "system"}
                          onPress={() => onThemeChange("system")}
                        >
                          跟随系统
                        </FilterButton>
                      </div>
                    </section>

                    <Separator />

                    <section>
                      <SectionTitle>关于</SectionTitle>
                      <div className="flex items-baseline gap-2 text-sm">
                        <span className="text-muted">版本</span>
                        <span className="font-mono text-foreground" title={version}>
                          {version}
                        </span>
                      </div>
                    </section>
                  </div>
                </Modal.Body>

                <Modal.Footer>
                  <Button slot="close" variant="ghost" onPress={close}>
                    关闭
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
