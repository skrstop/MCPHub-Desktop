import type React from "react";
import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { listen } from '@tauri-apps/api/event';
import { useToast } from '@/contexts/ToastContext';
import { isTauri } from '@/utils/tauriClient';
import { listRagDocs } from '@/services/ragService';
import { getSmartReindexParticipants } from '@/services/smartRoutingService';

type Phase = 'confirm' | 'running' | 'done';
type RowState = 'wait' | 'run' | 'ok' | 'none';

interface RagUploadProgressPayload {
  name: string;
  current: number;
  total: number;
  charsDone?: number;
  charsTotal?: number;
  chunksDone?: number;
  chunksTotal?: number;
}

interface Props {
  open: boolean;
  /** Dialog mode: 'switch' = model swap (default), 'reload' = plain data
   * reload with the current model (copy differs, no model mention). */
  mode?: 'switch' | 'reload';
  /** Which rows to show/work on: 'both' (default, model swap), 'rag' (RAG
   * page entry), 'smart' (Smart Routing entry). Drives display AND the
   * owner's onStart work via the same value. */
  scope?: 'both' | 'rag' | 'smart';
  /** Model the user switched TO (shown in the lead text). */
  targetModel: string;
  /** Whether Smart Routing is enabled (drives the second row's presence). */
  smartEnabled: boolean;
  /** RAG doc count for the confirm-phase meta (optional; hidden when absent). */
  ragDocCount?: number | null;
  /** Dismiss the dialog (cancel / clean-switch / after done). */
  onClose: () => void;
  /**
   * 开始重载 pressed: the OWNER performs the model switch + conditional
   * re-embed / smart reload. Returns 'reindexed' (dialog shows the done
   * phase) or 'clean' (switch needed no reload — dialog closes; the owner
   * has shown its own success toast).
   */
  onStart: () => Promise<'reindexed' | 'clean'>;
}

interface Row {
  current: number;
  total: number;
  currentName: string | null;
  subs: { label: string; done: number; total: number | null; tone: 'blue' | 'purple' | 'green'; unit?: string }[];
}

interface SubStat { done: number; total: number | null }

/**
 * Model-swap reindex dialog — pixel-faithful port of the approved mockup
 * (/tmp/mv_dialog/index2.html): confirm (amber "待重载" rows) → running
 * (dual progress bars; RAG row expands the legacy char/chunk/embed sub-bars
 * from rag://upload-progress; Smart row consumes smart://reindex-progress)
 * → done (green note). Cancel ALWAYS reverts to the previous model — a
 * dim-changing switch without reindex leaves the vector table dropped, so
 * there is no useful "stay on new model" path.
 */
export default function ReindexDialog({
  open,
  mode = 'switch',
  scope = 'both',
  targetModel,
  smartEnabled,
  ragDocCount,
  onClose,
  onStart,
}: Props) {
  const { t } = useTranslation();
  const { showToast } = useToast();
  const [phase, setPhase] = useState<Phase>('confirm');
  const [rag, setRag] = useState<Row | null>(null);
  const [smart, setSmart] = useState<Row | null>(null);
  // Confirm-phase meta counts ("133 篇" / "5 个服务器"), mirroring the
  // mockup. Smart count = read-only backend mirror of reindex_all's
  // denominator; fetched once when the dialog opens.
  const [smartCount, setSmartCount] = useState<{ servers: number; tools: number } | null>(null);
  // Self-fetched doc count: the parent's list may not be loaded when the
  // dialog opens (the confirm phase is evaluated the instant the user picks
  // a model), which showed a bogus "无需重载" for a 104-doc library. The
  // parent count is still the first paint; this fetch corrects it.
  const [docCount, setDocCount] = useState<number | null>(null);
  const mounted = useRef(true);
  const phaseRef = useRef<Phase>('confirm');
  // Elapsed-time stats (mirrors the RAG import overlay: 总耗时 + per-row).
  const runStartRef = useRef<number | null>(null);
  const ragStartRef = useRef<number | null>(null);
  const ragEndRef = useRef<number | null>(null);
  const smartStartRef = useRef<number | null>(null);
  const smartEndRef = useRef<number | null>(null);
  const [nowTick, setNowTick] = useState(0);
  // Last-known per-phase stats: events only carry the active phase's totals
  // (char-reading events have no chunk counts), but all three sub-bars stay
  // visible for the whole run — missing phases show a placeholder instead of
  // disappearing, and remembered values survive phase transitions.
  const subStatsRef = useRef<{ chars: SubStat; chunks: SubStat; embed: SubStat }>({
    chars: { done: 0, total: null },
    chunks: { done: 0, total: null },
    embed: { done: 0, total: null },
  });
  // Smart row per-tool progress within the current server (same remember
  // pattern as the RAG sub-stats).
  const smartToolRef = useRef<SubStat>({ done: 0, total: null });

  const resetSubStats = () => {
    subStatsRef.current = { chars: { done: 0, total: null }, chunks: { done: 0, total: null }, embed: { done: 0, total: null } };
    smartToolRef.current = { done: 0, total: null };
    runStartRef.current = null;
    ragStartRef.current = null;
    ragEndRef.current = null;
    smartStartRef.current = null;
    smartEndRef.current = null;
  };

  // 1s tick drives the elapsed-time displays while running.
  useEffect(() => {
    if (phase !== 'running') return;
    const t = setInterval(() => setNowTick((v) => v + 1), 1000);
    return () => clearInterval(t);
  }, [phase]);

  // Merge one progress payload into the remembered stats and build the three
  // always-present sub-rows.
  const buildSubs = (p: RagUploadProgressPayload) => {
    const s = subStatsRef.current;
    if (p.charsTotal != null) s.chars = { done: p.charsDone ?? 0, total: p.charsTotal };
    if (p.chunksTotal != null) {
      s.chunks = { done: p.chunksDone ?? 0, total: p.chunksTotal };
      s.embed = { done: p.chunksDone ?? 0, total: p.chunksTotal };
    }
    const mk = (label: string, st: SubStat, tone: 'blue' | 'purple' | 'green', unit?: string) => ({
      label, done: st.done, total: st.total, tone, unit,
    });
    return [
      mk(t('pages.rag.reembedBarChars'), s.chars, 'blue'),
      mk(t('pages.rag.reembedBarChunks'), s.chunks, 'purple', '块'),
      mk(t('pages.rag.reembedBarEmbed'), s.embed, 'green'),
    ];
  };

  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  // Reset when reopened so a second swap starts from the confirm phase.
  useEffect(() => {
    if (open) {
      setPhase('confirm');
      phaseRef.current = 'confirm';
      setRag(null);
      setSmart(null);
      resetSubStats();
    }
  }, [open]);

  // Confirm-phase smart server count (desktop only; silent on failure —
  // the meta is cosmetic, the reload itself does not depend on it).
  useEffect(() => {
    if (!open || !smartEnabled || !isTauri()) return;
    if (scope !== 'both' && scope !== 'smart') return;
    let alive = true;
    getSmartReindexParticipants()
      .then((n) => {
        if (alive) setSmartCount(n);
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [open, smartEnabled, scope]);

  // Authoritative doc count for the confirm meta + the 无需重载 decision.
  useEffect(() => {
    if (!open) return;
    setDocCount(null);
    let alive = true;
    listRagDocs()
      .then((docs) => {
        if (alive) setDocCount(docs.length);
      })
      .catch(() => {
        if (alive) setDocCount(0);
      });
    return () => {
      alive = false;
    };
  }, [open]);

  // Live progress listeners. Events are ONLY honored while phase ===
  // 'running': backend boot-restore / manual reindex can emit these events
  // while the dialog sits in confirm, and stale totals leaked into the
  // confirm rows in the first implementation (showed "已完成 5/5" before
  // the user even clicked start).
  useEffect(() => {
    if (!open) return;
    const unRag = listen<RagUploadProgressPayload>('rag://upload-progress', (e) => {
      const p = e.payload;
      if (!p || !mounted.current || phaseRef.current !== 'running') return;
      setRag((prev) => ({
        current: p.current || prev?.current || 0,
        total: p.total || prev?.total || 0,
        currentName: p.name || prev?.currentName || null,
        subs: buildSubs(p),
      }));
    });
    // Doc-count progress during a reload (reindex_all): file-level
    // current/total/name on a separate event from the char/chunk bars.
    // `current` is 0-based → store +1 so "N / M 篇" and the bar read naturally.
    const unReindex = listen<{ current: number; total: number; name: string }>(
      'rag://reindex-progress',
      (e) => {
        const p = e.payload;
        if (!p || !mounted.current || phaseRef.current !== 'running') return;
        if (p.total > 0 && p.current + 1 >= p.total && ragEndRef.current == null) {
          ragEndRef.current = Date.now();
        }
        setRag((prev) => ({
          current: p.current + 1,
          total: p.total,
          currentName: p.name || prev?.currentName || null,
          subs: prev?.subs ?? [],
        }));
      },
    );
    const unSmart = listen<{ current: number; total: number; name: string; tools?: number; toolDone?: number; toolTotal?: number }>(
      'smart://reindex-progress',
      (e) => {
        const p = e.payload;
        if (!p || !mounted.current || phaseRef.current !== 'running') return;
        // Smart row timing starts at its FIRST progress event (both-scope:
        // this is after RAG finished), and freezes at completion.
        if (smartStartRef.current == null) smartStartRef.current = Date.now();
        if (p.total > 0 && p.current >= p.total && smartEndRef.current == null) {
          smartEndRef.current = Date.now();
        }
        // Mockup form: "当前: Idea-mcp-server（62 个工具）"
        const name = p.tools && p.tools > 0 && p.name
          ? `${p.name}（${p.tools} ${t('pages.rag.reembedUnitTools')}）`
          : p.name || null;
        // Per-tool progress within the current server (remembered across
        // server transitions; reset when the next server starts at 0/total).
        if (p.toolTotal != null) {
          smartToolRef.current = { done: p.toolDone ?? 0, total: p.toolTotal };
        }
        const st = smartToolRef.current;
        setSmart({
          current: p.current,
          total: p.total,
          currentName: name,
          subs: st.total != null
            ? [{ label: t('pages.rag.reembedBarToolEmbed'), done: st.done, total: st.total, tone: 'green' as const }]
            : [],
        });
      },
    );
    return () => {
      unRag.then((un) => un());
      unReindex.then((un) => un());
      unSmart.then((un) => un());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  if (!open) return null;

  const rowState = (r: Row | null): RowState => {
    if (phase === 'confirm') return 'wait';
    if (phase === 'running' && r && r.total > 0 && r.current >= r.total) return 'ok';
    if (phase === 'running') return 'run';
    return 'ok'; // done phase
  };
  // A row with nothing to reload (0 docs / 0 servers) shows the neutral
  // 无需重载 tag instead of 待重载/已完成.
  const effectiveDocCount = docCount ?? ragDocCount ?? 0;
  const hasWork = (key: 'rag' | 'smart'): boolean => {
    if (key === 'rag') return effectiveDocCount > 0;
    return (smartCount?.servers ?? 0) > 1; // count includes the builtin virtual server; 1 = nothing beyond it
  };

  const tagFor = (s: RowState) => {
    const map: Record<RowState, { cls: React.CSSProperties; text: string }> = {
      wait: { cls: { background: '#fef3c7', color: '#d97706' }, text: t('pages.rag.reembedStateWait') },
      run: { cls: { background: '#dbeafe', color: '#3b82f6' }, text: t('pages.rag.reembedStateRunning') },
      ok: { cls: { background: '#dcfce7', color: '#16a34a' }, text: t('pages.rag.reembedStateDone') },
      none: { cls: { background: '#eef1f5', color: 'var(--hub-ink-3)' }, text: t('pages.rag.reembedStateNone') },
    };
    return map[s];
  };

  const fmt = (n: number) => n.toLocaleString();

  // 秒 → m分s秒（与 RAG 导入浮层的耗时格式一致）。
  const formatElapsed = (ms: number): string => {
    const total = Math.max(0, Math.floor(ms / 1000));
    const m = Math.floor(total / 60);
    const s = total % 60;
    return m > 0 ? `${m}m${s.toString().padStart(2, '0')}s` : `${s}s`;
  };

  const section = (
    key: 'rag' | 'smart',
    r: Row | null,
    metaConfirm: string | null,
    unit: string,
    startedAt: number | null,
    endedAt: number | null,
  ) => {
    let s = rowState(r);
    // Both-scope: Smart hasn't started while RAG is still running — show
    // 待重载 instead of a premature 重载中.
    if (key === 'smart' && s === 'run' && smartStartRef.current == null) s = 'wait';
    if (!hasWork(key)) s = 'none';
    const tag = tagFor(s);
    const purple = key === 'smart';
    const pct = r && r.total > 0 ? Math.min(100, Math.round((r.current / r.total) * 100)) : s === 'ok' ? 100 : 0;
    const meta =
      s === 'wait'
        ? (metaConfirm ?? `0 ${unit}`) // explicit "0 篇" when the list is empty
        : r && r.total > 0
          ? `${fmt(r.current)} / ${fmt(r.total)} ${unit}`
          : s === 'ok'
            ? `0 / 0 ${unit}`
            : null;
    return (
      <div
        className="rounded-[10px] p-3.5 flex flex-col gap-[7px] bg-gray-50 dark:bg-[var(--hub-item)]"
        style={{ border: '1px solid var(--hub-line)' }}
      >
        <div className="flex items-center gap-2">
          <span className="text-[13.5px] font-semibold flex-1" style={{ color: 'var(--hub-ink)' }}>
            {key === 'rag' ? t('pages.rag.reembedRowRag') : t('pages.rag.reembedRowSmart')}
          </span>
          {meta && (
            <span className="text-[11.5px] whitespace-nowrap" style={{ color: 'var(--hub-ink-3)' }}>
              {meta}
            </span>
          )}
          <span className="text-[10.5px] px-2 py-0.5 rounded-full font-semibold" style={tag.cls}>
            {tag.text}
          </span>
        </div>
        {(s === 'run' || s === 'ok') && (
          <>
            <div className="h-[5px] rounded-[3px] overflow-hidden" style={{ background: 'var(--hub-bg-2)' }}>
              <div
                className="h-full rounded-[3px]"
                style={{
                  width: `${pct}%`,
                  background: s === 'ok' && !purple ? '#16a34a' : purple ? '#8b5cf6' : '#3b82f6',
                  transition: 'width .3s',
                }}
              />
            </div>
            {startedAt != null && (s === 'run' || s === 'ok') && (
              <div className="text-[11px]" style={{ color: 'var(--hub-ink-3)' }}>
                {t('pages.rag.rowElapsed', '耗时')}{' '}
                {formatElapsed(Math.max(0, (s === 'ok' ? endedAt ?? Date.now() : Date.now()) - startedAt))}
              </div>
            )}
            {s === 'run' && r?.currentName && (
              <div className="text-[11.5px] truncate" style={{ color: 'var(--hub-ink-3)' }}>
                {t('pages.rag.reembedCurrent')}: <b style={{ color: 'var(--hub-ink-2)', fontWeight: 500 }}>{r.currentName}</b>
              </div>
            )}
            {key === 'smart' && s === 'ok' && r?.currentName && (
              <div className="text-[11.5px] truncate" style={{ color: 'var(--hub-ink-3)' }}>
                {r.currentName}
              </div>
            )}
            {s === 'run' && r && r.subs.length > 0 && (
              <div
                className="flex flex-col gap-1.5 border-t border-dashed pt-2 mt-0.5"
                style={{ borderColor: 'var(--hub-line)' }}
              >
                {r.subs.map((sb, i) => {
                  const sp = sb.total ? Math.min(100, Math.round((sb.done / sb.total) * 100)) : 0;
                  return (
                    <div key={i} className="flex items-center gap-2 min-w-0">
                      <span className="text-[11px] w-[70px] shrink-0" style={{ color: 'var(--hub-ink-2)' }}>{sb.label}</span>
                      <div className="flex-1 min-w-[40px] h-[4px] rounded-[2px] overflow-hidden" style={{ background: 'var(--hub-bg-2)' }}>
                        <div
                          className="h-full rounded-[2px]"
                          style={{
                            width: `${sp}%`,
                            background: sb.tone === 'blue' ? '#3b82f6' : sb.tone === 'purple' ? '#8b5cf6' : '#16a34a',
                          }}
                        />
                      </div>
                      <span
                        className="text-[10.5px] shrink-0 whitespace-nowrap text-right"
                        style={{ color: 'var(--hub-ink-3)', fontVariantNumeric: 'tabular-nums' }}
                      >
                        {sb.total != null ? `${fmt(sb.done)} / ${fmt(sb.total)}${sb.unit || ''}` : '—'}
                      </span>
                    </div>
                  );
                })}
              </div>
            )}
          </>
        )}
      </div>
    );
  };

  const start = async () => {
    setPhase('running');
    phaseRef.current = 'running';
    runStartRef.current = Date.now();
    ragStartRef.current = Date.now();
    // Independent per-row timing: RAG starts immediately; in a both-scope run
    // Smart starts when its first progress event arrives (after RAG), so its
    // elapsed excludes the RAG phase. Smart-only scope starts right away.
    smartStartRef.current = smartEnabled && scope === 'smart' ? Date.now() : null;
    // Seed rows so bars render before the first event arrives.
    setRag({ current: 0, total: 0, currentName: null, subs: buildSubs({ name: '', current: 0, total: 0 }) });
    setSmart(
      smartEnabled && (scope === 'both' || scope === 'smart')
        ? { current: 0, total: 0, currentName: null, subs: [] }
        : null,
    );
    try {
      const res = await onStart();
      if (!mounted.current) return;
      if (res === 'clean') {
        // Switch needed no reload — owner toasted; just close.
        onClose();
        return;
      }
      setPhase('done');
      phaseRef.current = 'done';
    } catch (e) {
      showToast(e instanceof Error ? e.message : String(e), 'error');
      if (mounted.current) {
        setPhase('done');
        phaseRef.current = 'done';
      }
    }
  };

  const cancel = () => {
    if (phase === 'running') return; // no mid-run cancel (bars are the feedback)
    onClose();
  };

  return (
    <div className="fixed inset-0 bg-black/50 z-[70] flex items-center justify-center p-4" style={{ pointerEvents: 'auto' }}>
      <div
        className="rounded-[14px] shadow-2xl max-w-[540px] w-full mx-4 overflow-hidden bg-white dark:bg-[var(--hub-surface)]"
        style={{ border: '1px solid var(--hub-line)' }}
      >
        <div
          className="flex items-center justify-between px-[22px] py-4"
          style={{ borderBottom: '1px solid var(--hub-line)' }}
        >
          <h2 className="text-[16px] font-bold" style={{ color: 'var(--hub-ink)' }}>
            {t('pages.rag.reembedDialogTitle')}
          </h2>
        </div>
        <div className="px-[22px] py-4 flex flex-col gap-3.5">
          <p className="text-[13px] leading-relaxed" style={{ color: 'var(--hub-ink-2)' }}>
            {mode === 'reload'
              ? t('pages.rag.reembedDialogLeadReload')
              : t('pages.rag.reembedDialogLead', { model: targetModel })}
          </p>
          {(scope === 'both' || scope === 'rag') &&
            section('rag', rag, `${fmt(effectiveDocCount)} ${t('pages.rag.reembedUnitDocs')}`, t('pages.rag.reembedUnitDocs'), ragStartRef.current, ragEndRef.current)}
          {smartEnabled &&
            (scope === 'both' || scope === 'smart') &&
            section(
              'smart',
              smart,
              smartCount != null
                ? `${smartCount.servers} ${t('pages.rag.reembedUnitServers')} ${smartCount.tools} ${t('pages.rag.reembedUnitTools')}`
                : null,
              t('pages.rag.reembedUnitServers'),
              smartStartRef.current,
              smartEndRef.current,
            )}
          {phase === 'running' && runStartRef.current != null && (
            <div className="text-[11.5px] text-right" style={{ color: 'var(--hub-ink-3)' }}>
              {t('pages.rag.totalElapsed', '总耗时')} {formatElapsed(Math.max(0, Date.now() - runStartRef.current))}
            </div>
          )}
          {phase === 'confirm' && (
            <div
              className="flex gap-2 rounded-lg px-3 py-2.5 text-[12px] leading-relaxed"
              style={{ background: '#fffbeb', border: '1px solid #fde68a', color: '#92600a' }}
            >
              ⚠ {t('pages.rag.reembedCancelHint')}
            </div>
          )}
          {phase === 'done' && (
            <div
              className="flex gap-2 rounded-lg px-3 py-2.5 text-[12px] leading-relaxed"
              style={{ background: '#f0fdf4', border: '1px solid #bbf7d0', color: '#166534' }}
            >
              ✓ {mode === 'reload'
                ? t('pages.rag.reembedDoneHintReload')
                : t('pages.rag.reembedDoneHint', { model: targetModel })}
            </div>
          )}
        </div>
        <div
          className="flex justify-end gap-2.5 px-[22px] py-3"
          style={{ borderTop: '1px solid var(--hub-line)' }}
        >
          {phase === 'confirm' && (
            <>
              <button onClick={cancel} className="hub-btn">
                {mode === 'reload' ? t('common.cancel') : t('pages.rag.reembedCancelRevert')}
              </button>
              <button onClick={start} className="hub-btn primary">
                {t('pages.rag.reembedStart')}
              </button>
            </>
          )}
          {phase === 'running' && (
            <button className="hub-btn primary" disabled>
              <span
                className="inline-block w-3 h-3 border-2 rounded-full align-[-2px] mr-1.5"
                style={{ borderColor: '#fff', borderTopColor: 'transparent', animation: 'reindex-spin .7s linear infinite' }}
              />
              {t('pages.rag.reembedRunning')}
            </button>
          )}
          {phase === 'done' && (
            <button onClick={onClose} className="hub-btn primary">
              {t('pages.rag.reembedFinish')}
            </button>
          )}
        </div>
        <style>{`@keyframes reindex-spin { to { transform: rotate(360deg) } }`}</style>
      </div>
    </div>
  );
}
