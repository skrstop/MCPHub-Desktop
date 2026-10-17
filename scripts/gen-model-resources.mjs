#!/usr/bin/env node
/**
 * Generate a Tauri config override that trims bundled RAG model files.
 *
 * Only sizes whose deploy.json has `"bundle": true` ship their model files
 * (model.gguf etc.); every other size ships ONLY its deploy.json so the model
 * still appears in the dropdown as downloadable (via `downloadUrls`) without
 * inflating the installer. The legacy `default` key is honored as a fallback.
 *
 * No staging directory is created: the override REPLACES `bundle.resources`
 * with the base list plus per-size globs —
 *   - every size:            runtimes/rag/model/<family>/<size>/deploy.json
 *   - bundle:true sizes:     runtimes/rag/model/<family>/<size>/**
 *
 * Output: src-tauri/tauri.models.json — consumed by
 * `tauri build --config src-tauri/tauri.models.json` (see package.json
 * "build"). Tauri merges `--config` over tauri.conf.json (arrays are
 * replaced, hence the full resources list here).
 */
import { readdirSync, readFileSync, statSync, writeFileSync } from 'node:fs';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const conf = JSON.parse(readFileSync(join(root, 'src-tauri', 'tauri.conf.json'), 'utf-8'));
const src = join(root, 'src-tauri', 'runtimes', 'rag', 'model');

const bundled = [];
const stubbed = [];
try {
  for (const fam of readdirSync(src)) {
    const famPath = join(src, fam);
    if (!statSync(famPath).isDirectory()) continue;
    for (const size of readdirSync(famPath)) {
      const sizePath = join(famPath, size);
      if (!statSync(sizePath).isDirectory()) continue;
      let cfg;
      try {
        cfg = JSON.parse(readFileSync(join(sizePath, 'deploy.json'), 'utf-8'));
      } catch {
        console.warn(`[gen-model-resources] skip ${fam}/${size}: unreadable deploy.json`);
        continue;
      }
      ((cfg.bundle ?? cfg.default) === true ? bundled : stubbed).push(`${fam}/${size}`);
    }
  }
} catch {
  console.warn('[gen-model-resources] no model dir; shipping deploy.json glob only');
}

// Base resources minus the whole rag model dir, then re-add per-size globs.
const base = (conf.bundle?.resources ?? []).filter((r) => r !== 'runtimes/rag');
const resources = [
  ...base,
  'runtimes/rag/model/*/*/deploy.json',
  ...bundled.map((p) => `runtimes/rag/model/${p}/**`),
];

writeFileSync(
  join(root, 'src-tauri', 'tauri.models.json'),
  JSON.stringify({ bundle: { resources } }, null, 2) + '\n',
);
console.log(
  `[gen-model-resources] bundled: ${bundled.join(', ') || '(none)'} | download-only: ${stubbed.join(', ') || '(none)'}`,
);
