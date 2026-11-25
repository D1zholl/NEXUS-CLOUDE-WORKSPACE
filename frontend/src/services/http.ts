/**
 * HTTP-ядро клиента API: cookie-сессия, CSRF, идемпотентность.
 *
 * Сгенерировано при разделении services/api.ts; правки — в этом модуле.
 */

interface ApiErrorBody {
  code?: string;
  message?: string;
  details?: unknown;
}

interface ApiFailure {
  success: false;
  error: ApiErrorBody;
  request_id?: string;
}

interface ApiSuccess<T> {
  success: true;
  data: T;
}

export class ApiRequestError extends Error {
  readonly code: string;
  readonly status: number;
  readonly details: unknown;
  readonly requestId: string | null;

  constructor(
    status: number,
    code: string,
    message: string,
    details: unknown = null,
    requestId: string | null = null,
  ) {
    super(message);
    this.name = 'ApiRequestError';
    this.status = status;
    this.code = code;
    this.details = details;
    this.requestId = requestId;
  }
}

let csrfToken: string | null = null;

export function setCsrfToken(token: string | null): void {
  csrfToken = token;
}

export function getCsrfToken(): string | null {
  return csrfToken;
}

const SAFE_METHODS = new Set(['GET', 'HEAD']);

/**
 * Новый ключ идемпотентности для одной опасной операции.
 *
 * Ключ создаётся один раз перед первым запросом и передаётся повторно при
 * retry, чтобы сервер вернул сохранённый результат вместо дубля.
 */
export function newIdempotencyKey(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  // Резерв для сред без Web Crypto (старые браузеры, SSR-тесты).
  return `idem-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

export function idempotencyHeaders(key?: string | null): Record<string, string> {
  return key ? { 'Idempotency-Key': key } : {};
}

function isFailure(value: unknown): value is ApiFailure {
  return (
    typeof value === 'object' &&
    value !== null &&
    (value as { success?: unknown }).success === false
  );
}

export async function request<T>(
  path: string,
  init: {
    method?: string;
    body?: unknown;
    signal?: AbortSignal;
    raw?: Blob;
    formData?: FormData;
    headers?: Record<string, string>;
  } = {},
): Promise<T> {
  const method = init.method ?? 'GET';
  const headers: Record<string, string> = { Accept: 'application/json', ...init.headers };
  if (init.body !== undefined && init.formData === undefined) {
    headers['Content-Type'] = 'application/json';
  }
  if (!SAFE_METHODS.has(method) && csrfToken) {
    headers['X-CSRF-Token'] = csrfToken;
  }

  let response: Response;
  try {
    response = await fetch(`/api/v1${path}`, {
      method,
      headers,
      credentials: 'same-origin',
      body:
        init.formData !== undefined
          ? init.formData
          : init.raw !== undefined
            ? init.raw
            : init.body === undefined
              ? undefined
              : JSON.stringify(init.body),
      signal: init.signal,
    });
  } catch {
    throw new ApiRequestError(
      0,
      'NETWORK_ERROR',
      'Не удалось связаться с сервером. Проверьте соединение и повторите попытку.',
    );
  }

  const text = await response.text();
  let payload: unknown = null;
  if (text) {
    try {
      payload = JSON.parse(text) as unknown;
    } catch {
      payload = null;
    }
  }

  if (!response.ok || isFailure(payload)) {
    const failure = isFailure(payload) ? payload : null;
    const body = failure?.error ?? {};
    const code = body.code || (response.ok ? 'INVALID_RESPONSE' : `HTTP_${response.status}`);
    const message =
      body.message ||
      (response.status === 0
        ? 'Сервер недоступен.'
        : 'Запрос не выполнен. Повторите попытку позже.');
    if (response.status === 401 && !path.startsWith('/auth/login')) {
      window.dispatchEvent(new Event('nexus:session-expired'));
    }
    throw new ApiRequestError(
      response.status,
      code,
      message,
      body.details ?? null,
      failure?.request_id ?? response.headers.get('x-request-id'),
    );
  }

  if (!payload || typeof payload !== 'object' || (payload as ApiSuccess<T>).success !== true) {
    throw new ApiRequestError(
      response.status,
      'INVALID_RESPONSE',
      'Сервер вернул неожиданный ответ. Обновите страницу.',
    );
  }

  return (payload as ApiSuccess<T>).data;
}

export function queryString(
  params: Record<string, string | number | boolean | undefined | null>,
): string {
  const search = new URLSearchParams();
  for (const [key, value] of Object.entries(params)) {
    if (value === undefined || value === null || value === '') continue;
    search.set(key, String(value));
  }
  const result = search.toString();
  return result ? `?${result}` : '';
}

export function errorMessage(error: unknown): string {
  if (error instanceof ApiRequestError) return error.message;
  if (error instanceof Error && error.message) return error.message;
  return 'Произошла непредвиденная ошибка.';
}
