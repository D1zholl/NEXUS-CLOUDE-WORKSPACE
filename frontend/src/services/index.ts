/**
 * Публичный фасад сервисов API.
 *
 * Сгенерировано при разделении services/api.ts; правки — в этом модуле.
 */

import { authApi } from './auth';
import { aiApi } from './ai';
import { cloudApi } from './cloud';
import { launcherApi } from './launcher';
import { minecraftApi } from './minecraft';
import { rolesApi } from './roles';
import { storageApi } from './storage';
import { systemApi, monitoringApi } from './system';
import { usersApi } from './users';

export * from './http';
export * from './types';
export * from './ai';
export * from './publicShare';
export * from './launcher';

export const api = {
  ...authApi,
  ...usersApi,
  ...rolesApi,
  ...systemApi,
  ...monitoringApi,
  ...storageApi,
  ...cloudApi,
  ...minecraftApi,
  ...launcherApi,
  ...aiApi,
};
