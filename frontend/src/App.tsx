import { useEffect } from 'react';
import type { ReactNode } from 'react';
import { Navigate, Outlet, Route, Routes, useLocation } from 'react-router-dom';

import { GlobalBackground } from './components/Background';
import { LoadingScreen, Toaster } from './components/ui';
import { AppLayout } from './components/AppLayout';
import { TransferDrawer } from './components/TransferDrawer';
import { DEFAULT_INTERFACE, applyAccent } from './lib/appearance';
import { AppearanceSettingsPage } from './pages/AppearanceSettingsPage';
import { AiWorkspacePage } from './pages/AiWorkspacePage';
import { AiAdminPage } from './pages/AiAdminPage';
import { AuditPage } from './pages/AuditPage';
import { BackgroundSettingsPage } from './pages/BackgroundSettingsPage';
import { CloudStoragePage } from './pages/cloud/CloudStoragePage';
import { DevelopersPage } from './pages/DevelopersPage';
import { HomePage } from './pages/HomePage';
import { JobsPage } from './pages/JobsPage';
import { LauncherPage } from './pages/LauncherPage';
import { LoginPage } from './pages/LoginPage';
import { MinecraftBuildPage } from './pages/MinecraftBuildPage';
import { MinecraftBuildsPage } from './pages/MinecraftBuildsPage';
import { ModuleDisabledPage } from './pages/ModuleDisabledPage';
import { MonitoringPage } from './pages/MonitoringPage';
import { ForbiddenPage, NotFoundPage } from './pages/NotFoundPage';
import { ProfilePage } from './pages/ProfilePage';
import { PublicSharePage } from './pages/PublicSharePage';
import { RolesPage } from './pages/RolesPage';
import { StoragePage } from './pages/StoragePage';
import { SystemOverviewPage } from './pages/SystemOverviewPage';
import { SystemGeneralPage, SystemLimitsPage, SystemNetworkPage } from './pages/SystemGeneralPages';
import { SystemMaintenancePage, SystemSecurityPage } from './pages/SystemSecurityPages';
import { UsersPage } from './pages/UsersPage';
import { useAuth, useCan, useSession } from './stores/auth';
import { resetBackgroundStore, useBackground } from './stores/background';
import { useAppearance } from './stores/appearance';
import { useTransfers } from './stores/transfers';
import { realtime } from './lib/realtime';
import { toast } from './stores/ui';

function RequireAuth() {
  const status = useAuth((state) => state.status);
  const location = useLocation();

  if (status === 'loading') return <LoadingScreen />;
  if (status !== 'authenticated') {
    return (
      <Navigate to="/login" replace state={{ from: `${location.pathname}${location.search}` }} />
    );
  }
  return <Outlet />;
}

function RequirePermission({ permission, children }: { permission: string; children: ReactNode }) {
  const allowed = useCan(permission);
  if (!allowed) return <ForbiddenPage />;
  return <>{children}</>;
}

/**
 * UI-гейт отключённого модуля.
 *
 * Сервер уже отвечает `MODULE_DISABLED`, но прямой переход по URL не должен
 * показывать интерфейс, который гарантированно не сможет загрузить данные.
 */
function RequireModule({
  module,
  children,
}: {
  module: 'minecraft' | 'cloud' | 'ai';
  children: ReactNode;
}) {
  const session = useSession();
  const enabled = session?.modules[module]?.enabled ?? true;
  if (!enabled) return <ModuleDisabledPage module={module} />;
  return <>{children}</>;
}

export default function App() {
  const status = useAuth((state) => state.status);

  useEffect(() => {
    const onExpired = (): void => {
      const previous = useAuth.getState().status;
      if (previous === 'authenticated' && window.location.pathname !== '/login') {
        toast('Сессия истекла. Войдите снова.', 'warning');
      }
    };
    window.addEventListener('nexus:session-expired', onExpired);
    return () => window.removeEventListener('nexus:session-expired', onExpired);
  }, []);

  useEffect(() => {
    void useAuth.getState().bootstrap();
    // Публичный фон нужен до входа: форма входа и публичные ссылки должны
    // использовать системное значение администратора, а не заводские параметры.
    void useBackground.getState().loadPublic();
  }, []);

  useEffect(() => {
    if (status === 'authenticated') {
      void useBackground.getState().load();
      void useAppearance.getState().load();
      realtime.connect();
      return;
    }
    if (status === 'anonymous') {
      realtime.disconnect();
      // Настройки предыдущего пользователя не должны переходить следующему.
      resetBackgroundStore();
      // После сброса возвращаем системный фон, действующий для всех.
      void useBackground.getState().loadPublic();
      useTransfers.getState().reset();
      useAppearance.setState({
        settings: { ...DEFAULT_INTERFACE },
        saved: { ...DEFAULT_INTERFACE },
        loaded: false,
        loading: false,
        error: null,
      });
      applyAccent(DEFAULT_INTERFACE.accent);
    }
  }, [status]);

  // Если первый запрос настроек не удался (например, сеть ещё не готова
  // сразу после входа или до входа), повторяем его при возврате фокуса или
  // сети. Это гарантирует, что системный default применится, а не останется
  // заводской.
  useEffect(() => {
    if (status === 'loading') return;
    const retry = (): void => {
      if (status === 'authenticated') {
        if (!useBackground.getState().loaded) void useBackground.getState().load(true);
        return;
      }
      void useBackground.getState().loadPublic();
    };
    window.addEventListener('focus', retry);
    window.addEventListener('online', retry);
    return () => {
      window.removeEventListener('focus', retry);
      window.removeEventListener('online', retry);
    };
  }, [status]);

  if (status === 'loading') {
    return (
      <>
        <GlobalBackground />
        <LoadingScreen label="Проверка сессии" />
        <Toaster />
      </>
    );
  }

  return (
    <>
      <GlobalBackground />
      <Routes>
        <Route path="/login" element={<LoginPage />} />
        <Route path="/developers" element={<DevelopersPage />} />
        <Route path="/s/:token" element={<PublicSharePage />} />
        <Route element={<RequireAuth />}>
          <Route path="/" element={<HomePage />} />
          <Route element={<AppLayout />}>
            <Route path="/profile" element={<ProfilePage />} />
            <Route path="/system/appearance" element={<AppearanceSettingsPage />} />
            <Route path="/system/background" element={<BackgroundSettingsPage />} />
            <Route
              path="/system"
              element={
                <RequirePermission permission="system.view">
                  <SystemOverviewPage />
                </RequirePermission>
              }
            />
            <Route
              path="/system/users"
              element={
                <RequirePermission permission="system.users.manage">
                  <UsersPage />
                </RequirePermission>
              }
            />
            <Route
              path="/system/roles"
              element={
                <RequirePermission permission="system.users.manage">
                  <RolesPage />
                </RequirePermission>
              }
            />
            <Route
              path="/system/audit"
              element={
                <RequirePermission permission="system.view">
                  <AuditPage />
                </RequirePermission>
              }
            />
            <Route
              path="/system/monitoring"
              element={
                <RequirePermission permission="system.view">
                  <MonitoringPage />
                </RequirePermission>
              }
            />
            <Route
              path="/system/jobs"
              element={
                <RequirePermission permission="system.view">
                  <JobsPage />
                </RequirePermission>
              }
            />
            <Route
              path="/system/storage"
              element={
                <RequirePermission permission="system.storage.manage">
                  <StoragePage />
                </RequirePermission>
              }
            />
            <Route
              path="/system/general"
              element={
                <RequirePermission permission="system.settings.manage">
                  <SystemGeneralPage />
                </RequirePermission>
              }
            />
            <Route
              path="/system/limits"
              element={
                <RequirePermission permission="system.settings.manage">
                  <SystemLimitsPage />
                </RequirePermission>
              }
            />
            <Route
              path="/system/network"
              element={
                <RequirePermission permission="system.settings.manage">
                  <SystemNetworkPage />
                </RequirePermission>
              }
            />
            <Route
              path="/system/security"
              element={
                <RequirePermission permission="system.settings.manage">
                  <SystemSecurityPage />
                </RequirePermission>
              }
            />
            <Route
              path="/system/maintenance"
              element={
                <RequirePermission permission="system.view">
                  <SystemMaintenancePage />
                </RequirePermission>
              }
            />
            <Route
              path="/system/ai"
              element={
                <RequirePermission permission="ai.admin.manage">
                  <AiAdminPage />
                </RequirePermission>
              }
            />
            <Route
              path="/workspace/ai"
              element={
                <RequireModule module="ai">
                  <RequirePermission permission="ai.access">
                    <AiWorkspacePage />
                  </RequirePermission>
                </RequireModule>
              }
            />
            <Route
              path="/workspace/builds"
              element={
                <RequireModule module="minecraft">
                  <RequirePermission permission="minecraft.view">
                    <MinecraftBuildsPage />
                  </RequirePermission>
                </RequireModule>
              }
            />
            <Route
              path="/workspace/builds/:id"
              element={
                <RequireModule module="minecraft">
                  <RequirePermission permission="minecraft.view">
                    <MinecraftBuildPage />
                  </RequirePermission>
                </RequireModule>
              }
            />
            <Route
              path="/workspace/storage"
              element={
                <RequireModule module="cloud">
                  <RequirePermission permission="cloud.view">
                    <CloudStoragePage />
                  </RequirePermission>
                </RequireModule>
              }
            />
            <Route
              path="/workspace/launcher"
              element={
                <RequireModule module="minecraft">
                  <RequirePermission permission="minecraft.view">
                    <LauncherPage />
                  </RequirePermission>
                </RequireModule>
              }
            />
            <Route path="*" element={<NotFoundPage />} />
          </Route>
        </Route>
      </Routes>
      {status === 'authenticated' ? <TransferDrawer /> : null}
      <Toaster />
    </>
  );
}
