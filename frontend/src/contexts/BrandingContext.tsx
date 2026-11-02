import React, { createContext, useContext, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { getPublicConfig } from '../services/configService';

interface BrandingContextValue {
  dashboardName: string;
  dashboardTitle: string;
  setDashboardName: (name: string) => void;
}

const BrandingContext = createContext<BrandingContextValue | null>(null);

export const BrandingProvider: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const { t } = useTranslation();
  const [dashboardName, setDashboardName] = useState('');
  const dashboardTitle = dashboardName || t('app.title');

  useEffect(() => {
    let active = true;
    void getPublicConfig().then((config) => {
      if (active) setDashboardName(config.dashboardName?.trim() || '');
    });
    return () => {
      active = false;
    };
  }, []);

  useEffect(() => {
    // Reuse dashboardTitle so the fallback matches the in-app title (i18n
    // aware) — a hardcoded English string showed "MCPHub Dashboard" on the
    // window tab for non-English users with no custom name set.
    document.title = dashboardTitle || 'MCPHub Dashboard';
  }, [dashboardTitle]);

  return (
    <BrandingContext.Provider value={{ dashboardName, dashboardTitle, setDashboardName }}>
      {children}
    </BrandingContext.Provider>
  );
};

export const useBranding = () => {
  const context = useContext(BrandingContext);
  if (!context) throw new Error('useBranding must be used within a BrandingProvider');
  return context;
};
