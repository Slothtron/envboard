import { useEffect, useMemo, useState } from "react";
import { Alert, Button, Separator, Surface, Tabs } from "@heroui/react";

import { api, withToken } from "../api/client";
import type { StatusInfo } from "../api/types";
import { IconDownload, IconGear, IconShield } from "../icons";
import { Hint, Kv, KvGrid, SectionTitle } from "../shared";
import { type ThemeChoice, rememberTheme, storedChoice } from "../theme";
import { FilterButton } from "../shared";

interface CertInfo {
  version?: number;
  serial_hex?: string;
  not_before?: string;
  not_after?: string;
  sig_alg?: string;
  pubkey_alg?: string;
  pubkey_curve?: string | null;
  pubkey_bits?: number | null;
  common_name?: string | null;
  organization?: string | null;
}

export interface SettingsViewProps {
  coreVersion: string;
  onFeedback: (status: "success" | "warning" | "danger", title: string) => void;
}

export function SettingsView({ coreVersion, onFeedback }: SettingsViewProps) {
  const [issued, setIssued] = useState<ThemeChoice>(() => storedChoice());
  const [status, setStatus] = useState<StatusInfo | null>(null);
  const [ca, setCa] = useState<CertInfo | null>(null);
  const [caMissing, setCaMissing] = useState(false);

  useEffect(() => {
    void api.status().then(setStatus).catch(() => setStatus(null));
    void api
      .ca()
      .then((info) => setCa(info as CertInfo))
      .catch(() => setCaMissing(true));
  }, []);

  const setTheme = (choice: ThemeChoice) => {
    setIssued(choice);
    rememberTheme(choice);
  };

  return (
    <div className="min-h-0 flex-1 overflow-y-auto p-5">
      <Surface className="flex flex-col p-0">
        <div className="flex items-center gap-2 border-b border-border px-4 py-3">
          <IconGear className="size-4 text-muted" />
          <h2 className="text-base font-semibold text-foreground">设置</h2>
        </div>

        <Tabs align="start" defaultSelectedKey="cert" variant="secondary">
          <Tabs.ListContainer>
            <Tabs.List aria-label="设置">
              <Tabs.Tab id="cert">
                证书
                <Tabs.Indicator />
              </Tabs.Tab>
              <Tabs.Tab id="general">
                通用
                <Tabs.Indicator />
              </Tabs.Tab>
              <Tabs.Tab id="about">
                关于
                <Tabs.Indicator />
              </Tabs.Tab>
            </Tabs.List>
          </Tabs.ListContainer>

          <Tabs.Panel className="p-4" id="cert">
            <CertTab ca={ca} caMissing={caMissing} onFeedback={onFeedback} />
          </Tabs.Panel>

          <Tabs.Panel className="p-4" id="general">
            <div className="flex flex-col gap-4">
              <section>
                <SectionTitle>外观</SectionTitle>
                <div aria-label="主题" className="flex flex-wrap items-center gap-1" role="group">
                  <FilterButton
                    isActive={issued === "light"}
                    onPress={() => {
                      setTheme("light");
                      onFeedback("success", "已切换为浅色主题");
                    }}
                  >
                    浅色
                  </FilterButton>
                  <FilterButton
                    isActive={issued === "dark"}
                    onPress={() => {
                      setTheme("dark");
                      onFeedback("success", "已切换为深色主题");
                    }}
                  >
                    深色
                  </FilterButton>
                  <FilterButton
                    isActive={issued === "system"}
                    onPress={() => {
                      setTheme("system");
                      onFeedback("success", "主题已改为跟随系统");
                    }}
                  >
                    跟随系统
                  </FilterButton>
                </div>
              </section>
              <KvGrid>
                <Kv k="版本" v={status?.version ?? (coreVersion || "…")} />
                <Kv
                  k="端口区间"
                  v={
                    status
                      ? Array.isArray(status.config.port_range)
                        ? `${status.config.port_range[0]}-${status.config.port_range[1]}`
                        : `${status.config.port_range.start}-${status.config.port_range.end}`
                      : "…"
                  }
                />
                <Kv k="状态目录" v={status?.config.state_dir ?? "…"} />
                <Kv
                  k="事件丢弃"
                  v={status ? String(status.events_dropped) : "…"}
                  tone={status && status.events_dropped > 0 ? "warning" : "default"}
                />
              </KvGrid>
            </div>
          </Tabs.Panel>

          <Tabs.Panel className="p-4" id="about">
            <Hint>
              envboard —— 一个环境 = 一个实例 = 一个端口。前端为 Vite + HeroUI 工程，构建产物内嵌进单二进制。
            </Hint>
          </Tabs.Panel>
        </Tabs>
      </Surface>
    </div>
  );
}

/* ------------------------------------------------------------------ */

function CertTab({
  ca,
  caMissing,
  onFeedback,
}: {
  ca: CertInfo | null;
  caMissing: boolean;
  onFeedback: (status: "success" | "warning" | "danger", title: string) => void;
}) {
  const pemUrl = useMemo(() => withToken("/api/ca.pem"), []);

  if (caMissing) {
    return (
      <Alert className="mb-4" status="warning">
        <Alert.Indicator />
        <Alert.Content>
          <Alert.Title>共享 CA 未初始化</Alert.Title>
          <Alert.Description>
            组合根未能读出 CA 摘要。确认状态目录下的 ca.key / ca.crt 存在且以共享 CA 模式启动后刷新。
          </Alert.Description>
        </Alert.Content>
      </Alert>
    );
  }

  return (
    <div className="flex flex-col gap-4">
      <Alert status="success">
        <Alert.Indicator>
          <IconShield className="size-4" />
        </Alert.Indicator>
        <Alert.Content>
          <Alert.Title>根证书已就绪</Alert.Title>
          <Alert.Description>
            被代理的 HTTPS 流量可以正常解密与改写。<b>其他设备需单独安装这份证书并配置代理。</b>
          </Alert.Description>
        </Alert.Content>
      </Alert>

      <div className="flex flex-wrap items-center gap-2">
        {/* 该视图唯一的主操作 */}
        <a href={pemUrl} rel="noreferrer" download="envboard-root-ca.pem">
          <Button size="sm" variant="primary" onPress={() => onFeedback("success", "根证书下载已开始")}>
            <IconDownload className="size-4" />
            下载根证书
          </Button>
        </a>
        <Button
          size="sm"
          variant="secondary"
          onPress={async () => {
            try {
              await navigator.clipboard.writeText(`${window.location.origin}${pemUrl}`);
              onFeedback("success", "证书下载地址已复制（含 token），发到手机浏览器打开即可安装");
            } catch {
              onFeedback("danger", "复制失败，请手动选择地址");
            }
          }}
        >
          复制下载地址
        </Button>
      </div>

      <Separator />

      <section>
        <SectionTitle>证书信息</SectionTitle>
        <KvGrid>
          <Kv k="主体" v={ca?.common_name ?? ca?.organization ?? (ca ? "…" : "读取中…")} />
          <Kv k="序列号" v={ca?.serial_hex ?? "…"} />
          <Kv k="有效期起" v={ca?.not_before ?? "…"} />
          <Kv k="有效期至" v={ca?.not_after ?? "…"} />
          <Kv k="签名算法" v={ca?.sig_alg ?? "…"} />
          <Kv
            k="公钥"
            v={ca ? `${ca.pubkey_alg}${ca.pubkey_bits ? ` ${ca.pubkey_bits}` : ""}${ca.pubkey_curve ? ` · ${ca.pubkey_curve}` : ""}` : "…"}
          />
        </KvGrid>
      </section>

      <Hint>
        根证书是进程常量（运行期不轮换）；私钥永不回显。手机等设备安装：浏览器打开下载地址获取 PEM，
        按系统提示安装并信任。
      </Hint>
    </div>
  );
}
