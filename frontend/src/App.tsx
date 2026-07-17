import React, { Suspense, lazy } from 'react';
import { BrowserRouter as Router, Route, Routes, Navigate, useParams } from 'react-router-dom';
import { AuthProvider } from './contexts/AuthContext';
import { ToastProvider } from './contexts/ToastContext';
import { ThemeProvider } from './contexts/ThemeContext';
import { ServerProvider } from './contexts/ServerContext';
import { ServerInstallProgressProvider } from './contexts/ServerInstallProgressContext';
import { SettingsProvider } from './contexts/SettingsContext';
import MainLayout from './layouts/MainLayout';
import ProtectedRoute from './components/ProtectedRoute';
import { UpdateCheckProvider } from './contexts/UpdateCheckContext';

class AppErrorBoundary extends React.Component<
  { children: React.ReactNode },
  { hasError: boolean; error: unknown }
> {
  constructor(props: { children: React.ReactNode }) {
    super(props);
    this.state = { hasError: false, error: null };
  }
  static getDerivedStateFromError(error: unknown) {
    return { hasError: true, error };
  }
  componentDidCatch(error: unknown, info: React.ErrorInfo) {
    console.error('[AppErrorBoundary] Caught render error:', error, info);
  }
  render() {
    if (this.state.hasError) {
      return (
        <div style={{ padding: 32, color: '#fff', background: '#111827', minHeight: '100vh' }}>
          <h2>Something went wrong</h2>
          <pre style={{ whiteSpace: 'pre-wrap', fontSize: 12, opacity: 0.7 }}>
            {String(this.state.error)}
          </pre>
        </div>
      );
    }
    return this.props.children;
  }
}
import EmbeddingSyncAlertListener from './components/EmbeddingSyncAlertListener';
import { getBasePath } from './utils/runtime';

const LoginPage = lazy(() => import('./pages/LoginPage'));
const DashboardPage = lazy(() => import('./pages/Dashboard'));
const ServersPage = lazy(() => import('./pages/ServersPage'));
const GroupsPage = lazy(() => import('./pages/GroupsPage'));
const UsersPage = lazy(() => import('./pages/UsersPage'));
const SettingsPage = lazy(() => import('./pages/SettingsPage'));
const MarketPage = lazy(() => import('./pages/MarketPage'));
const LogsPage = lazy(() => import('./pages/LogsPage'));
const ActivityPage = lazy(() => import('./pages/ActivityPage'));
const PromptsPage = lazy(() => import('./pages/PromptsPage'));
const ResourcesPage = lazy(() => import('./pages/ResourcesPage'));

// Helper component to redirect cloud server routes to market
const CloudRedirect: React.FC = () => {
  const { serverName } = useParams<{ serverName: string }>();
  return <Navigate to={`/market/${serverName}?tab=cloud`} replace />;
};

const RouteFallback: React.FC = () => (
  <div className="flex min-h-screen items-center justify-center text-sm text-gray-500">
    Loading...
  </div>
);

function App() {
  const basename = getBasePath();
  return (
    <AppErrorBoundary>
    <ThemeProvider>
      <AuthProvider>
        <UpdateCheckProvider>
        <ServerProvider>
          <ServerInstallProgressProvider>
          <ToastProvider>
            <SettingsProvider>
              <Router basename={basename}>
                <EmbeddingSyncAlertListener />
                <Routes>
                  {/* 公共路由 */}
                  <Route
                    path="/login"
                    element={
                      <Suspense fallback={<RouteFallback />}>
                        <LoginPage />
                      </Suspense>
                    }
                  />

                  {/* 受保护的路由，使用 MainLayout 作为布局容器 */}
                  <Route element={<ProtectedRoute />}>
                    <Route element={<MainLayout />}>
                      <Route path="/" element={<DashboardPage />} />
                      <Route path="/servers" element={<ServersPage />} />
                      <Route path="/groups" element={<GroupsPage />} />
                      <Route path="/prompts" element={<PromptsPage />} />
                      <Route path="/resources" element={<ResourcesPage />} />
                      <Route path="/users" element={<UsersPage />} />
                      <Route path="/market" element={<MarketPage />} />
                      <Route path="/market/:serverName" element={<MarketPage />} />
                      {/* Legacy cloud routes redirect to market with cloud tab */}
                      <Route path="/cloud" element={<Navigate to="/market?tab=cloud" replace />} />
                      <Route path="/cloud/:serverName" element={<CloudRedirect />} />
                      <Route path="/logs" element={<LogsPage />} />
                      <Route path="/activity" element={<ActivityPage />} />
                      <Route path="/settings" element={<SettingsPage />} />
                    </Route>
                  </Route>

                  {/* 未匹配的路由重定向到首页 */}
                  <Route path="*" element={<Navigate to="/" />} />
                </Routes>
              </Router>
            </SettingsProvider>
          </ToastProvider>
          </ServerInstallProgressProvider>
        </ServerProvider>
        </UpdateCheckProvider>
      </AuthProvider>
    </ThemeProvider>
    </AppErrorBoundary>
  );
}

export default App;
