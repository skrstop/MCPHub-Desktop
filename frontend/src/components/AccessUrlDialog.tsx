import React, { useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Copy, Check, X } from 'lucide-react';
import { useSettingsData } from '@/hooks/useSettingsData';

interface AccessUrlDialogProps {
  open: boolean;
  onClose: () => void;
}

/**
 * 访问地址说明弹框：列出当前 MCPHub 桌面版对外暴露的 HTTP 服务访问地址，
 * 以及不同访问场景（全局路由 / 分组路由 / 单服务路由）的常用 URL 模板，
 * 方便用户在 Cursor / Cherry Studio 等客户端中快速接入。
 */
const AccessUrlDialog: React.FC<AccessUrlDialogProps> = ({ open, onClose }) => {
  const { t } = useTranslation();
  const { exposeHttp, httpPort, routingConfig } = useSettingsData();
  const [copiedKey, setCopiedKey] = useState<string | null>(null);

  const baseUrl = useMemo(() => `http://localhost:${httpPort}`, [httpPort]);

  const items = useMemo(
    () => [
      {
        key: 'global',
        title: t('accessUrl.globalRoute', '全局路由（所有服务聚合）'),
        url: `${baseUrl}/mcp`,
        description: t(
          'accessUrl.globalRouteDescription',
          '聚合所有已启用 MCP Server 的工具/资源。需在「路由配置」中开启全局路由。',
        ),
      },
      {
        key: 'group',
        title: t('accessUrl.groupRoute', '分组路由'),
        url: `${baseUrl}/mcp/{groupName}`,
        description: t(
          'accessUrl.groupRouteDescription',
          '按分组聚合，将 {groupName} 替换为目标分组名称。',
        ),
      },
      {
        key: 'server',
        title: t('accessUrl.serverRoute', '单服务路由'),
        url: `${baseUrl}/mcp/{serverName}`,
        description: t(
          'accessUrl.serverRouteDescription',
          '直连单个 MCP Server，将 {serverName} 替换为服务名称。',
        ),
      },
      {
        key: 'sse',
        title: t('accessUrl.sseRoute', 'SSE 兼容地址'),
        url: `${baseUrl}/sse`,
        description: t(
          'accessUrl.sseRouteDescription',
          '兼容旧版 SSE 客户端的入口地址。',
        ),
      },
    ],
    [baseUrl, t],
  );

  const handleCopy = async (key: string, value: string) => {
    try {
      if (navigator.clipboard && window.isSecureContext) {
        await navigator.clipboard.writeText(value);
      } else {
        const textarea = document.createElement('textarea');
        textarea.value = value;
        textarea.style.position = 'fixed';
        textarea.style.left = '-9999px';
        document.body.appendChild(textarea);
        textarea.focus();
        textarea.select();
        document.execCommand('copy');
        document.body.removeChild(textarea);
      }
      setCopiedKey(key);
      setTimeout(() => setCopiedKey(null), 1500);
    } catch (err) {
      console.error('Copy access url failed', err);
    }
  };

  if (!open) {
    return null;
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/40 px-4"
      onClick={onClose}
    >
      <div
        className="relative w-full max-w-2xl rounded-lg bg-white dark:bg-gray-800 shadow-xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="flex items-center justify-between border-b border-gray-200 dark:border-gray-700 px-5 py-3">
          <h2 className="text-lg font-semibold text-gray-900 dark:text-white">
            {t('accessUrl.title', '服务访问地址')}
          </h2>
          <button
            onClick={onClose}
            className="rounded p-1 text-gray-500 hover:text-gray-900 dark:hover:text-gray-200 hover:bg-gray-100 dark:hover:bg-gray-700"
            aria-label={t('common.close', '关闭')}
          >
            <X className="h-5 w-5" />
          </button>
        </div>

        <div className="space-y-4 px-5 py-4 max-h-[70vh] overflow-y-auto">
          {!exposeHttp && (
            <div className="rounded-md border border-amber-200 bg-amber-50 p-3 text-sm text-amber-700">
              {t(
                'accessUrl.exposeHttpDisabled',
                '当前未开启 HTTP 暴露，外部客户端无法访问。请到「设置 → 路由配置」中开启 HTTP 暴露后再使用以下地址。',
              )}
            </div>
          )}

          <div className="rounded-md bg-gray-50 dark:bg-gray-900 p-3 text-sm text-gray-700 dark:text-gray-300">
            <p>
              {t(
                'accessUrl.intro',
                'MCPHub 桌面版会在本机启动一个 HTTP 服务，下列地址可直接配置到 Cursor、Cherry Studio 等支持 MCP 协议的客户端：',
              )}
            </p>
            <p className="mt-2">
              <span className="font-medium">{t('accessUrl.baseUrl', '基础地址')}：</span>
              <code className="ml-1 rounded bg-gray-200 dark:bg-gray-700 px-1.5 py-0.5 font-mono">
                {baseUrl}
              </code>
            </p>
            {routingConfig?.enableBearerAuth && (
              <p className="mt-2 text-xs text-gray-500 dark:text-gray-400">
                {t(
                  'accessUrl.bearerNotice',
                  '当前已开启 Bearer 鉴权，调用时请在请求头中带上配置的 Bearer Token。',
                )}
              </p>
            )}
          </div>

          <div className="space-y-3">
            {items.map((item) => (
              <div
                key={item.key}
                className="rounded-md border border-gray-200 dark:border-gray-700 p-3"
              >
                <div className="flex items-center justify-between gap-3">
                  <h3 className="text-sm font-medium text-gray-900 dark:text-white">
                    {item.title}
                  </h3>
                  <button
                    type="button"
                    onClick={() => handleCopy(item.key, item.url)}
                    className="inline-flex items-center gap-1 rounded px-2 py-1 text-xs text-blue-600 hover:bg-blue-50 dark:hover:bg-gray-700"
                  >
                    {copiedKey === item.key ? (
                      <>
                        <Check className="h-3.5 w-3.5" />
                        {t('common.copied', '已复制')}
                      </>
                    ) : (
                      <>
                        <Copy className="h-3.5 w-3.5" />
                        {t('common.copy', '复制')}
                      </>
                    )}
                  </button>
                </div>
                <code className="mt-2 block break-all rounded bg-gray-100 dark:bg-gray-900 px-2 py-1.5 font-mono text-xs text-gray-800 dark:text-gray-200">
                  {item.url}
                </code>
                <p className="mt-2 text-xs text-gray-500 dark:text-gray-400">
                  {item.description}
                </p>
              </div>
            ))}
          </div>
        </div>

        <div className="flex justify-end border-t border-gray-200 dark:border-gray-700 px-5 py-3">
          <button
            type="button"
            onClick={onClose}
            className="rounded-md bg-blue-600 px-4 py-2 text-sm font-medium text-white hover:bg-blue-700"
          >
            {t('common.close', '关闭')}
          </button>
        </div>
      </div>
    </div>
  );
};

export default AccessUrlDialog;

