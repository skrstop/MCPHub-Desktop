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
  /** Chunking-phase ticks (big files): scanned chars / doc chars. Present
   *  only while the chunker is running (charsDone stays 0 then). */
  chunkingDone?: number;
  chunkingTotal?: number;
  /** Real source-file size in bytes (from DocMeta.size). 0/absent = unknown
   *  — the large-file hint falls back to a char-count approximation. */
  bytesTotal?: number;
  /** Document char total (embed ticks carry the chunk-sum denominator in
   *  charsTotal — this is the true doc size, used to heal a missed chunking
   *  phase when the listener registered late). */
  docChars?: number;
}

interface Props {
  open: boolean;
  /** Dialog mode: 'switch' = model swap (default), 'reload' = plain data
   * reload with the current model (copy differs, no model mention),
   * 'import' = file import / single-doc update — skips the confirm phase
   * (the upload dialog already confirmed), runs straight in the running
   * phase with import-specific texts, and flips to done when the parent
   * raises `importFinished`. */
  mode?: 'switch' | 'reload' | 'import';
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
  // ── import-mode texts (all optional; defaults provided) ──
  importTitle?: string;
  importLead?: string;
  importRowLabel?: string;
  importDoneText?: string;
  /** import mode: parent raises this when the batch finished (success or
   * partial) — flips the dialog from running to the done phase. */
  importFinished?: boolean;
  /** import mode: file-level batch progress from the parent's upload loop
   * (the rag://upload-progress events carry NO current/total — they're
   * char/chunk-level only — so the "N / M 篇" row meta and file bar would
   * stick at 0/0 without this). */
  importProgress?: { current: number; total: number; name: string } | null;
  /**
   * 开始重载 pressed: the OWNER performs the model switch + conditional
   * re-embed / smart reload. Returns 'reindexed' (dialog shows the done
   * phase) or 'clean' (switch needed no reload — dialog closes; the owner
   * has shown its own success toast). Optional in import mode (no start
   * button — the parent already kicked off the batch).
   */
  onStart?: () => Promise<'reindexed' | 'clean'>;
}

interface Row {
  current: number;
  total: number;
  currentName: string | null;
  subs: { label: string; done: number; total: number | null; tone: 'blue' | 'purple' | 'green'; unit?: string; extra?: string }[];
}

interface SubStat { done: number; total: number | null; unit?: string; extra?: string }

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
  importTitle,
  importLead,
  importRowLabel,
  importDoneText,
  importFinished,
  importProgress,
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
  const subStatsRef = useRef<{ chars: SubStat; chunking: SubStat; chunks: SubStat; embed: SubStat; chunkCount: number; bytes: number | null }>({
    chars: { done: 0, total: null },
    chunking: { done: 0, total: null },
    chunks: { done: 0, total: null },
    embed: { done: 0, total: null },
    chunkCount: 0,
    bytes: null,
  });
  // Smart row per-tool progress within the current server (same remember
  // pattern as the RAG sub-stats).
  const smartToolRef = useRef<SubStat>({ done: 0, total: null });

  const resetSubStats = () => {
    subStatsRef.current = { chars: { done: 0, total: null }, chunking: { done: 0, total: null }, chunks: { done: 0, total: null }, embed: { done: 0, total: null }, chunkCount: 0, bytes: null };
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

  // Merge one progress payload into the remembered stats and build the four
  // always-present sub-rows (order restored from the lost-session dialog:
  // 分片进度 ABOVE 内容读取).
  const buildSubs = (p: RagUploadProgressPayload) => {
    const s = subStatsRef.current;
    // Real source-file size (first non-zero wins; survives phase switches).
    if ((p.bytesTotal ?? 0) > 0) s.bytes = p.bytesTotal!;
    // 阶段覆盖顺序（后端同一事件源的三种 tick 形态）：
    // ① 字符 tick（预分片 0% + 嵌入期累计）：charsTotal>0 即生效（含 done=0，
    //    否则预分片 tick 全「—」）。嵌入期的字符 tick 分母是「分片字符和」
    //    （含 overlap，> 文档字符数），只用于嵌入进度——分片开始后内容读取
    //    已完成，锁定满格，不再被嵌入期 tick 回写（否则出现「阶段1未满、
    //    阶段2已满」的错序观感）。
    if (p.charsTotal != null && p.charsTotal > 0 && s.chunking.total == null) {
      s.chars = { done: p.charsDone ?? 0, total: p.charsTotal };
    }
    // ② 分片期 tick（大文件逐分区）：内容读取此时已完成（置满），分片进度条
    //    按扫描字符填充，并累计后端上报的已分出块数（chunksDone=chunks so far）。
    if ((p.chunkingTotal ?? 0) > 0) {
      s.chars = { done: p.chunkingTotal!, total: p.chunkingTotal! };
      s.chunking = { done: p.chunkingDone ?? 0, total: p.chunkingTotal! };
      if ((p.chunksDone ?? 0) > 0) s.chunkCount = Math.max(s.chunkCount, p.chunksDone!);
    }
    // ③ 嵌入期 tick：分片进度条置满，真实分片计数接管分段条 + 语义计算条。
    if ((p.chunksTotal ?? 0) > 0) {
      // 自愈：分片期 tick 可能早于监听器注册/running 就绪而被丢弃（大文件
      // 分片数十秒，弹框刚打开时尤其如此）。首个嵌入 tick 若从未见过分片
      // tick，用 docChars（文档真实字符数）把 内容读取/分段处理 直接置满。
      if (s.chunking.total == null) {
        const dc = p.docChars ?? 0;
        if (dc > 0) {
          s.chars = { done: dc, total: dc };
          s.chunking = { done: dc, total: dc };
        }
      } else {
        s.chunking = { done: s.chunking.total!, total: s.chunking.total! };
      }
      s.chunks = { done: p.chunksDone ?? 0, total: p.chunksTotal!, unit: '块' };
      s.embed = { done: p.chunksDone ?? 0, total: p.chunksTotal! };
    }
    const mk = (label: string, st: SubStat, tone: 'blue' | 'purple' | 'green', unit?: string) => ({
      label, done: st.done, total: st.total, tone, unit: st.unit ?? unit, extra: st.extra,
    });
    // 四条子进度按处理顺序排列：内容读取 → 分段处理（分片扫描字符）→
    // 分片进度（块数；分片期总数未知时显示「已分片 N 块」后缀）→ 语义计算。
    const chunkCountRow: SubStat = (s.chunks.total ?? 0) > 0
      ? { ...s.chunks, unit: undefined }
      : { done: 0, total: null, extra: s.chunkCount > 0 ? t('pages.rag.chunkedCount', { n: s.chunkCount.toLocaleString() }) : undefined };
    return [
      mk(t('pages.rag.reembedBarChars'), s.chars, 'blue'),
      mk(t('pages.rag.reembedBarChunks'), s.chunking, 'purple'),
      mk(t('pages.rag.chunkProgress', '分片进度'), chunkCountRow, 'purple'),
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
  // Import mode skips confirm entirely (the upload dialog already confirmed)
  // and seeds the running row so bars render before the first event.
  useEffect(() => {
    if (open) {
      if (mode === 'import') {
        resetSubStats();
        setPhase('running');
        phaseRef.current = 'running';
        runStartRef.current = Date.now();
        ragStartRef.current = Date.now();
        setRag({ current: 0, total: 0, currentName: null, subs: buildSubs({ name: '', current: 0, total: 0 }) });
        setSmart(null);
      } else {
        setPhase('confirm');
        phaseRef.current = 'confirm';
        setRag(null);
        setSmart(null);
        resetSubStats();
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, mode]);

  // Import mode: the parent raises importFinished when the batch completes —
  // flip running → done (freeze the row timer).
  useEffect(() => {
    if (mode !== 'import' || !importFinished) return;
    if (phaseRef.current === 'running') {
      if (ragEndRef.current == null) ragEndRef.current = Date.now();
      setPhase('done');
      phaseRef.current = 'done';
    }
  }, [mode, importFinished]);

  // Import mode: sync the file-level row (current/total/name) from the
  // parent's upload loop — the events don't carry these fields. The loop's
  // final tick sets current=total (done marker); +1 would read "2/1" — clamp.
  useEffect(() => {
    if (mode !== 'import' || !importProgress) return;
    setRag((prev) => ({
      current: Math.min(importProgress.current + 1, Math.max(importProgress.total, 1)),
      total: importProgress.total,
      currentName: importProgress.name || prev?.currentName || null,
      subs: prev?.subs ?? [],
    }));
  }, [mode, importProgress?.current, importProgress?.total, importProgress?.name]);

  // Confirm-phase smart server count (desktop only; silent on failure —
  // the meta is cosmetic, the reload itself does not depend on it).
  useEffect(() => {
    if (!open || mode === 'import' || !smartEnabled || !isTauri()) return;
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
  }, [open, smartEnabled, scope, mode]);

  // Authoritative doc count for the confirm meta + the 无需重载 decision.
  useEffect(() => {
    if (!open || mode === 'import') return;
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
  }, [open, mode]);

  // Live progress listeners. Events are ONLY honored while phase ===
  // 'running': backend boot-restore / manual reindex can emit these events
  // while the dialog sits in confirm, and stale totals leaked into the
  // confirm rows in the first implementation (showed "已完成 5/5" before
  // the user even clicked start).
  // modeRef: the listener effect intentionally only depends on [open]; a ref
  // keeps the import-mode current+1 semantics from going stale.
  const modeRef = useRef(mode);
  modeRef.current = mode;
  useEffect(() => {
    if (!open) return;
    const unRag = listen<RagUploadProgressPayload>('rag://upload-progress', (e) => {
      const p = e.payload;
      if (!p || !mounted.current || phaseRef.current !== 'running') return;
      // Import mode: current/total come from the parent's upload loop
      // (importProgress prop — events carry none); only the sub-bars update
      // here. Reload mode keeps the legacy behavior.
      setRag((prev) => ({
        current: modeRef.current === 'import' ? (prev?.current ?? 0) : (p.current || prev?.current || 0),
        total: modeRef.current === 'import' ? (prev?.total ?? 0) : (p.total || prev?.total || 0),
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
    const unSmart = modeRef.current === 'import' ? Promise.resolve(() => {}) : listen<{ current: number; total: number; name: string; tools?: number; toolDone?: number; toolTotal?: number }>(
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
    if (phase === 'running') {
      // Stay 'run' for the whole running phase in EVERY mode: the file-level
      // counter hits current>=total as soon as the LAST doc STARTS processing
      // (reindex-progress fires at loop start), which flipped the row to ok
      // and hid the sub-bars (chunk progress + large-file hint) during the
      // longest phase — for a 1-doc reload, the ENTIRE run. Completion is
      // driven by the phase transition (onStart resolving / importFinished),
      // not by the counter.
      return 'run';
    }
    return 'ok'; // done phase
  };
  // A row with nothing to reload (0 docs / 0 servers) shows the neutral
  // 无需重载 tag instead of 待重载/已完成. Import mode always has work (the
  // parent opened the dialog because files are being processed).
  const effectiveDocCount = docCount ?? ragDocCount ?? 0;
  const hasWork = (key: 'rag' | 'smart'): boolean => {
    if (mode === 'import') return true;
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
  // 大文件提示阈值 + 字节格式化（与 RagPage 导入浮层同口径）。
  const LARGE_FILE_HINT_BYTES = 1024 * 1024;
  const formatBytes = (b: number): string =>
    b < 1024 * 1024 ? `${(b / 1024).toFixed(1)} KB` : `${(b / 1024 / 1024).toFixed(1)} MB`;

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
    labelOverride?: string,
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
            {labelOverride ?? (key === 'rag' ? t('pages.rag.reembedRowRag') : t('pages.rag.reembedRowSmart'))}
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
                    <div key={i} className="min-w-0">
                      {/* 第一行：固定宽标签 + 进度条（右侧无数字，条长恒定）。 */}
                      <div className="flex items-center gap-2 min-w-0">
                        <span className="text-[11px] w-[70px] shrink-0" style={{ color: 'var(--hub-ink-2)' }}>{sb.label}</span>
                        <div className="flex-1 h-[4px] rounded-[2px] overflow-hidden" style={{ background: 'var(--hub-bg-2)' }}>
                          <div
                            className="h-full rounded-[2px]"
                            style={{
                              width: `${sp}%`,
                              background: sb.tone === 'blue' ? '#3b82f6' : sb.tone === 'purple' ? '#8b5cf6' : '#16a34a',
                            }}
                          />
                        </div>
                      </div>
                      {/* 第二行：数字进度（右对齐，不再挤压条长）。 */}
                      <div
                        className="text-[10.5px] whitespace-nowrap text-right"
                        style={{ color: 'var(--hub-ink-3)', fontVariantNumeric: 'tabular-nums', marginTop: 1 }}
                      >
                        {sb.total != null ? `${fmt(sb.done)} / ${fmt(sb.total)}${sb.unit || ''}${sb.extra ? ` · ${sb.extra}` : ''}` : sb.extra ? sb.extra : '—'}
                      </div>
                    </div>
                  );
                })}
                {/* 大文件提示：当前文档体积超过阈值时提示分片/向量化耗时。 */}
                {(() => {
                  const charsTotal = subStatsRef.current.chars.total ?? 0;
                  // Prefer the real source-file size from the backend
                  // (DocMeta.size); fall back to a char-count approximation
                  // (1 字符 ≈ 1.5 字节) for older events without bytesTotal.
                  const approxBytes = subStatsRef.current.bytes ?? Math.round(charsTotal * 1.5);
                  if (approxBytes < LARGE_FILE_HINT_BYTES) return null;
                  return (
                    <div className="text-[11px]" style={{ color: 'var(--hub-ink-3)', marginTop: 2 }}>
                      {t('pages.rag.largeFileHint', '大文件导入耗时较长（分片与向量化），请耐心等待')}
                      {' · '}
                      {formatBytes(approxBytes)}
                    </div>
                  );
                })()}
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
      const res = await onStart!();
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
            {mode === 'import'
              ? (importTitle ?? t('pages.rag.importDialogTitle', '导入文档'))
              : t('pages.rag.reembedDialogTitle')}
          </h2>
        </div>
        <div className="px-[22px] py-4 flex flex-col gap-3.5">
          <p className="text-[13px] leading-relaxed" style={{ color: 'var(--hub-ink-2)' }}>
            {mode === 'import'
              ? (importLead ?? (phase === 'done'
                  ? t('pages.rag.importDialogLeadDone', '所选文件已导入并建立向量索引。')
                  : t('pages.rag.importDialogLead', '正在导入所选文件并建立向量索引，大文件可能耗时较长。')))
              : mode === 'reload'
              ? t('pages.rag.reembedDialogLeadReload')
              : t('pages.rag.reembedDialogLead', { model: targetModel })}
          </p>
          {(scope === 'both' || scope === 'rag') &&
            section('rag', rag, `${fmt(effectiveDocCount)} ${t('pages.rag.reembedUnitDocs')}`, t('pages.rag.reembedUnitDocs'), ragStartRef.current, ragEndRef.current, mode === 'import' ? (importRowLabel ?? t('pages.rag.importRowDocs', '导入文档')) : undefined)}
          {mode !== 'import' &&
            smartEnabled &&
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
          {phase === 'confirm' && mode === 'switch' && (
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
              ✓ {mode === 'import'
                ? (importDoneText ?? t('pages.rag.importDoneHint', '导入完成。'))
                : mode === 'reload'
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
              {mode === 'import' ? t('pages.rag.importRunning', '导入中…') : t('pages.rag.reembedRunning')}
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
