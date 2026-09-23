import React from 'react';
import { Check, X, Info, AlertTriangle } from 'lucide-react';
import { cn } from '@/utils/cn';

export type ToastType = 'success' | 'error' | 'info' | 'warning';

export interface ToastData {
  id: number;
  message: string;
  type: ToastType;
}

const ICONS: Record<ToastType, React.ReactNode> = {
  success: <Check className="w-5 h-5 text-green-500" />,
  error: <X className="w-5 h-5 text-red-500" />,
  info: <Info className="w-5 h-5 text-blue-500" />,
  warning: <AlertTriangle className="w-5 h-5 text-yellow-500" />,
};

const BG: Record<ToastType, string> = {
  success: 'bg-green-50 border-green-200 dark:bg-green-950/60 dark:border-green-800',
  error: 'bg-red-50 border-red-200 dark:bg-red-950/60 dark:border-red-800',
  info: 'bg-blue-50 border-blue-200 dark:bg-blue-950/60 dark:border-blue-800',
  warning: 'bg-yellow-50 border-yellow-200 dark:bg-yellow-950/60 dark:border-yellow-800',
};

const ACCENT: Record<ToastType, string> = {
  success: 'border-l-green-500',
  error: 'border-l-red-600',
  info: 'border-l-blue-500',
  warning: 'border-l-yellow-500',
};

const TEXT: Record<ToastType, string> = {
  success: 'text-green-800 dark:text-green-200',
  error: 'text-red-800 dark:text-red-200',
  info: 'text-blue-800 dark:text-blue-200',
  warning: 'text-yellow-800 dark:text-yellow-200',
};

/**
 * Stacked toast container: renders all active toasts in a vertical column at
 * the top-right, above every dialog (z-[100]). The parent (ToastContext) owns
 * timers and removal; entries appear with a slide-in animation.
 */
const ToastStack: React.FC<{ toasts: ToastData[] }> = ({ toasts }) => {
  return (
    <div className="fixed top-4 right-4 z-[100] flex flex-col gap-2 items-end pointer-events-none">
      {toasts.map((t) => (
        <div
          key={t.id}
          className={cn(
            'pointer-events-auto max-w-sm p-4 rounded-md shadow-lg border border-l-4',
            BG[t.type],
            ACCENT[t.type],
            TEXT[t.type],
            'animate-toast-in',
          )}
        >
          <div className="flex items-start">
            <div className="flex-shrink-0">{ICONS[t.type]}</div>
            <p className="ml-3 text-sm font-medium whitespace-pre-wrap break-words">{t.message}</p>
          </div>
        </div>
      ))}
    </div>
  );
};

export default ToastStack;
