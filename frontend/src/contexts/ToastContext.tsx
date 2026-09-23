import React, { createContext, useContext, useState, ReactNode, useCallback, useRef, useEffect } from 'react';
import ToastStack, { ToastData, ToastType } from '@/components/ui/Toast';

interface ToastContextProps {
  showToast: (message: string, type?: ToastType, duration?: number) => void;
}

const ToastContext = createContext<ToastContextProps | undefined>(undefined);

export const useToast = () => {
  const context = useContext(ToastContext);
  if (!context) {
    throw new Error('useToast must be used within a ToastProvider');
  }
  return context;
};

interface ToastProviderProps {
  children: ReactNode;
}

const MAX_TOASTS = 6;

/**
 * Toast provider with a stacked queue: toasts no longer replace each other —
 * batch operations (e.g. per-file import failures) keep every message visible
 * until its own timer expires. Errors/warnings stay longer (8s) than
 * success/info (4s); explicit `duration` still wins.
 */
export const ToastProvider: React.FC<ToastProviderProps> = ({ children }) => {
  const [toasts, setToasts] = useState<ToastData[]>([]);
  const timers = useRef<Map<number, ReturnType<typeof setTimeout>>>(new Map());

  const removeToast = useCallback((id: number) => {
    const timer = timers.current.get(id);
    if (timer) {
      clearTimeout(timer);
      timers.current.delete(id);
    }
    setToasts((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const showToast = useCallback(
    (message: string, type: ToastType = 'info', duration?: number) => {
      const ttl = duration ?? (type === 'error' || type === 'warning' ? 8000 : 4000);
      const id = Date.now() + Math.random();
      setToasts((prev) => {
        const next = [...prev, { id, message, type }];
        // Hard cap so a big batch can't flood the screen: drop oldest first.
        return next.length > MAX_TOASTS ? next.slice(next.length - MAX_TOASTS) : next;
      });
      timers.current.set(
        id,
        setTimeout(() => removeToast(id), ttl),
      );
    },
    [removeToast],
  );

  useEffect(() => {
    const pending = timers.current;
    return () => pending.forEach((t) => clearTimeout(t));
  }, []);

  return (
    <ToastContext.Provider value={{ showToast }}>
      {children}
      <ToastStack toasts={toasts} />
    </ToastContext.Provider>
  );
};
