// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, act, waitFor, cleanup } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import React from 'react';
import { afterEach } from 'vitest';

afterEach(cleanup);

const searchMock = vi.fn();
const perfMock = vi.fn();
vi.mock('@/services/smartRoutingService', () => ({
  searchSmartRouting: (...args: unknown[]) => searchMock(...args),
  fetchSmartRoutingPerformance: (...args: unknown[]) => perfMock(...args),
}));

import SmartSearchDialog from './SmartSearchDialog';

const t = (key: string, opts?: Record<string, unknown>) =>
  opts && 'count' in opts ? `${key}[${opts.count}]` : key;

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t }),
}));

const scopes = [
  { value: 'all', label: 'All', kind: 'all' as const },
  { value: 'group:g1', label: 'Group: web', kind: 'group' as const },
  { value: 'server:pw', label: 'Server: pw', kind: 'server' as const },
];

const deferred = () => {
  let resolve!: (v: unknown) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
};

beforeEach(() => {
  searchMock.mockReset();
  searchMock.mockResolvedValue([]);
  perfMock.mockReset();
  perfMock.mockResolvedValue({ vectorStore: { totalRows: 42, toolRows: 40 } });
});

describe('SmartSearchDialog', () => {
  it('renders scope options and disables search for a blank query', async () => {
    const user = userEvent.setup();
    render(<SmartSearchDialog onClose={() => {}} scopeOptions={scopes} />);
    const btn = screen.getByRole('button', { name: /searchBtn/ });
    expect((btn as HTMLButtonElement).disabled).toBe(true);
    await user.type(screen.getByRole('textbox'), 'deploy');
    expect((btn as HTMLButtonElement).disabled).toBe(false);
  });

  it('sends the scope verbatim and renders scored results', async () => {
    const user = userEvent.setup();
    searchMock.mockResolvedValue([
      {
        serverName: 'pw',
        toolName: 'click',
        contentId: 'c1',
        contentType: 'tool',
        textContent: null,
        metadata: { description: 'desc' },
        score: 1.42,
      },
    ]);
    render(<SmartSearchDialog onClose={() => {}} scopeOptions={scopes} />);
    await user.selectOptions(screen.getByRole('combobox'), 'group:g1');
    await user.type(screen.getByRole('textbox'), 'deploy');
    await user.click(screen.getByRole('button', { name: /searchBtn/ }));
    await waitFor(() => expect(screen.getByText(/click/)).toBeTruthy());
    expect(searchMock).toHaveBeenCalledWith('deploy', 'group:g1', 20);
    expect(screen.getByText(/1\.42/)).toBeTruthy();
  });

  it('discards a stale response after the scope changed mid-flight', async () => {
    const user = userEvent.setup();
    const d = deferred();
    searchMock.mockImplementation(() => d.promise);
    render(<SmartSearchDialog onClose={() => {}} scopeOptions={scopes} />);
    await user.type(screen.getByRole('textbox'), 'deploy');
    await user.click(screen.getByRole('button', { name: /searchBtn/ }));
    // Change scope while the request is in flight, then resolve the old response.
    await user.selectOptions(screen.getByRole('combobox'), 'server:pw');
    await act(async () => {
      d.resolve([
        {
          serverName: 'pw',
          toolName: 'stale-tool',
          contentId: 'c1',
          contentType: 'tool',
          textContent: null,
          metadata: {},
          score: 1,
        },
      ]);
    });
    await waitFor(() => expect(screen.queryByText(/stale-tool/)).toBeNull());
    // The stale response must not flip the dialog into a results state.
    expect(screen.queryByText(/server\.smartSearch\.results/)).toBeNull();
  });

  it('resets a dangling scope back to all when its option disappears', async () => {
    const user = userEvent.setup();
    const { rerender } = render(
      <SmartSearchDialog onClose={() => {}} scopeOptions={scopes} />,
    );
    await user.selectOptions(screen.getByRole('combobox'), 'group:g1');
    // The group is deleted while the dialog is open.
    rerender(
      <SmartSearchDialog
        onClose={() => {}}
        scopeOptions={[scopes[0], scopes[2]]}
      />,
    );
    await waitFor(() =>
      expect((screen.getByRole('combobox') as HTMLSelectElement).value).toBe('all'),
    );
  });

  it('localizes known backend errors and shows the error card', async () => {
    const user = userEvent.setup();
    searchMock.mockRejectedValue(new Error('mv runtime not running'));
    render(<SmartSearchDialog onClose={() => {}} scopeOptions={scopes} />);
    await user.type(screen.getByRole('textbox'), 'deploy');
    await user.click(screen.getByRole('button', { name: /searchBtn/ }));
    await waitFor(() =>
      expect(screen.getByText('server.smartSearch.errMvNotRunning')).toBeTruthy(),
    );
  });

  it('shows the empty-state card when the search returns nothing', async () => {
    const user = userEvent.setup();
    // Explicit (not the beforeEach default): this test pins "index has rows
    // → generic hint" and would catch a default flipping to toolRows: 0.
    perfMock.mockResolvedValue({ vectorStore: { totalRows: 42, toolRows: 40 } });
    render(<SmartSearchDialog onClose={() => {}} scopeOptions={scopes} />);
    await user.type(screen.getByRole('textbox'), 'deploy');
    await user.click(screen.getByRole('button', { name: /searchBtn/ }));
    await waitFor(() =>
      expect(screen.getByText('server.smartSearch.noResults')).toBeTruthy(),
    );
  });

  it('restores focus to the trigger on close', async () => {
    const trigger = document.createElement('button');
    document.body.appendChild(trigger);
    trigger.focus();
    expect(document.activeElement).toBe(trigger);
    const { unmount } = render(
      <SmartSearchDialog onClose={() => {}} scopeOptions={scopes} />,
    );
    // Focus moved into the dialog's query input on open.
    const input = screen.getByRole('textbox');
    expect(document.activeElement).toBe(input);
    unmount();
    expect(document.activeElement).toBe(trigger);
    trigger.remove();
  });

  it('shows the index-empty hint when the vector store has no rows', async () => {
    const user = userEvent.setup();
    searchMock.mockResolvedValue([]);
    perfMock.mockResolvedValue({ vectorStore: { totalRows: 0, toolRows: 0 } });
    render(<SmartSearchDialog onClose={() => {}} scopeOptions={scopes} />);
    await user.type(screen.getByRole('textbox'), 'deploy');
    await user.click(screen.getByRole('button', { name: /searchBtn/ }));
    await waitFor(() =>
      expect(screen.getByText('server.smartSearch.indexEmpty')).toBeTruthy(),
    );
  });

  it('keeps the generic no-results hint when the index has rows', async () => {
    const user = userEvent.setup();
    searchMock.mockResolvedValue([]);
    perfMock.mockResolvedValue({ vectorStore: { totalRows: 42, toolRows: 40 } });
    render(<SmartSearchDialog onClose={() => {}} scopeOptions={scopes} />);
    await user.type(screen.getByRole('textbox'), 'deploy');
    await user.click(screen.getByRole('button', { name: /searchBtn/ }));
    await waitFor(() =>
      expect(screen.getByText('server.smartSearch.noResults')).toBeTruthy(),
    );
    expect(screen.queryByText('server.smartSearch.indexEmpty')).toBeNull();
  });

  it('falls back to the generic hint when the status probe fails', async () => {
    const user = userEvent.setup();
    searchMock.mockResolvedValue([]);
    perfMock.mockRejectedValue(new Error('probe down'));
    render(<SmartSearchDialog onClose={() => {}} scopeOptions={scopes} />);
    await user.type(screen.getByRole('textbox'), 'deploy');
    await user.click(screen.getByRole('button', { name: /searchBtn/ }));
    await waitFor(() =>
      expect(screen.getByText('server.smartSearch.noResults')).toBeTruthy(),
    );
    expect(screen.queryByText('server.smartSearch.indexEmpty')).toBeNull();
  });

  it('clamps the request limit and hint count to the maxResults prop', async () => {
    const user = userEvent.setup();
    const rows = Array.from({ length: 5 }, (_, i) => ({
      serverName: 's',
      toolName: `tool-${i}`,
      contentId: `c${i}`,
      contentType: 'tool',
      textContent: null,
      metadata: {},
      score: 1,
    }));
    searchMock.mockResolvedValue(rows);
    render(
      <SmartSearchDialog onClose={() => {}} scopeOptions={scopes} maxResults={5} />,
    );
    await user.type(screen.getByRole('textbox'), 'deploy');
    await user.click(screen.getByRole('button', { name: /searchBtn/ }));
    await waitFor(() => expect(screen.getByText(/tool-4/)).toBeTruthy());
    expect(searchMock).toHaveBeenCalledWith('deploy', 'all', 5);
    expect(screen.getByText('server.smartSearch.resultsTruncated[5]')).toBeTruthy();
  });

  it('does not search on Enter with a whitespace-only query', async () => {
    const user = userEvent.setup();
    render(<SmartSearchDialog onClose={() => {}} scopeOptions={scopes} />);
    await user.type(screen.getByRole('textbox'), '   ');
    await user.keyboard('{Enter}');
    expect(searchMock).not.toHaveBeenCalled();
  });

  it('searches on Enter with a valid query', async () => {
    const user = userEvent.setup();
    render(<SmartSearchDialog onClose={() => {}} scopeOptions={scopes} />);
    await user.type(screen.getByRole('textbox'), 'deploy');
    await user.keyboard('{Enter}');
    await waitFor(() => expect(searchMock).toHaveBeenCalledWith('deploy', 'all', 20));
  });

  it('falls back to textContent when metadata has no description', async () => {
    const user = userEvent.setup();
    searchMock.mockResolvedValue([
      {
        serverName: 'pw',
        toolName: 'click',
        contentId: 'c1',
        contentType: 'tool',
        textContent: 'fallback text',
        metadata: {},
        score: 1,
      },
    ]);
    render(<SmartSearchDialog onClose={() => {}} scopeOptions={scopes} />);
    await user.type(screen.getByRole('textbox'), 'deploy');
    await user.click(screen.getByRole('button', { name: /searchBtn/ }));
    await waitFor(() => expect(screen.getByText('fallback text')).toBeTruthy());
  });

  it('clears a stale index-empty flag when a later search errors', async () => {
    const user = userEvent.setup();
    searchMock.mockResolvedValueOnce([]);
    perfMock.mockResolvedValueOnce({ vectorStore: { totalRows: 0, toolRows: 0 } });
    render(<SmartSearchDialog onClose={() => {}} scopeOptions={scopes} />);
    await user.type(screen.getByRole('textbox'), 'deploy');
    await user.click(screen.getByRole('button', { name: /searchBtn/ }));
    await waitFor(() =>
      expect(screen.getByText('server.smartSearch.indexEmpty')).toBeTruthy(),
    );
    // Second search fails — the index-empty hint must not survive the error.
    searchMock.mockRejectedValueOnce(new Error('boom'));
    await user.type(screen.getByRole('textbox'), ' again');
    await user.click(screen.getByRole('button', { name: /searchBtn/ }));
    await waitFor(() =>
      expect(screen.getByText('boom')).toBeTruthy(),
    );
    expect(screen.queryByText('server.smartSearch.indexEmpty')).toBeNull();
  });

  it('discards a stale error after the scope changed mid-flight', async () => {
    const user = userEvent.setup();
    const d = deferred();
    searchMock.mockImplementation(() => d.promise);
    render(<SmartSearchDialog onClose={() => {}} scopeOptions={scopes} />);
    await user.type(screen.getByRole('textbox'), 'deploy');
    await user.click(screen.getByRole('button', { name: /searchBtn/ }));
    await user.selectOptions(screen.getByRole('combobox'), 'server:pw');
    await act(async () => {
      d.reject(new Error('late failure'));
    });
    await waitFor(() =>
      expect(screen.queryByText('late failure')).toBeNull(),
    );
  });

  it('renders non-Error rejections via String()', async () => {
    const user = userEvent.setup();
    searchMock.mockRejectedValue('plain string failure');
    render(<SmartSearchDialog onClose={() => {}} scopeOptions={scopes} />);
    await user.type(screen.getByRole('textbox'), 'deploy');
    await user.click(screen.getByRole('button', { name: /searchBtn/ }));
    await waitFor(() => expect(screen.getByText('plain string failure')).toBeTruthy());
  });

  it('shows the truncation hint when the result count hits the limit', async () => {
    const user = userEvent.setup();
    const rows = Array.from({ length: 20 }, (_, i) => ({
      serverName: 's',
      toolName: `tool-${i}`,
      contentId: `c${i}`,
      contentType: 'tool',
      textContent: null,
      metadata: {},
      score: 1,
    }));
    searchMock.mockResolvedValue(rows);
    render(<SmartSearchDialog onClose={() => {}} scopeOptions={scopes} />);
    await user.type(screen.getByRole('textbox'), 'deploy');
    await user.click(screen.getByRole('button', { name: /searchBtn/ }));
    await waitFor(() =>
      expect(screen.getByText('server.smartSearch.resultsTruncated[20]')).toBeTruthy(),
    );
  });

  it('Escape closes the dialog', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<SmartSearchDialog onClose={onClose} scopeOptions={scopes} />);
    await user.keyboard('{Escape}');
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
