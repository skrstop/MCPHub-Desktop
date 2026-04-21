import { useState, useRef, useEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { Server } from '@/types';
import { ChevronDown, ChevronRight, AlertCircle, Copy, Check, Wrench, MessageSquare, FileText } from 'lucide-react';
import { StatusBadge } from '@/components/ui/Badge';
import ToolCard from '@/components/ui/ToolCard';
import PromptCard from '@/components/ui/PromptCard';
import ResourceCard from '@/components/ui/ResourceCard';
import DeleteDialog from '@/components/ui/DeleteDialog';
import { useToast } from '@/contexts/ToastContext';
import { useSettingsData } from '@/hooks/useSettingsData';

interface ServerCardProps {
  server: Server;
  onRemove: (serverName: string) => void;
  onEdit: (server: Server) => void;
  onToggle?: (server: Server, enabled: boolean) => Promise<boolean>;
  onRefresh?: () => void;
  onReload?: (server: Server) => Promise<boolean>;
}

const ServerCard = ({
  server,
  onRemove,
  onEdit,
  onToggle,
  onRefresh,
  onReload,
}: ServerCardProps) => {
  const { t } = useTranslation();
  const { showToast } = useToast();
  const [expandedTab, setExpandedTab] = useState<'tools' | 'prompts' | 'resources' | null>(null);
  const [showDeleteDialog, setShowDeleteDialog] = useState(false);
  const [isToggling, setIsToggling] = useState(false);
  const [isReloading, setIsReloading] = useState(false);
  const [showErrorPopover, setShowErrorPopover] = useState(false);
  const [copied, setCopied] = useState(false);
  const errorPopoverRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (errorPopoverRef.current && !errorPopoverRef.current.contains(event.target as Node)) {
        setShowErrorPopover(false);
      }
    };

    document.addEventListener('mousedown', handleClickOutside);
    return () => {
      document.removeEventListener('mousedown', handleClickOutside);
    };
  }, []);

  const { exportMCPSettings } = useSettingsData();
  const totalTools = server.tools?.length || 0;
  const enabledTools = server.tools?.filter((tool) => tool.enabled !== false).length || 0;
  const totalPrompts = server.prompts?.length || 0;
  const enabledPrompts = server.prompts?.filter((prompt) => prompt.enabled !== false).length || 0;
  const totalResources = server.resources?.length || 0;
  const enabledResources =
    server.resources?.filter((resource) => resource.enabled !== false).length || 0;

  const handleRemove = (e: React.MouseEvent) => {
    e.stopPropagation();
    setShowDeleteDialog(true);
  };

  const handleEdit = (e: React.MouseEvent) => {
    e.stopPropagation();
    onEdit(server);
  };

  const handleToggle = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (isToggling || !onToggle) return;

    setIsToggling(true);
    try {
      await onToggle(server, !(server.enabled !== false));
    } finally {
      setIsToggling(false);
    }
  };

  const handleReload = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (isReloading || !onReload) return;

    setIsReloading(true);
    try {
      const success = await onReload(server);
      if (success) {
        showToast(t('server.reloadSuccess') || 'Server reloaded successfully', 'success');
      } else {
        showToast(
          t('server.reloadError', { serverName: server.name }) || 'Failed to reload server',
          'error',
        );
      }
    } finally {
      setIsReloading(false);
    }
  };

  const handleErrorIconClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    setShowErrorPopover(!showErrorPopover);
  };

  const copyToClipboard = (e: React.MouseEvent) => {
    e.stopPropagation();
    if (!server.error) return;

    if (navigator.clipboard && window.isSecureContext) {
      navigator.clipboard.writeText(server.error).then(() => {
        setCopied(true);
        showToast(t('common.copySuccess') || 'Copied to clipboard', 'success');
        setTimeout(() => setCopied(false), 2000);
      });
    } else {
      // Fallback for HTTP or unsupported clipboard API
      const textArea = document.createElement('textarea');
      textArea.value = server.error;
      // Avoid scrolling to bottom
      textArea.style.position = 'fixed';
      textArea.style.left = '-9999px';
      document.body.appendChild(textArea);
      textArea.focus();
      textArea.select();
      try {
        document.execCommand('copy');
        setCopied(true);
        showToast(t('common.copySuccess') || 'Copied to clipboard', 'success');
        setTimeout(() => setCopied(false), 2000);
      } catch (err) {
        showToast(t('common.copyFailed') || 'Copy failed', 'error');
        console.error('Copy to clipboard failed:', err);
      }
      document.body.removeChild(textArea);
    }
  };

  const handleCopyServerConfig = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      const result = await exportMCPSettings(server.name);
      if (!result || !result.success || !result.data) {
        showToast(result?.message || t('common.copyFailed') || 'Copy failed', 'error');
        return;
      }
      const configJson =
        typeof result.data === 'string' ? result.data : JSON.stringify(result.data, null, 2);

      if (navigator.clipboard && window.isSecureContext) {
        await navigator.clipboard.writeText(configJson);
        showToast(t('common.copySuccess') || 'Copied to clipboard', 'success');
      } else {
        // Fallback for HTTP or unsupported clipboard API
        const textArea = document.createElement('textarea');
        textArea.value = configJson;
        textArea.style.position = 'fixed';
        textArea.style.left = '-9999px';
        document.body.appendChild(textArea);
        textArea.focus();
        textArea.select();
        try {
          document.execCommand('copy');
          showToast(t('common.copySuccess') || 'Copied to clipboard', 'success');
        } catch (err) {
          showToast(t('common.copyFailed') || 'Copy failed', 'error');
          console.error('Copy to clipboard failed:', err);
        }
        document.body.removeChild(textArea);
      }
    } catch (error) {
      console.error('Error copying server configuration:', error);
      showToast(t('common.copyFailed') || 'Copy failed', 'error');
    }
  };

  const handleConfirmDelete = () => {
    onRemove(server.name);
    setShowDeleteDialog(false);
  };

  const handleToolToggle = async (toolName: string, enabled: boolean) => {
    try {
      const { toggleTool } = await import('@/services/toolService');
      const result = await toggleTool(server.name, toolName, enabled);
      if (result.success) {
        showToast(
          t(enabled ? 'tool.enableSuccess' : 'tool.disableSuccess', { name: toolName }),
          'success',
        );
        // Trigger refresh to update the tool's state in the UI
        if (onRefresh) {
          onRefresh();
        }
      } else {
        showToast(result.error || t('tool.toggleFailed'), 'error');
      }
    } catch (error) {
      console.error('Error toggling tool:', error);
      showToast(t('tool.toggleFailed'), 'error');
    }
  };

  const handlePromptToggle = async (promptName: string, enabled: boolean) => {
    try {
      const { togglePrompt } = await import('@/services/promptService');
      const result = await togglePrompt(server.name, promptName, enabled);
      if (result.success) {
        showToast(
          t(enabled ? 'tool.enableSuccess' : 'tool.disableSuccess', { name: promptName }),
          'success',
        );
        // Trigger refresh to update the prompt's state in the UI
        if (onRefresh) {
          onRefresh();
        }
      } else {
        showToast(result.error || t('tool.toggleFailed'), 'error');
      }
    } catch (error) {
      console.error('Error toggling prompt:', error);
      showToast(t('tool.toggleFailed'), 'error');
    }
  };

  const handleToolDescriptionUpdate = (
    _toolName: string,
    _description: string,
    options?: { restored?: boolean },
  ) => {
    showToast(
      options?.restored ? t('tool.restoreDefaultSuccess') : t('tool.descriptionUpdateSuccess'),
      'success',
    );
    if (onRefresh) {
      onRefresh();
    }
  };

  const handlePromptDescriptionUpdate = (
    _promptName: string,
    _description: string,
    options?: { restored?: boolean },
  ) => {
    showToast(
      options?.restored ? t('prompt.restoreDefaultSuccess') : t('prompt.descriptionUpdateSuccess'),
      'success',
    );
    if (onRefresh) {
      onRefresh();
    }
  };

  const handleOAuthAuthorization = (e: React.MouseEvent) => {
    e.stopPropagation();
    // Open the OAuth authorization URL in a new window
    if (server.oauth?.authorizationUrl) {
      const width = 600;
      const height = 700;
      const left = window.screen.width / 2 - width / 2;
      const top = window.screen.height / 2 - height / 2;

      window.open(
        server.oauth.authorizationUrl,
        'OAuth Authorization',
        `width=${width},height=${height},left=${left},top=${top}`,
      );

      showToast(t('status.oauthWindowOpened'), 'info');
    }
  };

  const handleResourceToggle = async (resourceUri: string, enabled: boolean) => {
    try {
      const { toggleResource } = await import('@/services/resourceService');
      const result = await toggleResource(server.name, resourceUri, enabled);
      if (result.success) {
        showToast(
          t(enabled ? 'tool.enableSuccess' : 'tool.disableSuccess', { name: resourceUri }),
          'success',
        );
        if (onRefresh) {
          onRefresh();
        }
      } else {
        showToast(result.error || t('tool.toggleFailed'), 'error');
      }
    } catch (error) {
      console.error('Error toggling resource:', error);
      showToast(t('tool.toggleFailed'), 'error');
    }
  };

  const handleResourceDescriptionUpdate = async (
    resourceUri: string,
    description: string,
    options?: { restored?: boolean },
  ) => {
    try {
      const { updateResourceDescription, resetResourceDescription } = await import(
        '@/services/resourceService'
      );
      const result = options?.restored
        ? await resetResourceDescription(server.name, resourceUri)
        : await updateResourceDescription(server.name, resourceUri, description);
      if (result.success) {
        showToast(
          options?.restored
            ? t('builtinResources.restoreDefaultSuccess')
            : t('builtinResources.descriptionUpdateSuccess'),
          'success',
        );
        if (onRefresh) {
          onRefresh();
        }
      } else {
        showToast(
          result.error ||
            (options?.restored
              ? t('builtinResources.restoreDefaultFailed')
              : t('builtinResources.descriptionUpdateFailed')),
          'error',
        );
      }
    } catch (error) {
      console.error('Error updating resource description:', error);
      showToast(
        options?.restored
          ? t('builtinResources.restoreDefaultFailed')
          : t('builtinResources.descriptionUpdateFailed'),
        'error',
      );
    }
  };

  return (
    <>
      <div className="bg-white shadow rounded-lg mb-4 page-card transition-all duration-200">
        <div className="flex justify-between items-start gap-3 p-4">
          {/* Left: name/description row + badges row */}
          <div className="flex-1 min-w-0 flex flex-col gap-1.5">
            {/* Row 1: server name + description */}
            <div className="flex items-baseline gap-2 min-w-0">
              <h2
                className={`text-base font-semibold leading-tight shrink-0 ${server.enabled === false ? 'text-gray-500' : 'text-gray-900'}`}
              >
                {server.name}
              </h2>
              {server.config?.description && (
                <span className="text-xs text-gray-500 truncate min-w-0">({server.config.description})</span>
              )}
            </div>
            {/* Row 2: status badges */}
            <div className="flex flex-wrap items-center gap-1.5">
            <StatusBadge status={server.status} onAuthClick={handleOAuthAuthorization} />

            {/* Server type badge */}
            {server.config?.type && (
              <div className="server-badge server-badge-gray">
                <span className="whitespace-nowrap">
                  {server.config.type === 'stdio' && t('server.typeStdio')}
                  {server.config.type === 'sse' && t('server.typeSse')}
                  {server.config.type === 'streamable-http' && t('server.typeStreamableHttp')}
                  {server.config.type === 'openapi' && t('server.typeOpenapi')}
                </span>
              </div>
            )}

            {/* Tool count display */}
            <div
              className={`server-badge cursor-pointer ${expandedTab === 'tools' ? 'server-badge-blue-active' : 'server-badge-blue'}`}
              onClick={(e) => {
                e.stopPropagation();
                setExpandedTab(prev => prev === 'tools' ? null : 'tools');
              }}
            >
              <Wrench className="w-3 h-3 mr-1 shrink-0" />
              <span className="whitespace-nowrap">
                {totalTools === 0 ? '0' : `${enabledTools}/${totalTools}`} {t('server.tools')}
              </span>
            </div>

            {/* Prompt count display */}
            <div
              className={`server-badge cursor-pointer ${expandedTab === 'prompts' ? 'server-badge-purple-active' : 'server-badge-purple'}`}
              onClick={(e) => {
                e.stopPropagation();
                setExpandedTab(prev => prev === 'prompts' ? null : 'prompts');
              }}
            >
              <MessageSquare className="w-3 h-3 mr-1 shrink-0" />
              <span className="whitespace-nowrap">
                {totalPrompts === 0 ? '0' : `${enabledPrompts}/${totalPrompts}`} {t('server.prompts')}
              </span>
            </div>

            {/* Resource count display */}
            <div
              className={`server-badge cursor-pointer ${expandedTab === 'resources' ? 'server-badge-emerald-active' : 'server-badge-emerald'}`}
              onClick={(e) => {
                e.stopPropagation();
                setExpandedTab(prev => prev === 'resources' ? null : 'resources');
              }}
            >
              <FileText className="w-3 h-3 mr-1 shrink-0" />
              <span className="whitespace-nowrap">
                {totalResources === 0 ? '0' : `${enabledResources}/${totalResources}`} {t('nav.resources')}
              </span>
            </div>

            {server.error && (
              <div className="relative">
                <div
                  className="cursor-pointer"
                  onClick={handleErrorIconClick}
                  aria-label={t('server.viewErrorDetails')}
                >
                  <AlertCircle className="text-red-500 hover:text-red-600" size={16} />
                </div>

                {showErrorPopover && (
                  <div
                    ref={errorPopoverRef}
                    className="absolute z-10 mt-2 bg-white border border-gray-200 rounded-md shadow-lg p-0 w-120"
                    style={{
                      left: '-231px',
                      top: '24px',
                      maxHeight: '300px',
                      overflowY: 'auto',
                      width: '480px',
                      transform: 'translateX(50%)',
                    }}
                    onClick={(e) => e.stopPropagation()}
                  >
                    <div className="flex justify-between items-center sticky top-0 bg-white py-2 px-4 border-b border-gray-200 z-20 shadow-sm">
                      <div className="flex items-center space-x-2">
                        <h4 className="text-sm font-medium text-red-600">
                          {t('server.errorDetails')}
                        </h4>
                        <button
                          onClick={copyToClipboard}
                          className="p-1 text-gray-400 hover:text-gray-600 transition-colors btn-secondary"
                          title={t('common.copy')}
                        >
                          {copied ? (
                            <Check size={14} className="text-green-500" />
                          ) : (
                            <Copy size={14} />
                          )}
                        </button>
                      </div>
                      <button
                        onClick={(e) => {
                          e.stopPropagation();
                          setShowErrorPopover(false);
                        }}
                        className="text-gray-400 hover:text-gray-600"
                      >
                        ✕
                      </button>
                    </div>
                    <div className="p-4 pt-2">
                      <pre className="text-sm text-gray-700 break-words whitespace-pre-wrap">
                        {server.error}
                      </pre>
                    </div>
                  </div>
                )}
              </div>
            )}
            </div>{/* end badges row */}
          </div>{/* end left column */}

          {/* Right: action buttons */}
          <div className="flex items-center flex-wrap gap-1.5 shrink-0">
            <button onClick={handleCopyServerConfig} className="server-btn server-btn-secondary">
              {t('server.copy')}
            </button>
            <button
              onClick={handleEdit}
              className="server-btn server-btn-primary"
            >
              {t('server.edit')}
            </button>
            <button
              onClick={handleToggle}
              className={`server-btn ${
                isToggling
                  ? 'server-btn-disabled'
                  : server.enabled !== false
                    ? 'server-btn-secondary'
                    : 'server-btn-primary'
              }`}
              disabled={isToggling || isReloading}
            >
              {isToggling
                ? t('common.processing')
                : server.enabled !== false
                  ? t('server.disable')
                  : t('server.enable')}
            </button>
            {onReload && (
              <button
                onClick={handleReload}
                className="server-btn server-btn-secondary disabled:opacity-50 disabled:cursor-not-allowed"
                disabled={isReloading || isToggling || server.enabled === false}
              >
                {isReloading ? t('common.processing') : t('server.reload')}
              </button>
            )}
            <button
              onClick={handleRemove}
              className="server-btn server-btn-danger"
            >
              {t('server.delete')}
            </button>
            <button
              className="p-1 text-gray-400 hover:text-gray-600 transition-colors"
              onClick={(e) => e.stopPropagation()}
            >
              {expandedTab ? <ChevronDown size={16} /> : <ChevronRight size={16} />}
            </button>
          </div>
        </div>

        {expandedTab === 'tools' && server.tools && (
          <div className="px-4">
            <h6
              className={`font-medium ${server.enabled === false ? 'text-gray-600' : 'text-gray-900'} mb-2`}
            >
              {t('server.tools')}
            </h6>
            <div className="space-y-4">
              {server.tools.map((tool, index) => (
                <ToolCard
                  key={index}
                  server={server.name}
                  tool={tool}
                  onToggle={handleToolToggle}
                  onDescriptionUpdate={handleToolDescriptionUpdate}
                />
              ))}
            </div>
          </div>
        )}

        {expandedTab === 'prompts' && server.prompts && (
          <div className="px-4 pb-2">
            <h6
              className={`font-medium ${server.enabled === false ? 'text-gray-600' : 'text-gray-900'}`}
            >
              {t('server.prompts')}
            </h6>
            <div className="space-y-4">
              {server.prompts.map((prompt, index) => (
                <PromptCard
                  key={index}
                  server={server.name}
                  prompt={prompt}
                  onToggle={handlePromptToggle}
                  onDescriptionUpdate={handlePromptDescriptionUpdate}
                />
              ))}
            </div>
          </div>
        )}

        {expandedTab === 'resources' && server.resources && (
          <div className="px-4 pb-2">
            <h6
              className={`font-medium ${server.enabled === false ? 'text-gray-600' : 'text-gray-900'}`}
            >
              {t('nav.resources')}
            </h6>
            {server.resources.length === 0 ? (
              <div className="text-sm text-gray-500 py-2">
                {t('builtinResources.noResources')}
              </div>
            ) : (
              <div className="space-y-4">
                {server.resources.map((resource, index) => (
                  <ResourceCard
                    key={`${resource.uri}-${index}`}
                    resource={resource}
                    onToggle={handleResourceToggle}
                    onDescriptionUpdate={handleResourceDescriptionUpdate}
                  />
                ))}
              </div>
            )}
          </div>
        )}
      </div>

      <DeleteDialog
        isOpen={showDeleteDialog}
        onClose={() => setShowDeleteDialog(false)}
        onConfirm={handleConfirmDelete}
        serverName={server.name}
      />
    </>
  );
};

export default ServerCard;
