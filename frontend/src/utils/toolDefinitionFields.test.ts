import { describe, it, expect } from 'vitest';
import {
  CORE_TOOL_FIELDS,
  TOOL_DEFINITION_FIELDS,
  DEFAULT_TOOL_DEFINITION_FIELDS,
  fieldTipKey,
  allFieldTipKeys,
  toggleField,
} from './toolDefinitionFields';
import en from '../../../locales/en.json';
import zh from '../../../locales/zh.json';
import fr from '../../../locales/fr.json';
import tr from '../../../locales/tr.json';
import es from '../../../locales/es.json';

const LOCALES: Record<string, any> = { en, zh, fr, tr, es };

describe('TOOL_DEFINITION_FIELDS — list shape (origin #1286 parity)', () => {
  it('contains exactly the six optional fields in origin order', () => {
    expect([...TOOL_DEFINITION_FIELDS]).toEqual([
      'title',
      'annotations',
      'outputSchema',
      'execution',
      'icons',
      '_meta',
    ]);
  });

  it('default selection is title + annotations (origin parity)', () => {
    expect([...DEFAULT_TOOL_DEFINITION_FIELDS]).toEqual(['title', 'annotations']);
  });

  it('default is a subset of the optional list', () => {
    for (const f of DEFAULT_TOOL_DEFINITION_FIELDS) {
      expect(TOOL_DEFINITION_FIELDS).toContain(f);
    }
  });

  it('core four fields are name/description/inputSchema/serverName and never overlap the optional list', () => {
    expect([...CORE_TOOL_FIELDS]).toEqual(['name', 'description', 'inputSchema', 'serverName']);
    for (const c of CORE_TOOL_FIELDS) {
      expect(TOOL_DEFINITION_FIELDS).not.toContain(c);
    }
  });
});

describe('fieldTipKey — per-field tooltip keys', () => {
  it('capitalizes the first letter for each optional field; _meta maps to fieldMeta', () => {
    expect(fieldTipKey('title')).toBe('settings.fieldTitle');
    expect(fieldTipKey('annotations')).toBe('settings.fieldAnnotations');
    expect(fieldTipKey('outputSchema')).toBe('settings.fieldOutputSchema');
    expect(fieldTipKey('execution')).toBe('settings.fieldExecution');
    expect(fieldTipKey('icons')).toBe('settings.fieldIcons');
    expect(fieldTipKey('_meta')).toBe('settings.fieldMeta');
  });

  it('capitalizes core fields too', () => {
    expect(fieldTipKey('name')).toBe('settings.fieldName');
    expect(fieldTipKey('description')).toBe('settings.fieldDescription');
    expect(fieldTipKey('inputSchema')).toBe('settings.fieldInputSchema');
    expect(fieldTipKey('serverName')).toBe('settings.fieldServerName');
  });

  it('every field (core + optional) has a tip key listed by allFieldTipKeys', () => {
    expect(allFieldTipKeys().length).toBe(
      CORE_TOOL_FIELDS.length + TOOL_DEFINITION_FIELDS.length,
    );
    // No duplicates.
    expect(new Set(allFieldTipKeys()).size).toBe(allFieldTipKeys().length);
  });

  it('EVERY tip key exists in ALL five locales and is a non-empty string', () => {
    for (const [lang, dict] of Object.entries(LOCALES)) {
      for (const key of allFieldTipKeys()) {
        const v = key.split('.').reduce<any>((o, k) => (o == null ? o : o[k]), dict);
        expect([lang, key, typeof v === 'string' && v.length > 0]).toContain(true);
        expect(v, `${lang}.${key} must be a non-empty string`).toBeTruthy();
        expect(typeof v).toBe('string');
      }
    }
  });

  it('per-field tips are distinct across fields in every locale (no copy-paste)', () => {
    for (const [lang, dict] of Object.entries(LOCALES)) {
      const tips = allFieldTipKeys().map(
        (key) => key.split('.').reduce<any>((o, k) => o[k], dict) as string,
      );
      expect(new Set(tips).size, `${lang} tips must be unique`).toBe(tips.length);
    }
  });

  it('section headers exist in all locales', () => {
    for (const [lang, dict] of Object.entries(LOCALES)) {
      expect(typeof dict.settings.toolFieldCoreTitle).toBe('string');
      expect(dict.settings.toolFieldCoreTitle.length).toBeGreaterThan(0);
      expect(typeof dict.settings.toolFieldOptionalTitle).toBe('string');
      expect(dict.settings.toolFieldOptionalTitle.length).toBeGreaterThan(0);
      expect(typeof dict.settings.toolDefinitionFields).toBe('string');
      expect(typeof dict.settings.toolDefinitionFieldsDescription).toBe('string');
    }
  });
});

describe('toggleField — checkbox toggle state machine (per field)', () => {
  it('checking an unchecked field adds it, preserving list order', () => {
    expect(toggleField('outputSchema', ['title', 'annotations'], true)).toEqual([
      'title',
      'annotations',
      'outputSchema',
    ]);
  });

  it('unchecking a checked field removes it', () => {
    expect(toggleField('annotations', ['title', 'annotations'], false)).toEqual(['title']);
  });

  it('per-field: each optional field can be individually added from the default', () => {
    for (const f of TOOL_DEFINITION_FIELDS) {
      const next = toggleField(f, [...DEFAULT_TOOL_DEFINITION_FIELDS], true);
      expect(next).toContain(f);
      // Everything else unchanged.
      for (const other of DEFAULT_TOOL_DEFINITION_FIELDS) {
        if (other !== f) expect(next).toContain(other);
      }
    }
  });

  it('per-field: each optional field can be individually removed from all-six', () => {
    const all = [...TOOL_DEFINITION_FIELDS];
    for (const f of TOOL_DEFINITION_FIELDS) {
      const next = toggleField(f, all, false);
      expect(next).not.toContain(f);
      expect(next.length).toBe(all.length - 1);
    }
  });

  it('checking an already-checked field is idempotent (no duplicates)', () => {
    const next = toggleField('title', ['title', 'annotations'], true);
    expect(next).toEqual(['title', 'annotations']);
  });

  it('unchecking an unchecked field is idempotent', () => {
    const next = toggleField('icons', ['title'], false);
    expect(next).toEqual(['title']);
  });

  it('empty selection: checking from empty yields exactly that field', () => {
    expect(toggleField('_meta', [], true)).toEqual(['_meta']);
  });

  it('empty selection: unchecking from empty stays empty', () => {
    expect(toggleField('_meta', [], false)).toEqual([]);
  });

  it('order always follows TOOL_DEFINITION_FIELDS regardless of input order', () => {
    const scrambled = ['_meta', 'icons', 'execution', 'outputSchema', 'annotations', 'title'];
    const next = toggleField('icons', scrambled, false);
    expect(next).toEqual(['title', 'annotations', 'outputSchema', 'execution', '_meta']);
  });

  it('never mutates the input array', () => {
    const input = ['title', 'annotations'];
    const copy = [...input];
    toggleField('outputSchema', input, true);
    expect(input).toEqual(copy);
  });

  it('ignores unknown names present in the selected array (backend whitelist parity)', () => {
    const withJunk = ['title', 'bogusField'];
    const next = toggleField('title', withJunk, false);
    expect(next).toEqual([]);
  });
});
