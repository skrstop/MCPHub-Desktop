// Generic client-side list filtering + pagination for simple enabled/disabled items.
// Mirrors the shape of serverFilters.ts but is decoupled from Server so the
// Prompts and Resources pages can share one implementation.

export type ItemFilter = 'all' | 'active' | 'inactive';

export interface ItemPageInfo {
  page: number;
  limit: number;
  total: number;
  totalPages: number;
  hasNextPage: boolean;
  hasPrevPage: boolean;
}

export interface ItemFilterCounts {
  all: number;
  active: number;
  inactive: number;
}

export const getItemFilterCounts = <T>(items: T[], isEnabled: (item: T) => boolean): ItemFilterCounts => {
  let active = 0;
  for (const item of items) {
    if (isEnabled(item)) active += 1;
  }
  return {
    all: items.length,
    active,
    inactive: items.length - active,
  };
};

export const selectItemPage = <T>(
  items: T[],
  filter: ItemFilter,
  search: string,
  page: number,
  limit: number,
  opts: { haystack: (item: T) => string; isEnabled: (item: T) => boolean },
): { items: T[]; pagination: ItemPageInfo } => {
  const query = search.trim().toLowerCase();

  const filtered = items.filter((item) => {
    if (filter === 'active' && !opts.isEnabled(item)) return false;
    if (filter === 'inactive' && opts.isEnabled(item)) return false;
    if (!query) return true;
    return opts.haystack(item).toLowerCase().includes(query);
  });

  const total = filtered.length;
  const totalPages = Math.max(1, Math.ceil(total / limit));
  const safePage = Math.min(Math.max(1, page), totalPages);
  const start = (safePage - 1) * limit;

  return {
    items: filtered.slice(start, start + limit),
    pagination: {
      page: safePage,
      limit,
      total,
      totalPages,
      hasNextPage: safePage < totalPages,
      hasPrevPage: safePage > 1,
    },
  };
};
