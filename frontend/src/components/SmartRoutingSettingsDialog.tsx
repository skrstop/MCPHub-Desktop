import React, { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { X, SlidersHorizontal } from 'lucide-react';
import type { SmartRoutingConfig } from '@/contexts/SettingsContext';

/** Slider row mirroring SettingsPage's SrSlider (label + number input + range). */
const Slider: React.FC<{
  label: string;
  value: number;
  min: number;
  max: number;
  step: number;
  onChange: (v: number) => void;
}> = ({ label, value, min, max, step, onChange }) => {
  const decimals = step < 1 ? 2 : 0;
  return (
    <div>
      <div className="flex items-center justify-between mb-1 gap-2">
        <label className="text-[13px] font-medium" style={{ color: 'var(--hub-ink)' }}>
          {label}
        </label>
        <input
          type="number"
          min={min}
          max={max}
          step={step}
          value={value.toFixed(decimals)}
          onChange={(e) => {
            const v = parseFloat(e.target.value);
            if (Number.isNaN(v)) return;
            onChange(Math.max(min, Math.min(max, v)));
          }}
          className="hub-input w-20 text-right text-[13px] py-1"
        />
      </div>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        onChange={(e) => onChange(parseFloat(e.target.value))}
        className="w-full accent-blue-600"
      />
    </div>
  );
};

/** Smart Routing retrieval settings (desktop hybrid search): vector/keyword
 *  weights, max results, score threshold. These are the SmartRoutingSettings
 *  keys (config_json.smartRouting.*) — independent of RAG's RagSettings, and
 *  they drive smart_routing::search directly (clamped server-side on read). */
const SmartRoutingSettingsDialog: React.FC<{
  onClose: () => void;
  config: SmartRoutingConfig;
  onSave: (updates: Partial<SmartRoutingConfig>) => Promise<boolean | undefined>;
}> = ({ onClose, config, onSave }) => {
  const { t } = useTranslation();
  const [vectorWeight, setVectorWeight] = useState(config.vectorWeight ?? 0.5);
  const [keywordWeight, setKeywordWeight] = useState(config.keywordWeight ?? 0.5);
  const [maxResults, setMaxResults] = useState(config.maxResults ?? 20);
  const [scoreThreshold, setScoreThreshold] = useState(config.scoreThreshold ?? 0.5);
  const [saving, setSaving] = useState(false);

  // Weight linkage (same semantics as the SettingsPage sliders): the two
  // weights are complementary so the sum stays 1.0.
  const handleWeightChange = (key: 'vector' | 'keyword', v: number) => {
    const clamped = Math.max(0, Math.min(1, v));
    const other = Math.max(0, Math.min(1, 1 - clamped));
    if (key === 'vector') {
      setVectorWeight(clamped);
      setKeywordWeight(other);
    } else {
      setKeywordWeight(clamped);
      setVectorWeight(other);
    }
  };

  const handleSave = async () => {
    setSaving(true);
    try {
      const ok = await onSave({
        vectorWeight,
        keywordWeight,
        maxResults: Math.max(1, Math.min(200, Math.round(maxResults))),
        scoreThreshold,
      });
      if (ok) onClose();
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="fixed inset-0 bg-black/50 z-50 flex items-center justify-center p-4">
      <div
        role="dialog"
        aria-modal="true"
        aria-label={t('server.smartRoutingSettings.title')}
        className="bg-white dark:bg-gray-800 rounded-xl shadow-2xl max-w-lg w-full mx-4 border border-gray-100 dark:border-gray-700 max-h-[90vh] flex flex-col"
      >
        <div className="flex items-center justify-between p-5 border-b border-[var(--hub-line-2)]">
          <h2 className="text-xl font-bold text-gray-900 dark:text-gray-100">
            {t('server.smartRoutingSettings.title')}
          </h2>
          <button onClick={onClose} className="hub-icon-btn sm" aria-label={t('common.close')}>
            <X size={16} />
          </button>
        </div>
        <div className="flex-1 overflow-y-auto p-5 space-y-4">
          <p className="text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
            {t('server.smartRoutingSettings.hint')}
          </p>
          <div className="space-y-4">
            <Slider
              label={t('settings.srVectorWeight')}
              value={vectorWeight}
              min={0}
              max={1}
              step={0.01}
              onChange={(v) => handleWeightChange('vector', v)}
            />
            <Slider
              label={t('settings.srKeywordWeight')}
              value={keywordWeight}
              min={0}
              max={1}
              step={0.01}
              onChange={(v) => handleWeightChange('keyword', v)}
            />
            <Slider
              label={t('settings.srMaxResults')}
              value={maxResults}
              min={1}
              max={200}
              step={1}
              onChange={(v) => setMaxResults(Math.round(v))}
            />
            <Slider
              label={t('settings.srScoreThreshold')}
              value={scoreThreshold}
              min={0}
              max={1}
              step={0.01}
              onChange={setScoreThreshold}
            />
          </div>
          <p className="text-[12px]" style={{ color: 'var(--hub-ink-3)' }}>
            {t('server.smartRoutingSettings.thresholdHint')}
          </p>
        </div>
        <div className="flex items-center justify-end gap-2 p-5 border-t border-[var(--hub-line-2)]">
          <button onClick={onClose} className="hub-btn">
            {t('common.cancel')}
          </button>
          <button onClick={handleSave} className="hub-btn primary" disabled={saving}>
            {!saving && <SlidersHorizontal size={13} />} {t('common.save')}
          </button>
        </div>
      </div>
    </div>
  );
};

export default SmartRoutingSettingsDialog;
