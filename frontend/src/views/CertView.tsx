import { useCallback, useEffect, useState } from "react";
import { Alert, Button, Separator, Surface } from "@heroui/react";

import { api } from "../api/client";
import type { CaOverview } from "../api/types";
import { formatErrorNotice } from "../errors";
import { IconDownload, IconShield } from "../icons";
import { Hint, Kv, KvGrid, SectionTitle } from "../shared";

export interface CertViewProps {
  onFeedback: (status: "success" | "warning" | "danger", title: string, detail?: string) => void;
}

/**
 * 证书页（ADR-011：从设置页签独立成页）：共享 CA 的完整操作面（对齐组合根 CaControl）。
 * 安装走系统向导；卸载自动处理当前用户与本机存储。
 */
export function CertView({ onFeedback }: CertViewProps) {
  const [ca, setCa] = useState<CaOverview | null>(null);
  const [caMissing, setCaMissing] = useState(false);

  const reloadCa = useCallback(() => {
    void api
      .ca()
      .then((overview) => {
        setCa(overview);
        setCaMissing(false);
      })
      .catch(() => setCaMissing(true));
  }, []);

  useEffect(() => {
    reloadCa();
  }, [reloadCa]);

  return (
    <div className="min-h-0 flex-1 overflow-y-auto p-5">
      <Surface className="flex flex-col p-0">
        <div className="flex items-center gap-2 border-b border-border px-4 py-3">
          <IconShield className="size-4 text-muted" />
          <h2 className="text-base font-semibold text-foreground">证书</h2>
        </div>
        <div className="p-4">
          <CertPanel ca={ca} caMissing={caMissing} onFeedback={onFeedback} reloadCa={reloadCa} />
        </div>
      </Surface>
    </div>
  );
}

/* ------------------------------------------------------------------ */

function CertPanel({
  ca,
  caMissing,
  onFeedback,
  reloadCa,
}: {
  ca: CaOverview | null;
  caMissing: boolean;
  onFeedback: (status: "success" | "warning" | "danger", title: string, detail?: string) => void;
  reloadCa: () => void;
}) {
  const [certBusy, setCertBusy] = useState(false);

  async function installWizard() {
    try {
      await api.openCertInstallWizard();
      onFeedback("success", "已打开系统证书安装向导（安装位置在向导里选择）");
    } catch (error) {
      const f = formatErrorNotice(error, "打开证书安装向导");
      onFeedback("danger", f.title, f.detail);
    }
  }

  async function uninstallCert() {
    setCertBusy(true);
    try {
      const result = await api.caUninstallCert();
      reloadCa();
      const removed: string[] = [];
      if (result.removed_user) removed.push("当前用户");
      if (result.removed_machine) removed.push("计算机（已提权）");
      if (result.warning) {
        onFeedback("warning", result.warning, removed.length ? `已处理：${removed.join("、")}` : undefined);
      } else if (removed.length) {
        onFeedback("warning", `已卸载根证书（${removed.join("、")}）；被代理的 HTTPS 流量将报不受信任`);
      } else {
        onFeedback("warning", "信任库里没有这张证书，无需卸载");
      }
    } catch (error) {
      const f = formatErrorNotice(error, "卸载根证书");
      onFeedback("danger", f.title, f.detail);
    } finally {
      setCertBusy(false);
    }
  }

  async function saveAs() {
    try {
      const pem = await api.caPem();
      const path = await api.saveTextAs("envboard-root-ca.pem", pem);
      if (path) onFeedback("success", `根证书已保存到 ${path}`);
    } catch (error) {
      const f = formatErrorNotice(error, "下载根证书");
      onFeedback("danger", f.title, f.detail);
    }
  }

  function openManager() {
    api
      .openCertManager()
      .then(() => onFeedback("success", "已打开系统证书管理器"))
      .catch((error) => {
        const f = formatErrorNotice(error, "打开系统证书管理器");
        onFeedback("danger", f.title, f.detail);
      });
  }

  if (caMissing) {
    return (
      <Alert className="mb-4" status="warning">
        <Alert.Indicator />
        <Alert.Content>
          <Alert.Title>共享 CA 未物化</Alert.Title>
          <Alert.Description>
            本宿主没有可用的共享 CA（测试注入的假引擎不物化证书）。以真实引擎启动后刷新。
          </Alert.Description>
        </Alert.Content>
      </Alert>
    );
  }

  const info = ca?.info;
  const trust = ca?.trust;

  return (
    <div className="flex flex-col gap-4">
      {trust?.installed ? (
        <Alert status="success">
          <Alert.Indicator>
            <IconShield className="size-4" />
          </Alert.Indicator>
          <Alert.Content>
            <Alert.Title>根证书已就绪且已受信任</Alert.Title>
            <Alert.Description>
              被代理的 HTTPS 流量可以正常解密与改写。<b>其他设备需单独安装这份证书并配置代理。</b>
            </Alert.Description>
          </Alert.Content>
        </Alert>
      ) : (
        <Alert status="warning">
          <Alert.Indicator>
            <IconShield className="size-4" />
          </Alert.Indicator>
          <Alert.Content>
            <Alert.Title>
              {trust ? "根证书尚未导入系统信任库" : "根证书已就绪"}
            </Alert.Title>
            <Alert.Description>
              {trust
                ? "本机客户端会报证书不受信任：点「安装证书（系统向导）」按提示导入（可选当前用户或计算机）。"
                : "被代理的 HTTPS 流量可以正常解密与改写。其他设备需单独安装这份证书并配置代理。"}
            </Alert.Description>
          </Alert.Content>
        </Alert>
      )}

      <div className="flex flex-wrap items-center gap-2">
        {/* 主操作：IPC 取 PEM 文本 → 原生另存为对话框（打开资源管理器选落点，ADR-008） */}
        <Button size="sm" variant="primary" onPress={saveAs}>
          <IconDownload className="size-4" />
          下载根证书
        </Button>
        <Button size="sm" variant="secondary" onPress={installWizard}>
          <IconShield className="size-4" />
          安装证书（系统向导）
        </Button>
        <Button
          className="btn-action--warn"
          isDisabled={certBusy || !trust?.installed}
          size="sm"
          variant="outline"
          onPress={uninstallCert}
        >
          {certBusy ? "处理中…" : "卸载证书"}
        </Button>
        <Button size="sm" variant="ghost" onPress={openManager}>
          在证书管理器中查看
        </Button>
      </div>

      <Separator />

      <section>
        <SectionTitle>系统信任库</SectionTitle>
        <KvGrid>
          <Kv
            k="根证书"
            v={trust ? (trust.installed ? "已安装" : "未安装") : "…"}
            tone={trust?.installed ? "default" : "warning"}
          />
        </KvGrid>
        {trust?.detail ? <Hint>{trust.detail}</Hint> : null}
        <Hint>
          安装走系统证书向导，装到<b>当前用户</b>还是<b>计算机</b>由你在向导里选
          （选计算机时系统会要求管理员权限）；卸载会自动处理两个位置，涉及计算机根时
          会请求提权。操作可逆。
        </Hint>
      </section>

      <section>
        <SectionTitle>证书信息</SectionTitle>
        <KvGrid>
          <Kv k="主体" v={info?.common_name ?? info?.organization ?? (ca ? "…" : "读取中…")} />
          <Kv k="序列号" v={info?.serial_hex ?? "…"} />
          <Kv k="有效期起" v={info?.not_before ?? "…"} />
          <Kv k="有效期至" v={info?.not_after ?? "…"} />
          <Kv k="签名算法" v={info?.sig_alg ?? "…"} />
          <Kv
            k="公钥"
            v={info ? `${info.pubkey_alg}${info.pubkey_bits ? ` ${info.pubkey_bits}` : ""}${info.pubkey_curve ? ` · ${info.pubkey_curve}` : ""}` : "…"}
          />
          <Kv k="SHA-256 指纹" v={ca?.fingerprint ?? "…"} />
          <Kv k="文件路径" v={ca?.cert_path ?? "…"} />
        </KvGrid>
      </section>

      <Hint>
        其他设备安装：下载根证书文件后自行传输到目标设备，按系统提示安装并信任
        （桌面形态不提供局域网下载直链）。
      </Hint>
    </div>
  );
}
