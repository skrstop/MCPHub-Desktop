// @vitest-environment jsdom
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, waitFor, cleanup, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import React from 'react';

const t = (key: string) => key;
vi.mock('react-i18next', () => ({ useTranslation: () => ({ t }) }));

import SmartRoutingSettingsDialog from './SmartRoutingSettingsDialog';

const baseConfig = {
  enabled: true,
  dbUrl: '',
  embeddingDimensionsApiPassthrough: false,
  llmProviderBaseUrl: '',
  llmProviderApiKey: '',
  embeddingModel: '',
  progressiveDisclosure: false,
  vectorWeight: 0.5,
  keywordWeight: 0.5,
  maxResults: 20,
  scoreThreshold: 0.5,
};

afterEach(cleanup);

describe('SmartRoutingSettingsDialog', () => {
  it('renders the four retrieval settings with current values', () => {
    render(
      <SmartRoutingSettingsDialog
        onClose={() => {}}
        config={baseConfig}
        onSave={vi.fn()}
      />,
    );
    const numbers = screen.getAllByRole('spinbutton');
    const vals = numbers.map((n) => (n as HTMLInputElement).value);
    expect(vals).toContain('0.50'); // weights
    expect(vals).toContain('20'); // maxResults
    expect(vals).toContain('0.50'); // threshold
  });

  it('links the two weights so their sum stays 1', async () => {
    const user = userEvent.setup();
    render(
      <SmartRoutingSettingsDialog
        onClose={() => {}}
        config={baseConfig}
        onSave={vi.fn()}
      />,
    );
    const [vector, keyword] = screen.getAllByRole('spinbutton');
    // fireEvent with a complete value: char-by-char typing fights the
    // controlled toFixed(2) re-render (same as dragging the slider).
    fireEvent.change(vector, { target: { value: '0.8' } });
    await waitFor(() => {
      expect((vector as HTMLInputElement).value).toBe('0.80');
      expect((keyword as HTMLInputElement).value).toBe('0.20');
    });
  });

  it('saves the four values and closes on success', async () => {
    const user = userEvent.setup();
    const onSave = vi.fn().mockResolvedValue(true);
    const onClose = vi.fn();
    render(
      <SmartRoutingSettingsDialog
        onClose={onClose}
        config={baseConfig}
        onSave={onSave}
      />,
    );
    await user.click(screen.getByRole('button', { name: /common\.save/ }));
    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(1));
    const updates = onSave.mock.calls[0][0] as Record<string, number>;
    expect(updates.vectorWeight).toBe(0.5);
    expect(updates.keywordWeight).toBe(0.5);
    expect(updates.maxResults).toBe(20);
    expect(updates.scoreThreshold).toBe(0.5);
    await waitFor(() => expect(onClose).toHaveBeenCalled());
  });

  it('stays open when the save fails', async () => {
    const user = userEvent.setup();
    const onSave = vi.fn().mockResolvedValue(false);
    const onClose = vi.fn();
    render(
      <SmartRoutingSettingsDialog
        onClose={onClose}
        config={baseConfig}
        onSave={onSave}
      />,
    );
    await user.click(screen.getByRole('button', { name: /common\.save/ }));
    await waitFor(() => expect(onSave).toHaveBeenCalledTimes(1));
    expect(onClose).not.toHaveBeenCalled();
  });

  it('clamps maxResults into [1, 200]', async () => {
    const user = userEvent.setup();
    const onSave = vi.fn().mockResolvedValue(true);
    render(
      <SmartRoutingSettingsDialog
        onClose={() => {}}
        config={{ ...baseConfig, maxResults: 500 }}
        onSave={onSave}
      />,
    );
    await user.click(screen.getByRole('button', { name: /common\.save/ }));
    const updates = onSave.mock.calls[0][0] as Record<string, number>;
    expect(updates.maxResults).toBe(200);
  });
});
