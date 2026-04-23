import React from 'react';
import { BrowserRouter as Router, Route, Routes, Navigate, useParams } from 'react-router-dom';
import { AuthProvider } from './contexts/AuthContext';
import { ToastProvider } from './contexts/ToastContext';
import { ThemeProvider } from './contexts/ThemeContext';
import { ServerProvider } from './contexts/ServerContext';
import { SettingsProvider } from './contexts/SettingsContext';
import MainLayout from './layouts/MainLayout';
import ProtectedRoute from './components/ProtectedRoute';
import LoginPage from './pages/LoginPage';

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
import DashboardPage from './pages/Dashboard';
import ServersPage from './pages/ServersPage';
import GroupsPage from './pages/GroupsPage';
import UsersPage from './pages/UsersPage';
import SettingsPage from './pages/SettingsPage';
import MarketPage from './pages/MarketPage';
import LogsPage from './pages/LogsPage';
import ActivityPage from './pages/ActivityPage';
import PromptsPage from './pages/PromptsPage';
import ResourcesPage from './pages/ResourcesPage';
import EmbeddingSyncAlertListener from './components/EmbeddingSyncAlertListener';
import { getBasePath } from './utils/runtime';

// Helper component to redirect cloud server routes to market
const CloudRedirect: React.FC = () => {
  const { serverName } = useParams<{ serverName: string }>();
  return <Navigate to={`/market/${serverName}?tab=cloud`} replace />;
};

function App() {
  const basename = getBasePath();
  return (
    <AppErrorBoundary>
    <ThemeProvider>
      <AuthProvider>
        <ServerProvider>
          <ToastProvider>
            <SettingsProvider>
              <Router basename={basename}>
                <EmbeddingSyncAlertListener />
                <Routes>
                  {/* 公共路由 */}
                  <Route path="/login" element={<LoginPage />} />

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
        </ServerProvider>
      </AuthProvider>
    </ThemeProvider>
    </AppErrorBoundary>
  );
}

export default App;
