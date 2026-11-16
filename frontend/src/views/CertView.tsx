import { useCallback, useEffect, useState } from "react";
import { Alert, Button, Separator, Surface } from "@heroui/react";

import { api } from "../api/client";
import type { CaOverview } from "../api/types";
import { formatErrorNotice } from "../errors";
import { IconShield } from "../icons";
import { Hint, Kv, KvGrid, SectionTitle } from "../shared";

export interface CertViewProps {
  onFeedback: (status: "success" | "warning" | "danger", title: string, detail?: string) => void;
}

/**
 * 证书页（ADR-011：从设置页签独立成页）：共享 CA 的完整操作面（对齐组合根 CaControl）。
 * 安装/卸载都直接作用于**当前用户** ROOT 存储（免管理员、可逆），不走向导、不提供下载。
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
    <div className="min-h-0 flex-1 overflow-y-auto p-4">
      <Surface className="mx-auto flex w-full max-w-5xl flex-col p-0">
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
  const [busyAction, setBusyAction] = useState<"install" | "uninstall" | null>(null);

  async function installCert() {
    setBusyAction("install");
    try {
      await api.caInstallCert();
      reloadCa();
      onFeedback("success", "根证书已安装到当前用户信任库");
    } catch (error) {
      const f = formatErrorNotice(error, "安装根证书");
      onFeedback("danger", f.title, f.detail);
    } finally {
      setBusyAction(null);
    }
  }

  async function uninstallCert() {
    setBusyAction("uninstall");
    try {
      const result = await api.caUninstallCert();
      reloadCa();
      if (result.warning) {
        onFeedback("warning", result.warning, result.removed ? "已从当前用户存储卸载" : undefined);
      } else if (result.removed) {
        onFeedback("warning", "已从当前用户信任库卸载根证书；被代理的 HTTPS 流量将报不受信任");
      } else {
        onFeedback("warning", "当前用户信任库里没有这张证书，无需卸载");
      }
    } catch (error) {
      const f = formatErrorNotice(error, "卸载根证书");
      onFeedback("danger", f.title, f.detail);
    } finally {
      setBusyAction(null);
    }
  }

  if (caMissing) {
    return (
      <Alert className="mb-4" status="warning">
        <Alert.Indicator />
        <Alert.Content>
          <Alert.Title>证书信息暂不可用</Alert.Title>
          <Alert.Description>
            无法读取共享 CA。请确认服务已启动；若刚启动，稍等片刻后重试。
          </Alert.Description>
        </Alert.Content>
        <Button size="sm" variant="secondary" onPress={reloadCa}>
          重试
        </Button>
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
              被代理的 HTTPS 流量可以正常解密与改写。
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
                ? "本机客户端会报证书不受信任：点「安装证书」直接写入当前用户信任库（免管理员）。"
                : "被代理的 HTTPS 流量可以正常解密与改写。"}
            </Alert.Description>
          </Alert.Content>
        </Alert>
      )}

      <div className="flex flex-wrap items-center gap-2">
        <Button
          isDisabled={busyAction !== null || trust?.current_user === "installed"}
          isPending={busyAction === "install"}
          size="sm"
          variant="primary"
          onPress={installCert}
        >
          <IconShield className="size-4" />
          安装证书
        </Button>
        <Button
          className="btn-action--warn"
          isDisabled={busyAction !== null || trust?.current_user !== "installed"}
          isPending={busyAction === "uninstall"}
          size="sm"
          variant="outline"
          onPress={uninstallCert}
        >
          卸载证书
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
          安装与卸载都直接作用于<b>当前用户</b>的 ROOT 存储（免管理员、可逆）；没有向导，
          也不写入计算机存储。其他设备需单独安装这份证书并配置代理。
        </Hint>
        {trust?.machine === "installed" ? (
          <Hint>
            检测到<b>计算机存储</b>里也有这张证书：本版本不代删，需要时请在系统证书里手动移除。
          </Hint>
        ) : null}
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
        其他设备：把「文件路径」对应的 <code>envboard-ca-cert.pem</code> 拷到目标设备，
        按系统提示安装并信任。
      </Hint>
    </div>
  );
}
