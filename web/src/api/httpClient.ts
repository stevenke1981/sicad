import { ApiError, type ApiClient, type HealthStatus } from './client';
import type {
  CadDocument,
  CapabilityResponse,
  GenerateFromSpecRequest,
  GenerateRequest,
  JobListItem,
  JobManifest,
  PlanRequest,
  PlanResponse,
  ValidationReport,
} from './types';

export interface HttpClientOptions {
  /** Backend origin, e.g. `http://127.0.0.1:51234`. Empty string means same origin. */
  baseUrl: string;
  /** API token sent as `Authorization: Bearer`; omitted when empty. */
  token?: string | null;
  /** Injected for tests; defaults to the global `fetch`. */
  fetch?: typeof fetch;
}

const API_PREFIX = '/api/v1';

/** Turn a FastAPI error body (`{detail}`) into a readable message. */
export function errorMessageFromBody(body: unknown, fallback: string): string {
  if (body === null || typeof body !== 'object' || !('detail' in body)) return fallback;
  const detail = (body as { detail: unknown }).detail;
  if (typeof detail === 'string' && detail.trim()) return detail;
  if (Array.isArray(detail)) {
    // RequestValidationError: [{loc: [...], msg: "...", type: "..."}]
    const parts = detail.map((item: unknown) => {
      if (item && typeof item === 'object' && 'msg' in item) {
        const { loc, msg } = item as { loc?: unknown; msg: unknown };
        const where = Array.isArray(loc) ? loc.filter((p) => p !== 'body').join('.') : '';
        return where ? `${where}: ${String(msg)}` : String(msg);
      }
      return typeof item === 'string' ? item : JSON.stringify(item);
    });
    if (parts.length) return parts.join('; ');
  }
  if (detail !== undefined && detail !== null) return JSON.stringify(detail);
  return fallback;
}

export function createHttpClient(options: HttpClientOptions): ApiClient {
  const baseUrl = options.baseUrl.replace(/\/+$/, '');
  const token = options.token?.trim() || null;
  const doFetch: typeof fetch = options.fetch ?? ((input, init) => globalThis.fetch(input, init));

  const origin = (): string => {
    if (baseUrl) return new URL(baseUrl).origin;
    return globalThis.location?.origin ?? 'http://localhost';
  };

  /** Resolve a path or URL against the backend, refusing other origins (token must not leak). */
  const resolve = (pathOrUrl: string): string => {
    const target = new URL(pathOrUrl, baseUrl ? `${baseUrl}/` : `${origin()}/`);
    if (target.origin !== origin()) {
      throw new ApiError(0, `拒絕存取非後端來源的網址：${target.origin}`);
    }
    return baseUrl ? target.toString() : `${target.pathname}${target.search}`;
  };

  const headers = (json: boolean): Record<string, string> => {
    const out: Record<string, string> = { Accept: json ? 'application/json' : '*/*' };
    if (json) out['Content-Type'] = 'application/json';
    if (token) out.Authorization = `Bearer ${token}`;
    return out;
  };

  const send = async (path: string, init: RequestInit & { json?: unknown }): Promise<Response> => {
    const { json, ...rest } = init;
    const hasBody = json !== undefined;
    const response = await doFetch(resolve(path), {
      ...rest,
      headers: { ...headers(hasBody), ...(rest.headers as Record<string, string> | undefined) },
      body: hasBody ? JSON.stringify(json) : rest.body ?? null,
    });
    if (!response.ok) {
      const fallback = `${response.status} ${response.statusText}`.trim();
      let body: unknown = null;
      try {
        body = await response.json();
      } catch {
        // Non-JSON error body: fall back to the HTTP status line.
      }
      const detail = body && typeof body === 'object' && 'detail' in body ? body.detail : undefined;
      throw new ApiError(response.status, errorMessageFromBody(body, fallback), detail);
    }
    return response;
  };

  const getJson = async <T>(path: string): Promise<T> =>
    (await (await send(path, { method: 'GET' })).json()) as T;
  const postJson = async <T>(path: string, json: unknown): Promise<T> =>
    (await (await send(path, { method: 'POST', json })).json()) as T;

  const jobPath = (jobId: string) => `${API_PREFIX}/jobs/${encodeURIComponent(jobId)}`;

  return {
    health: () => getJson<HealthStatus>(`${API_PREFIX}/health`),
    capabilities: () => getJson<CapabilityResponse>(`${API_PREFIX}/capabilities`),
    plan: (request: PlanRequest) => postJson<PlanResponse>(`${API_PREFIX}/plan`, request),
    generate: (request: GenerateRequest) =>
      postJson<JobManifest>(`${API_PREFIX}/generate`, request),
    generateFromSpec: (request: GenerateFromSpecRequest) =>
      postJson<JobManifest>(`${API_PREFIX}/generate-from-spec`, request),
    validate: (spec: CadDocument) => postJson<ValidationReport>(`${API_PREFIX}/validate`, spec),
    listJobs: () => getJson<JobListItem[]>(`${API_PREFIX}/jobs`),
    getJob: (jobId: string) => getJson<JobManifest>(jobPath(jobId)),
    fetchArtifact: async (url: string) => (await send(url, { method: 'GET' })).blob(),
    bundleUrl: (jobId: string) => `${jobPath(jobId)}/bundle.zip`,
  };
}
