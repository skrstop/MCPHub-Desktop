/**
 * Tool-definition-field metadata for the Smart Routing settings UI
 * (origin #1286). Extracted as a pure module so the per-field tips,
 * core/optional split and checkbox behavior are unit-testable.
 *
 * Core four fields are ALWAYS carried by every search/describe result
 * (origin: "next to the name, description, inputSchema and serverName
 * they always carry") — shown checked and disabled in the UI.
 * Optional fields are user-configurable; the backend whitelist must
 * stay in sync (smart_routing/models.rs ALLOWED).
 */

export const CORE_TOOL_FIELDS = ['name', 'description', 'inputSchema', 'serverName'] as const;

export const TOOL_DEFINITION_FIELDS = [
  'title',
  'annotations',
  'outputSchema',
  'execution',
  'icons',
  '_meta',
] as const;

export type ToolDefinitionField = (typeof TOOL_DEFINITION_FIELDS)[number];

export const DEFAULT_TOOL_DEFINITION_FIELDS: readonly ToolDefinitionField[] = [
  'title',
  'annotations',
];

/** i18n key suffix for a field's tooltip (settings.field<InputSchema> etc.).
 *  `_meta` maps to fieldMeta (leading underscore is not a valid i18n segment). */
export const fieldTipKey = (field: string): string => {
  const suffix = field === '_meta' ? 'Meta' : `${field.charAt(0).toUpperCase()}${field.slice(1)}`;
  return `settings.field${suffix}`;
};

/** All tip keys (core + optional) that must exist in every locale. */
export const allFieldTipKeys = (): string[] =>
  [...CORE_TOOL_FIELDS, ...TOOL_DEFINITION_FIELDS].map(fieldTipKey);

/**
 * Next selection state after toggling `field` in `selected`.
 * Pure: never mutates inputs; preserves TOOL_DEFINITION_FIELDS order.
 */
export const toggleField = (
  field: ToolDefinitionField,
  selected: readonly string[],
  checked: boolean,
): ToolDefinitionField[] =>
  TOOL_DEFINITION_FIELDS.filter(
    (name) => (name === field ? checked : selected.includes(name)),
  );
