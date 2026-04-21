import { useEffect, useRef } from 'react';
import { useToast } from '@/contexts/ToastContext';
import { logStreamManager } from '@/services/logService';

const EMBED_SYNC_ERROR_MARKER = '[EMBED_SYNC_ERROR]';
const TOAST_DURATION_MS = 8000;
const TOAST_DEDUPE_WINDOW_MS = 30000;

type EmbeddingSyncStreamPayload = {
  type?: unknown;
  log?: {
    message?: unknown;
  };
  progress?: {
    serverName?: unknown;
    status?: unknown;
  };
};

const EmbeddingSyncAlertListener = () => {
  const { showToast } = useToast();
  const recentToastRef = useRef<Map<string, number>>(new Map());

  useEffect(() => {
    const shouldSuppressToast = (key: string): boolean => {
      const now = Date.now();

      recentToastRef.current.forEach((timestamp, existingKey) => {
        if (now - timestamp >= TOAST_DEDUPE_WINDOW_MS) {
          recentToastRef.current.delete(existingKey);
        }
      });

      const previousTimestamp = recentToastRef.current.get(key);
      if (previousTimestamp && now - previousTimestamp < TOAST_DEDUPE_WINDOW_MS) {
        return true;
      }

      recentToastRef.current.set(key, now);
      return false;
    };

    const extractServerName = (message: string): string | null => {
      const serverMatch = message.match(/server\s+"([^"]+)"/i);
      return serverMatch?.[1] || null;
    };

    const showEmbeddingErrorToast = (key: string, message: string) => {
      if (shouldSuppressToast(key)) {
        return;
      }

      showToast(message, 'error', TOAST_DURATION_MS);
    };

    const handleEmbeddingSyncErrorLog = (rawMessage: string) => {
      const cleanedMessage = rawMessage.replace(EMBED_SYNC_ERROR_MARKER, '').trim();
      const serverName = extractServerName(cleanedMessage);

      if (cleanedMessage.includes('Full embeddings resync failed')) {
        showEmbeddingErrorToast(
          'embedding-resync-failed',
          'Embedding resynchronization failed. Check smart routing logs and provider limits.',
        );
        return;
      }

      if (serverName) {
        showEmbeddingErrorToast(
          `embedding-sync-error:${serverName}`,
          `Embedding synchronization failed for server "${serverName}". Check logs for details.`,
        );
        return;
      }

      showEmbeddingErrorToast(
        `embedding-sync-error:${cleanedMessage}`,
        cleanedMessage ||
          'Embedding synchronization failed. Check smart routing logs and provider rate limits.',
      );
    };

    const handleMessage = (event: MessageEvent) => {
      try {
        const data = JSON.parse(event.data) as EmbeddingSyncStreamPayload;

        if (data?.type === 'embedding-sync-progress') {
          const serverName =
            typeof data.progress?.serverName === 'string' ? data.progress.serverName : '';
          const status = typeof data.progress?.status === 'string' ? data.progress.status : '';

          if (status === 'error' && serverName) {
            showEmbeddingErrorToast(
              `embedding-sync-error:${serverName}`,
              `Embedding synchronization failed for server "${serverName}". Check logs for details.`,
            );
          }

          return;
        }

        if (data?.type !== 'log') return;

        const message = String(data?.log?.message || '');
        if (!message.includes(EMBED_SYNC_ERROR_MARKER)) return;

        handleEmbeddingSyncErrorLog(message);
      } catch {
        // Ignore malformed stream messages.
      }
    };

    return logStreamManager.subscribe(handleMessage);
  }, [showToast]);

  return null;
};

export default EmbeddingSyncAlertListener;
