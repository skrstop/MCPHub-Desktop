import React, { useState, useMemo, useEffect, useRef, useCallback } from 'react';
import { useTranslation } from 'react-i18next';
import { Skill, ScannedSkill, SkillAgent, ExportResultItem } from '@/types';
import { useSkillData } from '@/hooks/useSkillData';
import { useAuth } from '@/contexts/AuthContext';
import { useToast } from '@/contexts/ToastContext';
import {
  Plus,
  Search,
  X,
  Eye,
  ChevronDown,
  HelpCircle,
  Trash2,
  Loader2,
  Link2,
  Copy as CopyIcon,
  Upload,
  Folder,
  PackageCheck,
  AlertTriangle,
  FolderPlus,
} from 'lucide-react';
import Pagination from '@/components/ui/Pagination';
import ConfirmDialog from '@/components/ui/ConfirmDialog';
import { selectItemPage } from '@/utils/listFilters';
import {
  scanSkillsForImport,
  scanFolderForSkills,
  getSkill,
  listSkillAgents,
  openAgentPath,
  openSkillLibrary,
  pickDirectory,
  createSkillAgent,
  deleteSkillAgent,
} from '@/services/skillService';

// ───────────────────────────────────────────────────────────────────────────
// Method help icon (?): click to toggle a popover explaining the difference
// and advantages of symlink vs file copy. Placed next to the "Target Agents"
// title in the install/export dialogs.
// ───────────────────────────────────────────────────────────────────────────
const MethodHelpIcon: React.FC = () => {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const cancelClose = () => {
    if (closeTimer.current) {
      clearTimeout(closeTimer.current);
      closeTimer.current = null;
    }
  };
  const scheduleClose = () => {
    cancelClose();
    // Small delay bridges the gap between the button and the absolutely
    // positioned popover so the user can move the pointer into the popover
    // without it dismissing.
    closeTimer.current = setTimeout(() => setOpen(false), 150);
  };
  const handleEnter = () => {
    cancelClose();
    setOpen(true);
  };
  const handleLeave = () => {
    scheduleClose();
  };
  const handleClick = (e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    cancelClose();
    setOpen(true);
  };

  useEffect(() => () => cancelClose(), []);

  return (
    <span className="relative inline-flex">
      <button
        type="button"
        onClick={handleClick}
        onMouseEnter={handleEnter}
        onMouseLeave={handleLeave}
        className="hub-icon-btn sm"
        title={t('skills.exportMethodHelp')}
        style={{ color: 'var(--hub-ink-3)' }}
      >
        <HelpCircle size={13} />
      </button>
      {open && (
        <div
          className="absolute z-50 left-0 top-full mt-1 w-[280px] text-[12px] space-y-1.5 shadow-lg"
          style={{
            padding: '10px 12px',
            background: 'var(--hub-surface)',
            border: '1px solid var(--hub-line)',
            borderRadius: 8,
            color: 'var(--hub-ink-2)',
          }}
          onMouseEnter={handleEnter}
          onMouseLeave={handleLeave}
        >
          <div className="flex items-start gap-1.5">
            <Link2 size={12} className="mt-0.5 flex-shrink-0" style={{ color: 'var(--hub-accent)' }} />
            <div>
              <span className="font-medium" style={{ color: 'var(--hub-ink)' }}>
                {t('skills.symlink')}:
              </span>{' '}
              {t('skills.symlinkHelp')}
            </div>
          </div>
          <div className="flex items-start gap-1.5">
            <CopyIcon size={12} className="mt-0.5 flex-shrink-0" style={{ color: 'var(--hub-accent)' }} />
            <div>
              <span className="font-medium" style={{ color: 'var(--hub-ink)' }}>
                {t('skills.fileCopy')}:
              </span>{' '}
              {t('skills.fileCopyHelp')}
            </div>
          </div>
        </div>
      )}
    </span>
  );
};

// ───────────────────────────────────────────────────────────────────────────
// Import dialog: scan all agents, group by agent, allow selecting skills to
// import. Skills already in the library (matched by dir_name) are disabled.
// ───────────────────────────────────────────────────────────────────────────
interface ImportDialogProps {
  onImport: (
    items: Array<{ agentId: string; dirName: string; path?: string }>,
  ) => Promise<{ success: boolean; message?: string }>;
  onClose: () => void;
}

const ImportDialog: React.FC<ImportDialogProps> = ({ onImport, onClose }) => {
  const { t } = useTranslation();
  const { showToast } = useToast();
  const [scanning, setScanning] = useState(true);
  const [scanned, setScanned] = useState<ScannedSkill[]>([]);
  const [agents, setAgents] = useState<SkillAgent[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Set<string>>(new Set()); // key = agentId::dirName
  // Default empty → all groups collapsed; user expands the ones they want.
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [importing, setImporting] = useState(false);
  // Manual folder selection (no source agent): skills found by scan-folder.
  const [manualSkills, setManualSkills] = useState<ScannedSkill[]>([]);
  const [manualFolderPath, setManualFolderPath] = useState('');

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        setScanning(true);
        // Fetch configured agents + scanned skills in parallel. All configured
        // agents are shown (even with 0 scanned skills), default collapsed.
        const [agentList, data] = await Promise.all([listSkillAgents(), scanSkillsForImport()]);
        if (cancelled) return;
        setAgents(agentList);
        setScanned(data);
      } catch (err) {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : t('skills.scanError'));
      } finally {
        if (!cancelled) setScanning(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [t]);

  const handleSelectFolder = async () => {
    try {
      const folder = await pickDirectory();
      if (!folder) return; // cancelled
      const found = await scanFolderForSkills(folder);
      if (found.length === 0) {
        showToast(t('skills.noSkillFound'), 'info');
        return;
      }
      setManualFolderPath(folder);
      setManualSkills((prev) => {
        // Merge by path (avoid duplicates if the same folder re-picked).
        const byPath = new Map(prev.map((s) => [s.path, s]));
        for (const s of found) byPath.set(s.path, s);
        return Array.from(byPath.values());
      });
      // Default-select the importable manual skills (ignore symlinks +
      // already-imported). Keys: __manual__::<dirName>.
      const importable = found.filter((s) => !s.isSymlink && !s.alreadyImported);
      if (importable.length > 0) {
        setSelected((prev) => {
          const next = new Set(prev);
          for (const s of importable) next.add(`__manual__::${s.dirName}`);
          return next;
        });
      }
      setExpanded((prev) => new Set(prev).add('__manual__')); // auto-expand
    } catch (err) {
      showToast(err instanceof Error ? err.message : t('skills.scanError'), 'error');
    }
  };

  // One group per configured agent (even if 0 skills), using the agent's
  // configured skillsPath (so empty agents still show their path + open btn).
  // The "__manual__" group (manual folder selection) is PREPENDED (pinned top).
  const groups = useMemo(() => {
    const agentGroups = agents.map((a) => ({
      agentId: a.id,
      agentName: a.name,
      agentPath: a.skillsPath,
      skills: scanned.filter((s) => s.agentId === a.id),
    }));
    if (manualSkills.length > 0) {
      agentGroups.unshift({
        agentId: '__manual__',
        agentName: t('skills.manualSelection'),
        agentPath: manualFolderPath,
        skills: manualSkills,
      });
    }
    return agentGroups;
  }, [agents, scanned, manualSkills, manualFolderPath, t]);

  const handleOpenPath = async (path: string, e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await openAgentPath(path);
      showToast(t('skills.openingPath', { path }), 'info');
    } catch (err) {
      showToast(err instanceof Error ? err.message : t('skills.openPathError'), 'error');
    }
  };

  const toggleSelect = (key: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  const toggleGroup = (agentId: string) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(agentId)) next.delete(agentId);
      else next.add(agentId);
      return next;
    });
  };

  const handleSubmit = async () => {
    // Build a lookup key→skill so manual items can carry their `path`.
    const allSkills = [...scanned, ...manualSkills];
    const byKey = new Map(allSkills.map((s) => [`${s.agentId}::${s.dirName}`, s]));
    const items: Array<{ agentId: string; dirName: string; path?: string }> = [];
    for (const key of selected) {
      const [agentId, dirName] = key.split('::');
      const skill = byKey.get(key);
      // Manual skills (agentId === '__manual__') send their `path` so the
      // backend imports from that folder with NO source-agent record.
      const path = agentId === '__manual__' ? skill?.path : undefined;
      items.push({ agentId, dirName, path });
    }
    setImporting(true);
    const result = await onImport(items);
    setImporting(false);
    if (result.success) {
      onClose();
    } else {
      setError(result.message || t('skills.importError'));
    }
  };

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-4xl w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[90vh] flex flex-col">
        <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
          <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">
            {t('skills.importDialogTitle')}
          </h2>
          <button onClick={onClose} className="hub-icon-btn sm">
            <X size={16} />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto p-5">
          {scanning ? (
            <div className="flex items-center justify-center gap-2 py-10 text-[var(--hub-ink-3)]">
              <Loader2 size={16} className="animate-spin" />
              <span className="text-[13px]">{t('skills.scanLoading')}</span>
            </div>
          ) : error ? (
            <div className="bg-red-50 border-l-4 border-red-500 text-red-700 p-4 rounded-md text-sm">
              {error}
            </div>
          ) : groups.length === 0 ? (
            <div className="py-10 text-center text-[var(--hub-ink-3)] text-[13px]">
              {t('skills.noAgents')}
            </div>
          ) : (
            <div className="space-y-2">
              {groups.map((g) => {
                const isCollapsed = !expanded.has(g.agentId);
                const selectableCount = g.skills.filter(
                  (s) => !s.alreadyImported && !s.isSymlink,
                ).length;
                return (
                <div key={g.agentId} className="hub-card overflow-hidden" style={{ background: 'var(--hub-surface)' }}>
                  <div
                    className="flex items-center justify-between cursor-pointer transition-colors hover:bg-[var(--hub-surface-hover)]"
                    style={{ padding: '10px 14px' }}
                    onClick={() => toggleGroup(g.agentId)}
                  >
                    <div className="flex items-center gap-2 min-w-0">
                      <ChevronDown
                        size={13}
                        style={{
                          color: 'var(--hub-ink-3)',
                          transform: isCollapsed ? 'rotate(-90deg)' : 'rotate(0deg)',
                          transition: 'transform 0.15s',
                          flexShrink: 0,
                        }}
                      />
                      <span
                        className="font-medium text-[13.5px] truncate"
                        style={{ color: 'var(--hub-ink)', maxWidth: 320 }}
                        title={g.agentName}
                      >
                        {g.agentName}
                      </span>
                      {g.skills.length > 0 ? (
                        // Prominent accent badge so agents with data stand out
                        // among the (many) empty collapsed agents.
                        <span
                          className="hub-tag accent"
                          style={{
                            fontSize: 10,
                            fontWeight: 600,
                            display: 'inline-flex',
                            alignItems: 'center',
                            gap: 3,
                          }}
                          title={`${selectableCount} selectable / ${g.skills.length} total`}
                        >
                          {selectableCount}/{g.skills.length}
                        </span>
                      ) : (
                        <span className="hub-mono" style={{ fontSize: 11, color: 'var(--hub-ink-3)' }}>
                          0
                        </span>
                      )}
                      <span
                        className="hub-mono truncate flex-1 min-w-0"
                        style={{ fontSize: 11, color: 'var(--hub-ink-3)' }}
                        title={g.agentPath}
                      >
                        {g.agentPath}
                      </span>
                    </div>
                    <button
                      onClick={(e) => handleOpenPath(g.agentPath, e)}
                      className="hub-icon-btn sm"
                      title={t('skills.openFolder')}
                      style={{ color: 'var(--hub-ink-3)' }}
                    >
                      <Folder size={13} />
                    </button>
                  </div>
                  {!isCollapsed && (
                      <div className="border-t border-[var(--hub-line-2)]">
                        {g.skills.length === 0 ? (
                          <div
                            className="text-[12px]"
                            style={{ padding: '9px 14px 9px 34px', color: 'var(--hub-ink-3)' }}
                          >
                            {t('skills.noSkillsInAgent')}
                          </div>
                        ) : (
                        g.skills.map((s) => {
                          const key = `${s.agentId}::${s.dirName}`;
                          const alreadyImported = s.alreadyImported;
                          const disabled = alreadyImported || s.isSymlink;
                          const checked = selected.has(key);
                          return (
                            <label
                              key={key}
                              className="flex items-center gap-2.5 cursor-pointer transition-colors hover:bg-[var(--hub-surface-hover)]"
                              style={{
                                padding: '9px 14px 9px 34px',
                                opacity: disabled ? 0.5 : 1,
                                cursor: disabled ? 'not-allowed' : 'pointer',
                              }}
                            >
                              <input
                                type="checkbox"
                                checked={checked}
                                disabled={disabled}
                                onChange={() => toggleSelect(key)}
                                className="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
                              />
                              <div className="min-w-0 flex-1">
                                <div className="flex items-center gap-2 flex-wrap min-w-0">
                                  <span className="font-medium text-[13px] truncate" style={{ color: 'var(--hub-ink)', maxWidth: 200 }} title={s.name}>
                                    {s.name}
                                  </span>
                                  <span className="hub-mono truncate" style={{ fontSize: 11, color: 'var(--hub-ink-3)', maxWidth: 150 }} title={s.dirName}>
                                    {s.dirName}
                                  </span>
                                  {s.isSymlink && (
                                    <span
                                      className="hub-tag"
                                      style={{
                                        fontSize: 10,
                                        display: 'inline-flex',
                                        alignItems: 'center',
                                        gap: 3,
                                        background: 'var(--hub-bg-2)',
                                        color: 'var(--hub-ink-3)',
                                      }}
                                      title={t('skills.symlinkSkillHint')}
                                    >
                                      <Link2 size={10} />
                                      {t('skills.symlinkSkill')}
                                    </span>
                                  )}
                                  {alreadyImported && (
                                    <span
                                      className="hub-tag"
                                      style={{ fontSize: 10, background: 'var(--hub-bg-2)', color: 'var(--hub-ink-3)' }}
                                    >
                                      {t('skills.alreadyImported')}
                                    </span>
                                  )}
                                </div>
                                {s.description && (
                                  <div className="truncate mt-0.5" style={{ fontSize: 12, color: 'var(--hub-ink-3)' }}>
                                    {s.description}
                                  </div>
                                )}
                              </div>
                            </label>
                          );
                        })
                        )}
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>

        <div className="flex items-center justify-between gap-2 p-5 border-t border-[var(--hub-line-2)]">
          <div className="flex items-center gap-3 min-w-0">
            <button
              onClick={handleSelectFolder}
              disabled={importing}
              className="hub-btn flex items-center gap-1.5"
              title={t('skills.manualSelectSkills')}
            >
              <FolderPlus size={13} />
              {t('skills.manualSelectSkills')}
            </button>
            <span className="text-[12px] hub-mono truncate" style={{ color: 'var(--hub-ink-3)' }}>
              {t('skills.importSelected', { count: selected.size })}
            </span>
          </div>
          <div className="flex gap-2 flex-shrink-0">
            <button onClick={onClose} className="hub-btn" disabled={importing}>
              {t('common.cancel')}
            </button>
            <button
              onClick={handleSubmit}
              disabled={importing || selected.size === 0}
              className="hub-btn primary"
            >
              {importing ? t('common.saving') : t('skills.import')}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};

// ───────────────────────────────────────────────────────────────────────────
// View dialog: show a skill's detail and the agents it has been exported to.
// ───────────────────────────────────────────────────────────────────────────
interface ViewDialogProps {
  skillId: string;
  onUninstall: (
    skillId: string,
    agentId: string,
  ) => Promise<{ success: boolean; message?: string }>;
  onClose: () => void;
}

const ViewDialog: React.FC<ViewDialogProps> = ({ skillId, onUninstall, onClose }) => {
  const { t } = useTranslation();
  const { showToast } = useToast();
  const [skill, setSkill] = useState<Skill | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [uninstallTarget, setUninstallTarget] = useState<{ agentId: string; agentName: string } | null>(null);

  const refetch = async () => {
    try {
      const data = await getSkill(skillId);
      setSkill(data);
    } catch {
      // keep existing
    }
  };

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        setLoading(true);
        const data = await getSkill(skillId);
        if (cancelled) return;
        setSkill(data);
      } catch (err) {
        if (cancelled) return;
        setError(err instanceof Error ? err.message : t('skills.fetchError'));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [skillId, t]);

  const handleUninstallConfirm = async () => {
    if (!uninstallTarget) return;
    const result = await onUninstall(skillId, uninstallTarget.agentId);
    if (result.success) {
      showToast(t('skills.uninstallSuccess'), 'success');
      setUninstallTarget(null);
      await refetch();
    } else {
      setError(result.message || t('skills.uninstallError'));
      setUninstallTarget(null);
    }
  };

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-3xl w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[90vh] flex flex-col">
        <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
          <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">
            {t('skills.viewSkill')}
          </h2>
          <button onClick={onClose} className="hub-icon-btn sm">
            <X size={16} />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto p-5">
          {loading ? (
            <div className="flex items-center justify-center gap-2 py-10 text-[var(--hub-ink-3)]">
              <Loader2 size={16} className="animate-spin" />
            </div>
          ) : error ? (
            <div className="bg-red-50 border-l-4 border-red-500 text-red-700 p-4 rounded-md text-sm">{error}</div>
          ) : skill ? (
            <div className="space-y-4">
              <div>
                <div className="flex items-center gap-2 flex-wrap min-w-0">
                  <span className="font-medium text-[15px] truncate" style={{ color: 'var(--hub-ink)', maxWidth: 250 }} title={skill.name}>
                    {skill.name}
                  </span>
                  <span className="hub-mono truncate" style={{ fontSize: 12, color: 'var(--hub-ink-3)', maxWidth: 200 }} title={skill.dirName}>
                    {skill.dirName}
                  </span>
                </div>
                {skill.description && (
                  <p className="mt-1 text-[13px]" style={{ color: 'var(--hub-ink-2)' }}>
                    {skill.description}
                  </p>
                )}
              </div>

              <div>
                {skill.exports.length === 0 ? (
                  <div className="text-[13px]" style={{ color: 'var(--hub-ink-3)' }}>
                    {t('skills.emptyExports')}
                  </div>
                ) : (
                  <div>
                    <div className="hub-sect" style={{ marginBottom: 6 }}>
                      {t('skills.installedAgents')}
                    </div>
                    <div className="space-y-1.5">
                      {skill.exports.map((ex) => (
                        <div
                          key={ex.agentId}
                          className="hub-card flex items-center justify-between"
                          style={{ padding: '8px 12px', background: 'var(--hub-surface)' }}
                        >
                          <div className="flex items-center gap-2 min-w-0">
                            <span className="text-[13px] truncate" style={{ color: 'var(--hub-ink)', maxWidth: 400 }} title={ex.agentName}>
                              {ex.agentName}
                            </span>
                            <span
                              className="hub-tag"
                              style={{
                                fontSize: 10,
                                display: 'inline-flex',
                              alignItems: 'center',
                              gap: 4,
                              background: 'var(--hub-bg-2)',
                              color: 'var(--hub-ink-2)',
                            }}
                          >
                            {ex.method === 'symlink' ? <Link2 size={10} /> : <CopyIcon size={10} />}
                            {ex.method === 'symlink' ? t('skills.symlink') : t('skills.fileCopy')}
                          </span>
                        </div>
                        <div className="flex items-center gap-2">
                          {ex.createdAt && (
                            <span className="hub-mono" style={{ fontSize: 11, color: 'var(--hub-ink-3)' }}>
                              {ex.createdAt}
                            </span>
                          )}
                          <button
                            onClick={() => setUninstallTarget({ agentId: ex.agentId, agentName: ex.agentName })}
                            className="hub-icon-btn sm"
                            title={t('skills.uninstall')}
                            style={{ color: 'var(--hub-err)' }}
                          >
                            <Trash2 size={13} />
                          </button>
                        </div>
                      </div>
                    ))}
                    </div>
                  </div>
                )}
              </div>
            </div>
          ) : null}
        </div>

        <div className="flex justify-end p-5 border-t border-[var(--hub-line-2)]">
          <button onClick={onClose} className="hub-btn">
            {t('common.close')}
          </button>
        </div>

        <ConfirmDialog
          isOpen={!!uninstallTarget}
          onClose={() => setUninstallTarget(null)}
          onConfirm={handleUninstallConfirm}
          title={t('skills.uninstall')}
          message={t('skills.uninstallConfirm', { agent: uninstallTarget?.agentName ?? '' })}
          variant="danger"
        />
      </div>
    </div>
  );
};

// ───────────────────────────────────────────────────────────────────────────
// Export dialog: pick target agents (searchable multiselect) + method
// (symlink / copy) with a ? help popover. Exports selected skills.
// ───────────────────────────────────────────────────────────────────────────
interface ExportDialogProps {
  selectedCount: number;
  onExport: (
    agentIds: string[],
    method: 'symlink' | 'copy',
  ) => Promise<{ success: boolean; message?: string; data?: ExportResultItem[] }>;
  onClose: () => void;
}

const ExportDialog: React.FC<ExportDialogProps> = ({ selectedCount, onExport, onClose }) => {
  const { t } = useTranslation();
  const [agents, setAgents] = useState<SkillAgent[]>([]);
  const [loading, setLoading] = useState(true);
  const [agentSearch, setAgentSearch] = useState('');
  const [selectedAgents, setSelectedAgents] = useState<Set<string>>(new Set());
  const [method, setMethod] = useState<'symlink' | 'copy'>('symlink');
  const [exporting, setExporting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const data = await listSkillAgents();
        if (cancelled) return;
        setAgents(data);
      } catch {
        // ignore — leave empty
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const filteredAgents = useMemo(() => {
    const q = agentSearch.trim().toLowerCase();
    if (!q) return agents;
    return agents.filter((a) => a.name.toLowerCase().includes(q) || a.id.toLowerCase().includes(q));
  }, [agents, agentSearch]);

  const toggleAgent = (id: string) => {
    setSelectedAgents((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const handleSubmit = async () => {
    setExporting(true);
    const result = await onExport(Array.from(selectedAgents), method);
    setExporting(false);
    if (result.success) {
      onClose();
    } else {
      setError(result.message || t('skills.exportError'));
    }
  };

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-3xl w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[90vh] flex flex-col">
        <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
          <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">
            {t('skills.exportDialogTitle')}
          </h2>
          <button onClick={onClose} className="hub-icon-btn sm">
            <X size={16} />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto p-5 space-y-5">
          {error && (
            <div className="bg-red-50 border-l-4 border-red-500 text-red-700 p-4 rounded-md text-sm">{error}</div>
          )}

          {/* Searchable agent multiselect */}
          <div>
            <div className="flex items-center gap-1.5 mb-2">
              <label className="block text-sm font-medium text-gray-700 dark:text-gray-300">
                {t('skills.exportTargetAgents')}
              </label>
              <MethodHelpIcon />
            </div>
            <div
              className="hub-card flex items-center gap-2 px-2.5 mb-2"
              style={{ height: 30, background: 'var(--hub-surface)' }}
            >
              <Search size={13} style={{ color: 'var(--hub-ink-3)' }} />
              <input
                value={agentSearch}
                onChange={(e) => setAgentSearch(e.target.value)}
                placeholder={t('skills.searchAgents')}
                className="flex-1 bg-transparent outline-none text-[13px]"
                style={{ color: 'var(--hub-ink)' }}
              />
              {agentSearch && (
                <button onClick={() => setAgentSearch('')} className="hub-icon-btn sm">
                  <X size={11} />
                </button>
              )}
            </div>
            <div className="hub-card max-h-56 overflow-y-auto" style={{ background: 'var(--hub-surface)' }}>
              {loading ? (
                <div className="flex items-center justify-center py-6 text-[var(--hub-ink-3)]">
                  <Loader2 size={15} className="animate-spin" />
                </div>
              ) : filteredAgents.length === 0 ? (
                <div className="py-6 text-center text-[13px]" style={{ color: 'var(--hub-ink-3)' }}>
                  {t('skills.noAgents')}
                </div>
              ) : (
                filteredAgents.map((a) => {
                  const checked = selectedAgents.has(a.id);
                  return (
                    <label
                      key={a.id}
                      className="flex items-center gap-2.5 cursor-pointer transition-colors hover:bg-[var(--hub-surface-hover)]"
                      style={{ padding: '8px 12px' }}
                    >
                      <input
                        type="checkbox"
                        checked={checked}
                        onChange={() => toggleAgent(a.id)}
                        className="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
                      />
                      <div className="min-w-0 flex-1">
                        <div className="text-[13px] truncate" style={{ color: 'var(--hub-ink)' }} title={a.name}>
                          {a.name}
                        </div>
                        <div className="hub-mono truncate" style={{ fontSize: 11, color: 'var(--hub-ink-3)' }} title={a.skillsPath}>
                          {a.skillsPath}
                        </div>
                      </div>
                    </label>
                  );
                })
              )}
            </div>
          </div>

          {/* Method radio with ? help */}
          <div>
            <label className="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">
              {t('skills.exportMethod')}
            </label>
            <div className="flex gap-4">
              <label className="flex items-center gap-2 text-[13px]" style={{ color: 'var(--hub-ink)' }}>
                <input
                  type="radio"
                  name="export-method"
                  checked={method === 'symlink'}
                  onChange={() => setMethod('symlink')}
                  className="h-4 w-4 text-blue-600 border-gray-300 focus:ring-blue-500"
                />
                <Link2 size={13} />
                {t('skills.symlink')}
              </label>
              <label className="flex items-center gap-2 text-[13px]" style={{ color: 'var(--hub-ink)' }}>
                <input
                  type="radio"
                  name="export-method"
                  checked={method === 'copy'}
                  onChange={() => setMethod('copy')}
                  className="h-4 w-4 text-blue-600 border-gray-300 focus:ring-blue-500"
                />
                <CopyIcon size={13} />
                {t('skills.fileCopy')}
              </label>
            </div>
          </div>
        </div>

        <div className="flex items-center justify-between gap-2 p-5 border-t border-[var(--hub-line-2)]">
          <span className="text-[12px] hub-mono" style={{ color: 'var(--hub-ink-3)' }}>
            {t('skills.exportSummary', { skills: selectedCount, agents: selectedAgents.size })}
          </span>
          <div className="flex gap-2">
            <button onClick={onClose} className="hub-btn" disabled={exporting}>
              {t('common.cancel')}
            </button>
            <button
              onClick={handleSubmit}
              disabled={exporting || selectedAgents.size === 0}
              className="hub-btn primary"
            >
              {exporting ? t('common.saving') : t('skills.export')}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
};

// ───────────────────────────────────────────────────────────────────────────
// Install dialog (per-row): install a single skill to multiple agents.
// Shows each agent's current install method (if already installed) and lets
// the user switch methods — the backend (Phase 2) deletes the previous
// symlink/file and reinstalls with the chosen method.
// ───────────────────────────────────────────────────────────────────────────
interface InstallDialogProps {
  skill: Skill;
  onInstall: (
    skillId: string,
    agentIds: string[],
    method: 'symlink' | 'copy',
  ) => Promise<{ success: boolean; message?: string }>;
  onUninstall: (
    skillId: string,
    agentId: string,
  ) => Promise<{ success: boolean; message?: string }>;
  onClose: () => void;
}

const InstallDialog: React.FC<InstallDialogProps> = ({ skill, onInstall, onUninstall, onClose }) => {
  const { t } = useTranslation();
  const { showToast } = useToast();
  const [agents, setAgents] = useState<SkillAgent[]>([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState('');
  const [selected, setSelected] = useState<Set<string>>(new Set());
  // Per-agent chosen install method. Initialized from each agent's current
  // method (if already installed) once the agent list loads; defaults to
  // 'symlink' for agents that don't have the skill yet.
  const [agentMethod, setAgentMethod] = useState<Record<string, 'symlink' | 'copy'>>({});
  const [installing, setInstalling] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // current method per agent (from the skill's exports)
  const currentMethodByAgent = useMemo(() => {
    const m = new Map<string, 'symlink' | 'copy'>();
    for (const ex of skill.exports) m.set(ex.agentId, ex.method);
    return m;
  }, [skill]);

  // Live per-agent installed method. Initialized from the skill's exports
  // and mutated on uninstall so the UI updates without refetching.
  const [installedMethods, setInstalledMethods] = useState<Map<string, 'symlink' | 'copy'>>(
    () => new Map(currentMethodByAgent),
  );
  // Agent pending uninstall confirmation
  const [uninstallTarget, setUninstallTarget] = useState<{ agentId: string; agentName: string } | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const data = await listSkillAgents();
        if (cancelled) return;
        setAgents(data);
        // Seed each agent's chosen method: keep current method if installed,
        // else default to symlink.
        const init: Record<string, 'symlink' | 'copy'> = {};
        for (const a of data) {
          init[a.id] = currentMethodByAgent.get(a.id) ?? 'symlink';
        }
        setAgentMethod(init);
      } catch {
        // ignore
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [currentMethodByAgent]);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return agents;
    return agents.filter((a) => a.name.toLowerCase().includes(q) || a.id.toLowerCase().includes(q));
  }, [agents, search]);

  // Split into already-installed vs not-yet-installed so installed agents are
  // displayed distinctly (with their current method), per requirement.
  const installedList = useMemo(
    () => filtered.filter((a) => installedMethods.has(a.id)),
    [filtered, installedMethods],
  );
  const availableList = useMemo(
    () => filtered.filter((a) => !installedMethods.has(a.id)),
    [filtered, installedMethods],
  );

  const renderRow = (a: SkillAgent) => {
    const checked = selected.has(a.id);
    const current = installedMethods.get(a.id);
    const chosen = agentMethod[a.id] ?? 'symlink';
    const switching = current && current !== chosen;
    return (
      <div
        key={a.id}
        className="flex items-center gap-2.5 transition-colors hover:bg-[var(--hub-surface-hover)]"
        style={{ padding: '8px 12px' }}
      >
        <label className="flex items-center gap-2.5 cursor-pointer min-w-0 flex-1">
          <input
            type="checkbox"
            checked={checked}
            onChange={() => toggle(a.id)}
            className="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500 flex-shrink-0"
          />
          <div className="min-w-0 flex-1">
            <div className="flex items-center gap-2 flex-wrap">
              <span className="text-[13px] truncate" style={{ color: 'var(--hub-ink)', maxWidth: 320 }} title={a.name}>
                {a.name}
              </span>
              {current && (
                <span
                  className="hub-tag"
                  style={{
                    fontSize: 10,
                    display: 'inline-flex',
                    alignItems: 'center',
                    gap: 4,
                    background: 'var(--hub-bg-2)',
                    color: 'var(--hub-ink-3)',
                  }}
                >
                  {current === 'symlink' ? <Link2 size={10} /> : <CopyIcon size={10} />}
                  {t('skills.currentMethod')} {current === 'symlink' ? t('skills.symlink') : t('skills.fileCopy')}
                </span>
              )}
              {switching && (
                <span className="hub-tag accent" style={{ fontSize: 10 }}>
                  {t('skills.switchTo')} {chosen === 'symlink' ? t('skills.symlink') : t('skills.fileCopy')}
                </span>
              )}
            </div>
            <div className="hub-mono truncate" style={{ fontSize: 11, color: 'var(--hub-ink-3)' }} title={a.skillsPath}>
              {a.skillsPath}
            </div>
          </div>
        </label>
        {/* Per-agent method toggle */}
        <div
          className="flex items-center flex-shrink-0 rounded-md"
          style={{ border: '1px solid var(--hub-line)', background: 'var(--hub-bg-2)' }}
        >
          <button
            type="button"
            onClick={() => setMethodFor(a.id, 'symlink')}
            title={t('skills.symlink')}
            className="flex items-center gap-1 px-2 py-1 text-[11px] transition-colors"
            style={{
              borderRadius: 5,
              background: chosen === 'symlink' ? 'var(--hub-surface)' : 'transparent',
              color: chosen === 'symlink' ? 'var(--hub-ink)' : 'var(--hub-ink-3)',
              border: 'none',
              cursor: 'pointer',
            }}
          >
            <Link2 size={11} />
            {t('skills.symlink')}
          </button>
          <button
            type="button"
            onClick={() => setMethodFor(a.id, 'copy')}
            title={t('skills.fileCopy')}
            className="flex items-center gap-1 px-2 py-1 text-[11px] transition-colors"
            style={{
              borderRadius: 5,
              background: chosen === 'copy' ? 'var(--hub-surface)' : 'transparent',
              color: chosen === 'copy' ? 'var(--hub-ink)' : 'var(--hub-ink-3)',
              border: 'none',
              cursor: 'pointer',
            }}
          >
            <CopyIcon size={11} />
            {t('skills.fileCopy')}
          </button>
        </div>
        {current && (
          <button
            type="button"
            onClick={() => setUninstallTarget({ agentId: a.id, agentName: a.name })}
            className="hub-icon-btn sm flex-shrink-0"
            title={t('skills.uninstall')}
            style={{ color: 'var(--hub-err)' }}
          >
            <Trash2 size={13} />
          </button>
        )}
      </div>
    );
  };

  const toggle = (id: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const setMethodFor = (id: string, method: 'symlink' | 'copy') => {
    setAgentMethod((prev) => ({ ...prev, [id]: method }));
    // Switching the method implies intent to install/switch — auto-select the
    // agent so the user doesn't have to check it separately.
    setSelected((prev) => {
      if (prev.has(id)) return prev;
      const next = new Set(prev);
      next.add(id);
      return next;
    });
  };

  const handleUninstallConfirm = async () => {
    if (!uninstallTarget) return;
    const result = await onUninstall(skill.id, uninstallTarget.agentId);
    if (result.success) {
      // Optimistically remove the install from local state.
      setInstalledMethods((prev) => {
        const next = new Map(prev);
        next.delete(uninstallTarget.agentId);
        return next;
      });
      setSelected((prev) => {
        const next = new Set(prev);
        next.delete(uninstallTarget.agentId);
        return next;
      });
      showToast(t('skills.uninstallSuccess'), 'success');
      setUninstallTarget(null);
    } else {
      setError(result.message || t('skills.uninstallError'));
      setUninstallTarget(null);
    }
  };

  const handleSubmit = async () => {
    setInstalling(true);
    // Group selected agents by their chosen method and install per group —
    // the backend export command takes one method per call, so per-agent
    // switching is achieved by splitting into one call per method.
    const byMethod: Record<'symlink' | 'copy', string[]> = { symlink: [], copy: [] };
    for (const id of selected) {
      const m = agentMethod[id] ?? 'symlink';
      byMethod[m].push(id);
    }
    let allOk = true;
    let lastError: string | undefined;
    for (const m of ['symlink', 'copy'] as const) {
      if (byMethod[m].length === 0) continue;
      const result = await onInstall(skill.id, byMethod[m], m);
      if (!result.success) {
        allOk = false;
        lastError = result.message;
      }
    }
    setInstalling(false);
    if (allOk) {
      showToast(t('skills.installSuccess'), 'success');
      onClose();
    } else {
      setError(lastError || t('skills.installError'));
    }
  };

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-3xl w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[90vh] flex flex-col">
        <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
          <div className="min-w-0">
            <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">
              {t('skills.installDialogTitle')}
            </h2>
            <div className="flex items-center gap-2 mt-1 min-w-0">
              <span className="font-medium text-[13px] truncate" style={{ color: 'var(--hub-ink)', maxWidth: 200 }} title={skill.name}>
                {skill.name}
              </span>
              <span className="hub-mono truncate" style={{ fontSize: 11, color: 'var(--hub-ink-3)', maxWidth: 150 }} title={skill.dirName}>
                {skill.dirName}
              </span>
            </div>
          </div>
          <button onClick={onClose} className="hub-icon-btn sm">
            <X size={16} />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto p-5 space-y-5">
          {error && (
            <div className="bg-red-50 border-l-4 border-red-500 text-red-700 p-4 rounded-md text-sm">{error}</div>
          )}

          <div>
            <div className="flex items-center gap-1.5 mb-2">
              <label className="block text-sm font-medium text-gray-700 dark:text-gray-300">
                {t('skills.installTargetAgents')}
              </label>
              <MethodHelpIcon />
            </div>
            <div
              className="hub-card flex items-center gap-2 px-2.5 mb-2"
              style={{ height: 30, background: 'var(--hub-surface)' }}
            >
              <Search size={13} style={{ color: 'var(--hub-ink-3)' }} />
              <input
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder={t('skills.searchAgents')}
                className="flex-1 bg-transparent outline-none text-[13px]"
                style={{ color: 'var(--hub-ink)' }}
              />
              {search && (
                <button onClick={() => setSearch('')} className="hub-icon-btn sm">
                  <X size={11} />
                </button>
              )}
            </div>
            <div className="hub-card max-h-56 overflow-y-auto" style={{ background: 'var(--hub-surface)' }}>
              {loading ? (
                <div className="flex items-center justify-center py-6 text-[var(--hub-ink-3)]">
                  <Loader2 size={15} className="animate-spin" />
                </div>
              ) : filtered.length === 0 ? (
                <div className="py-6 text-center text-[13px]" style={{ color: 'var(--hub-ink-3)' }}>
                  {t('skills.noAgents')}
                </div>
              ) : (
                <>
                  {installedList.length > 0 && (
                    <div
                      className="hub-sect"
                      style={{
                        padding: '8px 12px 4px',
                        fontSize: 11,
                        color: 'var(--hub-ink-3)',
                        borderBottom: '1px solid var(--hub-line-2)',
                      }}
                    >
                      {t('skills.installedSection')} ({installedList.length})
                    </div>
                  )}
                  {installedList.map(renderRow)}
                  {availableList.length > 0 && (
                    <div
                      className="hub-sect"
                      style={{
                        padding: '8px 12px 4px',
                        fontSize: 11,
                        color: 'var(--hub-ink-3)',
                        borderBottom: '1px solid var(--hub-line-2)',
                      }}
                    >
                      {t('skills.notInstalledSection')} ({availableList.length})
                    </div>
                  )}
                  {availableList.map(renderRow)}
                </>
              )}
            </div>
          </div>
        </div>

        <div className="flex items-center justify-between gap-2 p-5 border-t border-[var(--hub-line-2)]">
          <span className="text-[12px] hub-mono" style={{ color: 'var(--hub-ink-3)' }}>
            {t('skills.exportSummary', { skills: 1, agents: selected.size })}
          </span>
          <div className="flex gap-2">
            <button onClick={onClose} className="hub-btn" disabled={installing}>
              {t('common.cancel')}
            </button>
            <button
              onClick={handleSubmit}
              disabled={installing || selected.size === 0}
              className="hub-btn primary"
            >
              {installing ? t('common.saving') : t('skills.install')}
            </button>
          </div>
        </div>

        <ConfirmDialog
          isOpen={!!uninstallTarget}
          onClose={() => setUninstallTarget(null)}
          onConfirm={handleUninstallConfirm}
          title={t('skills.uninstall')}
          message={t('skills.uninstallConfirm', { agent: uninstallTarget?.agentName ?? '' })}
          variant="danger"
        />
      </div>
    </div>
  );
};

// ───────────────────────────────────────────────────────────────────────────
// Delete dialog: removes the library copy. Symlink exports are removed
// mandatorily (dangling symlinks would point at the deleted library copy);
// file-copy exports are optional — the user chooses which copies to delete.
// ───────────────────────────────────────────────────────────────────────────
interface DeleteSkillDialogProps {
  skill: Skill;
  onDelete: (id: string, cleanupAgentIds: string[]) => Promise<{ success: boolean; message?: string }>;
  onClose: () => void;
}

const DeleteSkillDialog: React.FC<DeleteSkillDialogProps> = ({ skill, onDelete, onClose }) => {
  const { t } = useTranslation();
  const [deleting, setDeleting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // optional copy-export agent ids selected for cleanup
  const [copyCleanup, setCopyCleanup] = useState<Set<string>>(new Set());

  const symlinkExports = skill.exports.filter((e) => e.method === 'symlink');
  // Copy exports offered for optional file cleanup (including the source-agent
  // record — it's deletable like any other install).
  const copyExports = skill.exports.filter((e) => e.method === 'copy');

  const toggleCopy = (agentId: string) => {
    setCopyCleanup((prev) => {
      const next = new Set(prev);
      if (next.has(agentId)) next.delete(agentId);
      else next.add(agentId);
      return next;
    });
  };

  const handleConfirm = async () => {
    setDeleting(true);
    // Symlink agents are always cleaned up (mandatory); copy agents are optional.
    const cleanupAgentIds = [
      ...symlinkExports.map((e) => e.agentId),
      ...copyExports.filter((e) => copyCleanup.has(e.agentId)).map((e) => e.agentId),
    ];
    const result = await onDelete(skill.id, cleanupAgentIds);
    setDeleting(false);
    if (result.success) {
      onClose();
    } else {
      setError(result.message || t('skills.deleteError'));
    }
  };

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-2xl w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[90vh] flex flex-col">
        <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
          <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">
            {t('skills.deleteDialogTitle')}
          </h2>
          <button onClick={onClose} className="hub-icon-btn sm">
            <X size={16} />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto p-5 space-y-4">
          {error && (
            <div className="bg-red-50 border-l-4 border-red-500 text-red-700 p-4 rounded-md text-sm">{error}</div>
          )}

          <div className="flex items-start gap-2.5 p-3 rounded-md" style={{ background: 'var(--hub-bg-2)' }}>
            <AlertTriangle size={16} className="flex-shrink-0 mt-0.5" style={{ color: 'var(--hub-warn, #d97706)' }} />
            <div className="text-[13px]" style={{ color: 'var(--hub-ink-2)' }}>
              {t('skills.deleteLibraryCopy', { name: skill.dirName })}
            </div>
          </div>

          {skill.exports.length === 0 ? (
            <div className="text-[13px]" style={{ color: 'var(--hub-ink-3)' }}>
              {t('skills.deleteNoExports')}
            </div>
          ) : (
            <>
              {symlinkExports.length > 0 && (
                <div>
                  <div className="hub-sect" style={{ marginBottom: 6 }}>
                    {t('skills.deleteSymlinkMandatory')}
                  </div>
                  <div className="space-y-1.5">
                    {symlinkExports.map((ex) => (
                      <div
                        key={ex.agentId}
                        className="hub-card flex items-center gap-2"
                        style={{ padding: '8px 12px', background: 'var(--hub-surface)' }}
                      >
                        <Link2 size={12} style={{ color: 'var(--hub-ink-3)' }} />
                        <span className="text-[13px] truncate" style={{ color: 'var(--hub-ink)', maxWidth: 400 }} title={ex.agentName}>
                          {ex.agentName}
                        </span>
                        <span
                          className="hub-tag ml-auto"
                          style={{ fontSize: 10, background: 'var(--hub-bg-2)', color: 'var(--hub-ink-3)' }}
                        >
                          {t('skills.symlink')}
                        </span>
                      </div>
                    ))}
                  </div>
                </div>
              )}

              {copyExports.length > 0 && (
                <div>
                  <div className="hub-sect" style={{ marginBottom: 6 }}>
                    {t('skills.deleteCopyOptional')}
                  </div>
                  <div className="space-y-1.5">
                    {copyExports.map((ex) => {
                      const checked = copyCleanup.has(ex.agentId);
                      return (
                        <label
                          key={ex.agentId}
                          className="hub-card flex items-center gap-2 cursor-pointer"
                          style={{ padding: '8px 12px', background: 'var(--hub-surface)' }}
                        >
                          <input
                            type="checkbox"
                            checked={checked}
                            onChange={() => toggleCopy(ex.agentId)}
                            className="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500"
                          />
                          <CopyIcon size={12} style={{ color: 'var(--hub-ink-3)' }} />
                          <span className="text-[13px] truncate" style={{ color: 'var(--hub-ink)', maxWidth: 400 }} title={ex.agentName}>
                            {ex.agentName}
                          </span>
                          <span
                            className="hub-tag ml-auto"
                            style={{ fontSize: 10, background: 'var(--hub-bg-2)', color: 'var(--hub-ink-3)' }}
                          >
                            {t('skills.fileCopy')}
                          </span>
                        </label>
                      );
                    })}
                  </div>
                </div>
              )}
            </>
          )}
        </div>

        <div className="flex justify-end gap-2 p-5 border-t border-[var(--hub-line-2)]">
          <button onClick={onClose} className="hub-btn" disabled={deleting}>
            {t('common.cancel')}
          </button>
          <button
            onClick={handleConfirm}
            disabled={deleting}
            className="hub-btn"
            style={{ color: 'var(--hub-err)', borderColor: 'var(--hub-err)' }}
          >
            {deleting ? t('common.saving') : t('skills.confirmDeleteBtn')}
          </button>
        </div>
      </div>
    </div>
  );
};

// ───────────────────────────────────────────────────────────────────────────
// Agent management dialog: list all agents (built-in read-only, custom
// deletable) + create custom agent form.
// ───────────────────────────────────────────────────────────────────────────
interface AgentManagementDialogProps {
  onClose: () => void;
}

const AgentManagementDialog: React.FC<AgentManagementDialogProps> = ({ onClose }) => {
  const { t } = useTranslation();
  const { showToast } = useToast();
  const [agents, setAgents] = useState<SkillAgent[]>([]);
  const [loading, setLoading] = useState(true);
  const [name, setName] = useState('');
  const [path, setPath] = useState('');
  const [creating, setCreating] = useState(false);
  const [search, setSearch] = useState('');
  const [deleteId, setDeleteId] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const data = await listSkillAgents();
      setAgents(data);
    } catch {
      setAgents([]);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const handlePick = async () => {
    try {
      const p = await pickDirectory();
      if (p) setPath(p);
    } catch (err) {
      showToast(err instanceof Error ? err.message : t('skills.pickFolderError'), 'error');
    }
  };

  const handleCreate = async () => {
    const n = name.trim();
    const p = path.trim();
    if (!n) {
      showToast(t('skills.customAgentNameRequired'), 'error');
      return;
    }
    if (!p) {
      showToast(t('skills.customAgentPathRequired'), 'error');
      return;
    }
    setCreating(true);
    try {
      await createSkillAgent(n, p);
      showToast(t('skills.customAgentAdded'), 'success');
      setName('');
      setPath('');
      await refresh();
    } catch (err) {
      showToast(err instanceof Error ? err.message : t('skills.customAgentAddError'), 'error');
    } finally {
      setCreating(false);
    }
  };

  const handleDeleteConfirm = async () => {
    if (!deleteId) return;
    try {
      await deleteSkillAgent(deleteId);
      showToast(t('skills.uninstallSuccess'), 'success');
      await refresh();
    } catch (err) {
      showToast(err instanceof Error ? err.message : 'Failed to delete agent', 'error');
    } finally {
      setDeleteId(null);
    }
  };

  const sorted = useMemo(
    () => [...agents].sort((a, b) => a.name.localeCompare(b.name)),
    [agents],
  );
  const customCount = agents.filter((a) => a.custom).length;
  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return sorted;
    return sorted.filter((a) => a.name.toLowerCase().includes(q));
  }, [sorted, search]);

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-[40rem] w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[90vh] flex flex-col">
        <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
          <div className="min-w-0">
            <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">
              {t('skills.agentManagement')}
            </h2>
            <div className="text-[12px] hub-mono mt-1" style={{ color: 'var(--hub-ink-3)' }}>
              {agents.length} ({customCount} custom)
            </div>
          </div>
          <button onClick={onClose} className="hub-icon-btn sm">
            <X size={16} />
          </button>
        </div>

        {/* Fixed toolbar: create custom agent + search (does not scroll) */}
        <div className="p-5 border-b border-[var(--hub-line-2)] space-y-3">
          {/* Create custom agent */}
          <div>
            <div className="flex items-center gap-1.5 mb-2">
              <FolderPlus size={13} style={{ color: 'var(--hub-ink-3)' }} />
              <label className="block text-sm font-medium text-gray-700 dark:text-gray-300">
                {t('skills.addCustomAgent')}
              </label>
            </div>
            <div className="flex items-center gap-2 flex-wrap">
              <input
                type="text"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder={t('skills.customAgentNamePlaceholder')}
                className="block py-2 px-3 border border-gray-300 rounded-md shadow-sm focus:outline-none focus:ring-blue-500 focus:border-blue-500 sm:text-sm form-input"
                style={{ width: 180 }}
              />
              <div className="flex items-center flex-1 min-w-[200px]">
                <input
                  type="text"
                  value={path}
                  onChange={(e) => setPath(e.target.value)}
                  placeholder={t('skills.customAgentPathPlaceholder')}
                  className="flex-1 block py-2 px-3 border border-gray-300 rounded-md shadow-sm focus:outline-none focus:ring-blue-500 focus:border-blue-500 sm:text-sm form-input font-mono rounded-r-none"
                />
                <button
                  type="button"
                  onClick={handlePick}
                  className="hub-btn ghost flex items-center gap-1"
                  style={{ height: 38, borderRadius: '0 6px 6px 0', border: '1px solid var(--hub-line)', borderLeft: 'none' }}
                  title={t('skills.pickFolder')}
                >
                  <Folder size={14} />
                </button>
              </div>
              <button type="button" onClick={handleCreate} disabled={creating} className="hub-btn primary flex items-center gap-1">
                {creating ? <Loader2 size={13} className="animate-spin" /> : <Plus size={13} />}
                {t('skills.addCustomAgent')}
              </button>
            </div>
          </div>

          {/* Search by name (below the add form) */}
          <div className="hub-card flex items-center gap-2 px-2.5" style={{ height: 32, background: 'var(--hub-surface)' }}>
            <Search size={13} style={{ color: 'var(--hub-ink-3)' }} />
            <input
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              placeholder={t('skills.searchAgents') || t('common.searchPlaceholder') || 'Search…'}
              className="flex-1 bg-transparent outline-none text-[13px]"
              style={{ color: 'var(--hub-ink)' }}
            />
            {search && (
              <button onClick={() => setSearch('')} className="hub-icon-btn sm">
                <X size={11} />
              </button>
            )}
          </div>
        </div>

        {/* Scrollable agent list */}
        <div className="flex-1 overflow-y-auto p-5">
          {loading ? (
            <div className="flex items-center justify-center py-10 text-[var(--hub-ink-3)]">
              <Loader2 size={15} className="animate-spin" />
            </div>
          ) : filtered.length === 0 ? (
            <div className="hub-card p-8 text-center text-[13px]" style={{ color: 'var(--hub-ink-3)' }}>
              {search ? t('pages.rag.noResults') : t('skills.noAgents')}
            </div>
          ) : (
            <div className="hub-card overflow-hidden" style={{ background: 'var(--hub-surface)' }}>
              {filtered.map((a, idx) => (
                <div
                  key={a.id}
                  style={{
                    padding: '10px 14px',
                    borderTop: idx === 0 ? 0 : '1px solid var(--hub-line-2)',
                  }}
                >
                  <div className="flex items-center justify-between gap-3 w-full">
                    <div className="flex flex-col gap-0.5 min-w-0 flex-1">
                      <div className="flex items-center gap-2 min-w-0">
                        <span className="truncate text-[13px] font-medium" style={{ color: 'var(--hub-ink)' }} title={a.name}>
                          {a.name}
                        </span>
                        <span className="hub-tag whitespace-nowrap flex-shrink-0" style={{ fontSize: 10 }}>
                          {a.custom ? t('skills.customAgentTag') : t('skills.builtinAgentTag')}
                        </span>
                      </div>
                      <span className="hub-mono truncate" style={{ fontSize: 11, color: 'var(--hub-ink-3)' }} title={a.skillsPath}>
                        {a.skillsPath}
                      </span>
                    </div>
                    {a.custom && (
                      <button
                        className="hub-icon-btn sm flex-shrink-0"
                        onClick={() => setDeleteId(a.id)}
                        title={t('skills.uninstall')}
                        style={{ color: 'var(--hub-err)' }}
                      >
                        <Trash2 size={13} />
                      </button>
                    )}
                  </div>
                </div>
              ))}
            </div>
          )}
        </div>

        <div className="flex items-center justify-end gap-2 p-5 border-t border-[var(--hub-line-2)]">
          <button onClick={onClose} className="hub-btn">
            {t('skills.close')}
          </button>
        </div>
      </div>

      {deleteId && (
        <ConfirmDialog
          isOpen={!!deleteId}
          onClose={() => setDeleteId(null)}
          onConfirm={handleDeleteConfirm}
          title={t('skills.deleteAgent')}
          message={t('skills.deleteAgentConfirm')}
          variant="danger"
        />
      )}
    </div>
  );
};

// ───────────────────────────────────────────────────────────────────────────
// SkillsPage
// ───────────────────────────────────────────────────────────────────────────
const SkillsPage: React.FC = () => {
  const { t } = useTranslation();
  const { auth } = useAuth();
  const { showToast } = useToast();
  const { skills, loading, error, setError, importSkills, exportSkills, removeSkill, uninstallSkill } = useSkillData();

  const [search, setSearch] = useState('');
  const [page, setPage] = useState(1);
  const [pageSize, setPageSize] = useState(10);
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());

  const [showImport, setShowImport] = useState(false);
  const [showAgentMgmt, setShowAgentMgmt] = useState(false);
  const [viewSkillId, setViewSkillId] = useState<string | null>(null);
  const [showExport, setShowExport] = useState(false);
  const [installSkill, setInstallSkill] = useState<Skill | null>(null);
  const [skillToDelete, setSkillToDelete] = useState<Skill | null>(null);

  const isAdmin = auth.user?.isAdmin;

  // Sort by dir_name, then filter + paginate client-side (selectItemPage
  // filters/searches but does NOT sort — see utils/listFilters.ts).
  const sortedSkills = useMemo(
    () => [...skills].sort((a, b) => a.dirName.localeCompare(b.dirName)),
    [skills],
  );

  const { items: visibleSkills, pagination } = useMemo(
    () =>
      selectItemPage(sortedSkills, 'all', search, page, pageSize, {
        haystack: (s) => s.dirName + ' ' + s.name + ' ' + s.description,
        isEnabled: () => true,
      }),
    [sortedSkills, search, page, pageSize],
  );

  useEffect(() => {
    if (pagination.page !== page) {
      setPage(pagination.page);
    }
  }, [pagination.page, page]);

  // existingDirNames removed: "already imported" is now a filesystem check
  // (ScannedSkill.alreadyImported, set by the backend scanning the library dir),
  // not a DB-derived set.

  const toggleSelect = (id: string) => {
    setSelectedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const handleImport = async (items: Array<{ agentId: string; dirName: string }>) => {
    const result = await importSkills(items);
    if (result.success) {
      showToast(t('skills.importSuccess', { count: result.data?.successCount ?? 0 }), 'success');
    }
    return result;
  };

  const handleExport = async (agentIds: string[], method: 'symlink' | 'copy') => {
    const result = await exportSkills(Array.from(selectedIds), agentIds, method);
    if (result.success) {
      const ok = (result.data || []).filter((r) => r.success).length;
      showToast(t('skills.exportSuccess', { count: ok }), 'success');
      setSelectedIds(new Set());
    }
    return result;
  };

  const handleInstall = async (
    skillId: string,
    agentIds: string[],
    method: 'symlink' | 'copy',
  ) => {
    // Install = export this single skill to the chosen agents with the chosen
    // method. The backend (Phase 2) deletes any previous install for the same
    // (skill, agent) and reinstalls — so switching symlink↔copy works.
    const result = await exportSkills([skillId], agentIds, method);
    return result;
  };

  const handleDelete = async (id: string, cleanupAgentIds: string[]) => {
    const result = await removeSkill(id, cleanupAgentIds);
    if (result.success) {
      showToast(t('skills.deleteSuccess'), 'success');
      setSelectedIds((prev) => {
        const next = new Set(prev);
        next.delete(id);
        return next;
      });
    }
    return result;
  };

  const handleUninstall = async (skillId: string, agentId: string) => {
    const result = await uninstallSkill(skillId, agentId);
    if (result.success) {
      showToast(t('skills.uninstallSuccess'), 'success');
    }
    return result;
  };

  const handleOpenSkillLibrary = async (id: string) => {
    try {
      await openSkillLibrary(id);
    } catch (err) {
      showToast(err instanceof Error ? err.message : t('skills.openPathError'), 'error');
    }
  };

  return (
    <div>
      <div className="flex items-end justify-between gap-4 mb-6">
        <div>
          <h1 className="hub-h1">{t('pages.skills.title')}</h1>
          <p className="hub-sub">
            <span className="hub-num">{skills.length}</span> {t('nav.skills').toLowerCase()}
          </p>
        </div>
        {isAdmin && (
          <div className="flex items-center gap-2">
            <button onClick={() => setShowAgentMgmt(true)} className="hub-btn">
              <FolderPlus size={13} /> {t('skills.agentManagement')}
            </button>
            <button onClick={() => setShowImport(true)} className="hub-btn primary">
              <Plus size={13} /> {t('skills.importExisting')}
            </button>
          </div>
        )}
      </div>

      {error && (
        <div
          className="hub-card flex items-center justify-between gap-3 mb-4"
          style={{
            padding: '10px 14px',
            borderColor: 'oklch(0.85 0.1 25)',
            background: 'oklch(0.97 0.03 25)',
            color: 'oklch(0.4 0.18 25)',
          }}
        >
          <div className="flex items-center gap-2 min-w-0">
            <X size={14} className="flex-shrink-0" />
            <span className="truncate text-[13px]">{error}</span>
          </div>
          <button className="hub-icon-btn sm" onClick={() => setError(null)}>
            <X size={13} />
          </button>
        </div>
      )}

      {/* Toolbar */}
      {!loading && skills.length > 0 && (
        <div className="flex items-center gap-2 mb-4 flex-wrap">
          <div
            className="hub-card flex items-center gap-2 px-2.5 flex-1"
            style={{ height: 30, background: 'var(--hub-surface)', maxWidth: 360 }}
          >
            <Search size={13} style={{ color: 'var(--hub-ink-3)' }} />
            <input
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              className="flex-1 bg-transparent outline-none text-[13px]"
              style={{ color: 'var(--hub-ink)' }}
              placeholder={t('common.searchPlaceholder') || 'Search…'}
            />
            {search && (
              <button onClick={() => setSearch('')} className="hub-icon-btn sm">
                <X size={11} />
              </button>
            )}
          </div>

          <div className="ml-auto flex items-center gap-2">
            {selectedIds.size > 0 && (
              <button onClick={() => setShowExport(true)} className="hub-btn primary">
                <Upload size={13} /> {t('skills.exportToAgent')}
                <span className="hub-mono" style={{ fontSize: 11, opacity: 0.8 }}>
                  {selectedIds.size}
                </span>
              </button>
            )}
            <div className="hub-mono text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
              {pagination.total}/{skills.length}
            </div>
          </div>
        </div>
      )}

      {loading ? (
        <div className="hub-card p-10 text-center" style={{ color: 'var(--hub-ink-3)' }}>
          {t('app.loading')}
        </div>
      ) : skills.length === 0 ? (
        <div className="hub-card p-10 text-center" style={{ color: 'var(--hub-ink-3)' }}>
          <div className="flex flex-col items-center gap-3">
            <div
              className="grid place-items-center"
              style={{
                width: 40,
                height: 40,
                borderRadius: 10,
                border: '1px solid var(--hub-line)',
                background: 'var(--hub-bg-2)',
              }}
            >
              <Eye size={18} />
            </div>
            <div className="font-medium" style={{ color: 'var(--hub-ink-2)', fontSize: 13 }}>
              {t('skills.empty')}
            </div>
            {isAdmin && (
              <button
                onClick={() => setShowImport(true)}
                className="hub-btn ghost sm"
                style={{ color: 'var(--hub-accent)' }}
              >
                <Plus size={12} /> {t('skills.importFirst')}
              </button>
            )}
          </div>
        </div>
      ) : (
        <>
          {visibleSkills.length === 0 ? (
            <div className="hub-card p-10 text-center" style={{ color: 'var(--hub-ink-3)' }}>
              {t('market.noServers') || t('common.all')}
            </div>
          ) : (
            <div className="hub-card overflow-hidden">
              {visibleSkills.map((skill, idx) => {
                const checked = selectedIds.has(skill.id);
                return (
                  <div
                    key={skill.id}
                    className="flex items-center justify-between transition-colors hover:bg-[var(--hub-surface-hover)]"
                    style={{
                      padding: '12px 16px',
                      borderTop: idx === 0 ? 0 : '1px solid var(--hub-line-2)',
                      background: checked ? 'var(--hub-surface)' : undefined,
                    }}
                  >
                    <div className="flex items-center gap-2.5 flex-1 min-w-0">
                      <input
                        type="checkbox"
                        checked={checked}
                        onChange={() => toggleSelect(skill.id)}
                        className="h-4 w-4 text-blue-600 border-gray-300 rounded focus:ring-blue-500 flex-shrink-0"
                      />
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2 flex-wrap min-w-0">
                          <span
                            className="font-medium truncate"
                            style={{ fontSize: 13.5, color: 'var(--hub-ink)', maxWidth: 250 }}
                            title={skill.name}
                          >
                            {skill.name}
                          </span>
                          <span
                            className="hub-mono truncate"
                            style={{ fontSize: 11.5, color: 'var(--hub-ink-3)', maxWidth: 200 }}
                            title={skill.dirName}
                          >
                            {skill.dirName}
                          </span>
                        </div>
                        {skill.exports.length > 0 && (
                          <div className="flex items-center gap-1.5 flex-wrap mt-1">
                            <span
                              className="hub-mono"
                              style={{ fontSize: 10, color: 'var(--hub-ink-3)' }}
                            >
                              {t('skills.installedAgents')}:
                            </span>
                            {skill.exports.map((ex) => (
                              <span
                                key={ex.agentId}
                                className="hub-tag"
                                style={{
                                  fontSize: 10,
                                  display: 'inline-flex',
                                  alignItems: 'center',
                                  gap: 3,
                                  background: 'var(--hub-bg-2)',
                                  color: 'var(--hub-ink-2)',
                                }}
                                title={`${ex.agentName} (${ex.method === 'symlink' ? t('skills.symlink') : t('skills.fileCopy')})`}
                              >
                                {ex.method === 'symlink' ? <Link2 size={10} /> : <CopyIcon size={10} />}
                                {ex.agentName.length > 30 ? ex.agentName.slice(0, 30) + '…' : ex.agentName}
                              </span>
                            ))}
                          </div>
                        )}
                        {skill.description && (
                          <div className="truncate mt-0.5" style={{ fontSize: 12, color: 'var(--hub-ink-3)' }}>
                            {skill.description}
                          </div>
                        )}
                      </div>
                    </div>
                    <div className="flex items-center gap-1 ml-3">
                      {isAdmin && (
                        <button
                          onClick={() => setInstallSkill(skill)}
                          className="hub-icon-btn sm"
                          title={t('skills.install')}
                          style={{ color: 'var(--hub-accent)' }}
                        >
                          <PackageCheck size={13} />
                        </button>
                      )}
                      <button
                        onClick={() => setViewSkillId(skill.id)}
                        className="hub-icon-btn sm"
                        title={t('skills.view')}
                      >
                        <Eye size={13} />
                      </button>
                      <button
                        onClick={() => handleOpenSkillLibrary(skill.id)}
                        className="hub-icon-btn sm"
                        title={t('skills.openSkillFolder')}
                        style={{ color: 'var(--hub-ink-3)' }}
                      >
                        <Folder size={13} />
                      </button>
                      {isAdmin && (
                        <button
                          onClick={() => setSkillToDelete(skill)}
                          className="hub-icon-btn sm"
                          title={t('skills.delete')}
                          style={{ color: 'var(--hub-err)' }}
                        >
                          <Trash2 size={13} />
                        </button>
                      )}
                    </div>
                  </div>
                );
              })}
            </div>
          )}

          {/* Pagination footer */}
          <div className="flex items-center mt-4 text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
            <div className="flex-[2]">
              {t('common.showing', {
                start: (pagination.page - 1) * pagination.limit + 1,
                end: Math.min(pagination.page * pagination.limit, pagination.total),
                total: pagination.total,
              })}
            </div>
            <div className="flex-[4] flex justify-center">
              {pagination.totalPages > 1 && (
                <Pagination
                  currentPage={pagination.page}
                  totalPages={pagination.totalPages}
                  onPageChange={setPage}
                  disabled={loading}
                />
              )}
            </div>
            <div className="flex-[2] flex items-center justify-end gap-2">
              <label htmlFor="perPage">{t('common.itemsPerPage')}:</label>
              <select
                id="perPage"
                value={pageSize}
                onChange={(e) => {
                  setPageSize(Number(e.target.value));
                  setPage(1);
                }}
                disabled={loading}
                className="hub-input"
                style={{ height: 26, width: 70, padding: '0 6px', fontSize: 12 }}
              >
                <option value={5}>5</option>
                <option value={10}>10</option>
                <option value={20}>20</option>
                <option value={50}>50</option>
              </select>
            </div>
          </div>
        </>
      )}

      {/* Agent management dialog */}
      {showAgentMgmt && <AgentManagementDialog onClose={() => setShowAgentMgmt(false)} />}

      {/* Import dialog */}
      {showImport && (
        <ImportDialog
          onImport={handleImport}
          onClose={() => setShowImport(false)}
        />
      )}

      {/* View dialog */}
      {viewSkillId && (
        <ViewDialog
          skillId={viewSkillId}
          onUninstall={handleUninstall}
          onClose={() => setViewSkillId(null)}
        />
      )}

      {/* Export dialog (bulk, multi-select) */}
      {showExport && (
        <ExportDialog
          selectedCount={selectedIds.size}
          onExport={handleExport}
          onClose={() => setShowExport(false)}
        />
      )}

      {/* Install dialog (per-row, single skill → multiple agents) */}
      {installSkill && (
        <InstallDialog
          skill={installSkill}
          onInstall={handleInstall}
          onUninstall={handleUninstall}
          onClose={() => setInstallSkill(null)}
        />
      )}

      {/* Delete dialog (with optional cleanup of exported copies) */}
      {skillToDelete && (
        <DeleteSkillDialog
          skill={skillToDelete}
          onDelete={handleDelete}
          onClose={() => setSkillToDelete(null)}
        />
      )}
    </div>
  );
};

export default SkillsPage;
