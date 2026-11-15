// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, cleanup } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import React from 'react';

afterEach(cleanup);

const toolsMock = vi.fn();
vi.mock('@/services/ragService', async (importOriginal) => ({
  ...(await importOriginal()),
  getRagTools: (...args: unknown[]) => toolsMock(...args),
}));

import { ToolsDialog } from '@/pages/RagPage';

const t = (key: string) => key;

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t }),
}));

const tool = {
  name: 'rag_search',
  description: 'Search RAG documents by semantic similarity.',
  inputSchema: {
    type: 'object',
    properties: {
      query: { type: 'string', description: 'The search query.' },
      tags: { type: 'array', description: 'Optional tag filter.' },
    },
    required: ['query'],
  },
};

beforeEach(() => {
  toolsMock.mockReset();
  toolsMock.mockResolvedValue([]);
});

describe('RagPage ToolsDialog', () => {
  it('renders the tool name, description and required markers', async () => {
    toolsMock.mockResolvedValue([tool]);
    render(<ToolsDialog onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText('rag_search')).toBeTruthy());
    expect(screen.getByText('Search RAG documents by semantic similarity.')).toBeTruthy();
    expect(screen.getByText('query', { exact: true }).textContent).toBe('query*');
    expect(screen.getByText('tags', { exact: true }).textContent).toBe('tags');
  });

  it('shows the empty state when the backend returns no tools', async () => {
    render(<ToolsDialog onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText('pages.rag.toolsEmpty')).toBeTruthy());
  });

  it('shows the disabled hint when RAG is known-off (catalog still visible)', async () => {
    toolsMock.mockResolvedValue([tool]);
    render(<ToolsDialog onClose={() => {}} enabled={false} />);
    await waitFor(() => expect(screen.getByText('rag_search')).toBeTruthy());
    expect(screen.getByText('pages.rag.toolsDisabledHint')).toBeTruthy();
  });

  it('shows no disabled hint when the flag is unknown (null) or omitted', async () => {
    toolsMock.mockResolvedValue([tool]);
    const { unmount } = render(<ToolsDialog onClose={() => {}} enabled={null} />);
    await waitFor(() => expect(screen.getByText('rag_search')).toBeTruthy());
    expect(screen.queryByText('pages.rag.toolsDisabledHint')).toBeNull();
    unmount();
    render(<ToolsDialog onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText('rag_search')).toBeTruthy());
    expect(screen.queryByText('pages.rag.toolsDisabledHint')).toBeNull();
  });

  it('surfaces fetch errors and closes via the footer button', async () => {
    const user = userEvent.setup();
    toolsMock.mockRejectedValue(new Error('backend down'));
    const onClose = vi.fn();
    render(<ToolsDialog onClose={onClose} />);
    await waitFor(() => expect(screen.getByText('backend down')).toBeTruthy());
    await user.click(screen.getByRole('button', { name: 'pages.rag.close' }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
