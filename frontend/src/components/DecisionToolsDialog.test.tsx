// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor, cleanup } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import React from 'react';

afterEach(cleanup);

const toolsMock = vi.fn();
vi.mock('@/services/decisionService', () => ({
  getDecisionTools: (...args: unknown[]) => toolsMock(...args),
}));

import { ToolsDialog } from '@/pages/DecisionPage';

const t = (key: string) => key;

vi.mock('react-i18next', () => ({
  useTranslation: () => ({ t }),
}));

const tool = {
  name: 'decision_decide',
  description: 'Run a structured decision with the local decision model.',
  inputSchema: {
    type: 'object',
    properties: {
      decisionType: { type: 'string', description: 'choice/score/noul' },
      state: { type: 'string', description: 'context' },
      question: { type: 'string', description: 'the question' },
      options: { type: 'array', description: '2-20 candidates' },
    },
    required: ['state', 'question', 'options'],
  },
};

beforeEach(() => {
  toolsMock.mockReset();
  toolsMock.mockResolvedValue([]);
});

describe('DecisionPage ToolsDialog', () => {
  it('renders the tool name, description and schema properties with required markers', async () => {
    toolsMock.mockResolvedValue([tool]);
    render(<ToolsDialog onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText('decision_decide')).toBeTruthy());
    expect(
      screen.getByText('Run a structured decision with the local decision model.'),
    ).toBeTruthy();
    // All four properties listed; required ones carry a trailing "*" marker,
    // the optional decisionType does not.
    expect(screen.getByText('decisionType', { exact: true }).textContent).toBe('decisionType');
    expect(screen.getByText('state', { exact: true }).textContent).toBe('state*');
    expect(screen.getByText('question', { exact: true }).textContent).toBe('question*');
    expect(screen.getByText('options', { exact: true }).textContent).toBe('options*');
  });

  it('shows the empty state when the backend returns no tools', async () => {
    render(<ToolsDialog onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText('pages.decision.toolsEmpty')).toBeTruthy());
    expect(toolsMock).toHaveBeenCalledTimes(1);
  });

  it('shows the disabled hint when decision support is off (catalog still visible)', async () => {
    toolsMock.mockResolvedValue([tool]);
    render(<ToolsDialog onClose={() => {}} enabled={false} />);
    await waitFor(() => expect(screen.getByText('decision_decide')).toBeTruthy());
    expect(screen.getByText('pages.decision.toolsDisabledHint')).toBeTruthy();
  });

  it('shows no disabled hint when the enabled prop is omitted', async () => {
    toolsMock.mockResolvedValue([tool]);
    render(<ToolsDialog onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText('decision_decide')).toBeTruthy());
    expect(screen.queryByText('pages.decision.toolsDisabledHint')).toBeNull();
  });

  it('shows no disabled hint while the enable flag is unknown (null)', async () => {
    toolsMock.mockResolvedValue([tool]);
    render(<ToolsDialog onClose={() => {}} enabled={null} />);
    await waitFor(() => expect(screen.getByText('decision_decide')).toBeTruthy());
    expect(screen.queryByText('pages.decision.toolsDisabledHint')).toBeNull();
  });

  it('shows the error message when the fetch fails', async () => {
    toolsMock.mockRejectedValue(new Error('backend down'));
    render(<ToolsDialog onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText('backend down')).toBeTruthy());
  });

  it('closes via the footer Close button and the header X button', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    const { container } = render(<ToolsDialog onClose={onClose} />);
    await waitFor(() => expect(screen.getByText('pages.decision.toolsEmpty')).toBeTruthy());
    await user.click(screen.getByRole('button', { name: 'common.close' }));
    expect(onClose).toHaveBeenCalledTimes(1);
    // Header X button (hub-icon-btn) also closes.
    const x = container.querySelector('button.hub-icon-btn') as HTMLButtonElement;
    expect(x).toBeTruthy();
    await user.click(x);
    expect(onClose).toHaveBeenCalledTimes(2);
  });

  it('renders the resolved tools when the fetch settles before unmount', async () => {
    let resolve!: (v: unknown) => void;
    toolsMock.mockReturnValue(
      new Promise((res) => {
        resolve = res;
      }),
    );
    const { unmount } = render(<ToolsDialog onClose={() => {}} />);
    await waitFor(() => expect(screen.getByText('common.loading')).toBeTruthy());
    resolve([tool]);
    await waitFor(() => expect(screen.getByText('decision_decide')).toBeTruthy());
    unmount();
  });
});
