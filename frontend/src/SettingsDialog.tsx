import { useState } from "react";
import { Button, Modal, ScrollShadow, Separator } from "@heroui/react";

import type { StatusInfo } from "./api/types";
import { formatPortRange } from "./data";
import { IconGear } from "./icons";
import { FilterButton, Hint, Kv, KvGrid, SectionTitle } from "./shared";
import { rememberTheme, storedChoice, type ThemeChoice } from "./theme";

export interface SettingsDialogProps {
  isOpen: boolean;
  onOpenChange: (isOpen: boolean) => void;
  status: StatusInfo | null;
  coreVersion: string;
  onFeedback: (status: "success" | "warning" | "danger", title: string, detail?: string) => void;
}

/**
 * 全局设置模态（ADR-011：原设置页删除，仅保留非证书的运行时信息与主题）。
 * 证书独立成页（views/CertView.tsx），这里不再出现任何证书入口。
 */
export function SettingsDialog({
  isOpen,
  onOpenChange,
  status,
  coreVersion,
  onFeedback,
}: SettingsDialogProps) {
  const [choice, setChoice] = useState<ThemeChoice>(() => storedChoice());

  const setTheme = (next: ThemeChoice) => {
    setChoice(next);
    rememberTheme(next);
  };

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
                    <IconGear className="size-5" />
                  </Modal.Icon>
                  <Modal.Heading>设置</Modal.Heading>
                </Modal.Header>

                <Modal.Body>
                  <ScrollShadow className="max-h-96">
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
                            onPress={() => {
                              setTheme("light");
                              onFeedback("success", "已切换为浅色主题");
                            }}
                          >
                            浅色
                          </FilterButton>
                          <FilterButton
                            isActive={choice === "dark"}
                            onPress={() => {
                              setTheme("dark");
                              onFeedback("success", "已切换为深色主题");
                            }}
                          >
                            深色
                          </FilterButton>
                          <FilterButton
                            isActive={choice === "system"}
                            onPress={() => {
                              setTheme("system");
                              onFeedback("success", "主题已改为跟随系统");
                            }}
                          >
                            跟随系统
                          </FilterButton>
                        </div>
                        <Hint>主题偏好保存在本机（应用持久 profile）；清除应用数据会回到默认深色。</Hint>
                      </section>

                      <Separator />

                      <section>
                        <SectionTitle>运行时</SectionTitle>
                        <KvGrid>
                          <Kv k="版本" v={status?.version ?? (coreVersion || "…")} />
                          <Kv k="端口区间" v={formatPortRange(status?.config.port_range)} />
                          <Kv k="状态目录" v={status?.config.state_dir ?? "…"} />
                          <Kv
                            k="事件丢弃"
                            v={status ? String(status.events_dropped) : "…"}
                            tone={status && status.events_dropped > 0 ? "warning" : "default"}
                          />
                        </KvGrid>
                      </section>
                    </div>
                  </ScrollShadow>
                </Modal.Body>

                <Modal.Footer>
                  <Button slot="close" variant="secondary" onPress={close}>
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
