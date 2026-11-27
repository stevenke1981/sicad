// Client for the Rust core envelope (crates/promptcad-core `api` module). Types come from
// the generated core-request.d.ts / core-response.d.ts (ADR 0008). This module never touches
// the Tauri global: the desktop transport is `BackendInfo.coreInvoke` from bootstrap.ts.
import type { BackendInfo } from './bootstrap';
import { removeTopLevelKey } from './jsonText';
import type { CoreRequest } from './generated/core-request';
import type {
  CoreResponse,
  CoreResult,
  ErrorCode,
  PatchOrigin,
  SchemaViolation,
} from './generated/core-response';

export type { CoreRequest, CoreResponse, CoreResult, ErrorCode, PatchOrigin, SchemaViolation };

/** Narrow the result union by its `op` tag. */
export type ResultOf<Op extends CoreResult['op']> = Extract<CoreResult, { op: Op }>;

/** A raw document / patch as sent to the core (validated by the core, not by the client). */
export type RawObject = Record<string, unknown>;

/**
 * The single core interface used by UI code (ADR 0003). One request in, one response
 * envelope out: both `status: "ok"` and `status: "error"` are normal resolutions. Only
 * transport failures reject, with {@link CoreTransportError}.
 */
export interface CoreClient {
  request(req: CoreRequest, options?: CoreRequestOptions): Promise<CoreResponse>;
  /**
   * True when the transport sends {@link CoreRequestOptions.documentText} verbatim, so
   * numbers keep the exact digits of the editor text (HTTP). The Tauri transport needs an
   * object for `invoke` and always sends the parsed `req.document`.
   */
  readonly verbatimDocuments?: boolean;
}

export interface CoreRequestOptions {
  /**
   * The verbatim JSON text of `req.document` (it must parse to the same object). Transports
   * that can send text use it instead of re-serializing `req.document`.
   */
  documentText?: string;
}

/** The core processed the request but could not handle it (`status: "error"`). */
export class CoreError extends Error {
  readonly code: ErrorCode;
  readonly path: string | null;
  readonly violations: SchemaViolation[];

  constructor(code: ErrorCode, message: string, path: string | null, violations: SchemaViolation[]) {
    super(message);
    this.name = 'CoreError';
    this.code = code;
    this.path = path;
    this.violations = violations;
  }
}

export type CoreTransportKind =
  | 'unavailable' // HTTP 503: no core binary on the server
  | 'timeout' // HTTP 504
  | 'core_failed' // HTTP 502
  | 'too_large' // HTTP 413
  | 'auth' // HTTP 401 / 403
  | 'not_found' // HTTP 404: server predates the core endpoint
  | 'network' // fetch rejected / invoke rejected
  | 'protocol'; // unexpected status or malformed envelope

/** The request never produced a core response (host or HTTP failure). */
export class CoreTransportError extends Error {
  readonly kind: CoreTransportKind;
  readonly status: number | null;

  constructor(kind: CoreTransportKind, message: string, status: number | null = null) {
    super(message);
    this.name = 'CoreTransportError';
    this.kind = kind;
    this.status = status;
  }
}

/** True when the core cannot be used on this backend at all (not a one-off failure). */
export function isCoreUnavailable(error: unknown): boolean {
  return (
    error instanceof CoreTransportError &&
    (error.kind === 'unavailable' || error.kind === 'not_found')
  );
}

/**
 * Example documents carry an example-only top-level key `expected_resolved` that is not part
 * of the schema; the core rejects it, so callers strip it before sending.
 */
export function stripExampleKeys<T extends object>(doc: T): T {
  if (!('expected_resolved' in doc)) return doc;
  const { expected_resolved: _dropped, ...rest } = doc as T & { expected_resolved?: unknown };
  return rest as T;
}

/**
 * A document straight from editor text. `value` is the parsed object (for transports that
 * need one); `text` is the source with example-only keys removed, sent verbatim over HTTP so
 * integers above 2^53, integers ≥ 1e21 and `-0` reach the core unchanged.
 */
export class DocumentText {
  readonly text: string;
  readonly value: RawObject;

  /** Throws `SyntaxError` for invalid JSON and `TypeError` when it is not a JSON object. */
  constructor(source: string) {
    const parsed: unknown = JSON.parse(source);
    if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
      throw new TypeError('document must be a JSON object');
    }
    const value = parsed as RawObject;
    if ('expected_resolved' in value) {
      this.text = removeTopLevelKey(source, 'expected_resolved');
      this.value = stripExampleKeys(value);
    } else {
      this.text = source;
      this.value = value;
    }
  }
}

/** A document argument: a parsed object, or editor text ({@link DocumentText}). */
export type DocumentInput = RawObject | DocumentText;

function checked(value: unknown): CoreResponse {
  const status =
    value !== null && typeof value === 'object' ? (value as { status?: unknown }).status : null;
  if (status !== 'ok' && status !== 'error') {
    throw new CoreTransportError('protocol', 'Rust core 回傳的格式不正確。');
  }
  return value as CoreResponse;
}

/** Desktop: the Tauri shell exposes `core_request`; `invoke` comes from bootstrap.ts. */
export function createTauriCoreClient(invoke: (request: unknown) => Promise<unknown>): CoreClient {
  return {
    verbatimDocuments: false,
    // `invoke` takes an object: `options.documentText` cannot be used on this transport.
    async request(req) {
      let raw: unknown;
      try {
        raw = await invoke(req);
      } catch (error) {
        const detail = error instanceof Error ? error.message : String(error);
        throw new CoreTransportError('network', `桌面程式無法呼叫 Rust core：${detail}`);
      }
      return checked(raw);
    },
  };
}

export interface HttpCoreClientOptions {
  /** Backend origin; empty string means same origin. */
  baseUrl: string;
  /** API token sent as `Authorization: Bearer`; omitted when empty. */
  token?: string | null;
  /** Injected for tests; defaults to the global `fetch`. */
  fetch?: typeof fetch;
}

const CORE_PATH = '/api/v1/core';

/** Browser/server: `POST /api/v1/core`; HTTP 200 carries both `ok` and `error` envelopes. */
export function createHttpCoreClient(options: HttpCoreClientOptions): CoreClient {
  const baseUrl = options.baseUrl.replace(/\/+$/, '');
  const token = options.token?.trim() || null;
  const doFetch: typeof fetch = options.fetch ?? ((input, init) => globalThis.fetch(input, init));

  return {
    verbatimDocuments: true,
    async request(req, options = {}) {
      const headers: Record<string, string> = {
        Accept: 'application/json',
        'Content-Type': 'application/json',
      };
      if (token) headers.Authorization = `Bearer ${token}`;
      let response: Response;
      try {
        response = await doFetch(`${baseUrl}${CORE_PATH}`, {
          method: 'POST',
          headers,
          body: requestBody(req, options.documentText),
        });
      } catch (error) {
        const detail = error instanceof Error ? error.message : String(error);
        throw new CoreTransportError('network', `無法連線到伺服器：${detail}`);
      }
      if (response.status !== 200) throw transportErrorForStatus(response.status);
      let body: unknown;
      try {
        body = await response.json();
      } catch {
        throw new CoreTransportError('protocol', '伺服器回傳的不是有效的 JSON。', 200);
      }
      return checked(body);
    },
  };
}

/**
 * The request JSON. With `documentText`, the document is spliced in verbatim instead of
 * re-serialized, so its numbers are not round-tripped through JavaScript doubles.
 */
export function requestBody(req: CoreRequest, documentText?: string): string {
  if (documentText === undefined || !('document' in req)) return JSON.stringify(req);
  const { document: _parsed, ...rest } = req;
  const head = JSON.stringify(rest); // `{"op":"…",…}`: never empty (`op` is required)
  return `${head.slice(0, -1)},"document":${documentText}}`;
}

function transportErrorForStatus(status: number): CoreTransportError {
  switch (status) {
    case 503:
      return new CoreTransportError(
        'unavailable',
        '伺服器未安裝 Rust core（找不到 promptcad-core 執行檔，HTTP 503），無法處理 2.0 文件。',
        status,
      );
    case 504:
      return new CoreTransportError('timeout', 'Rust core 處理逾時（HTTP 504）。', status);
    case 502:
      return new CoreTransportError(
        'core_failed',
        'Rust core 執行失敗或輸出異常（HTTP 502）。',
        status,
      );
    case 413:
      return new CoreTransportError('too_large', '文件太大，伺服器拒絕處理（HTTP 413）。', status);
    case 401:
    case 403:
      return new CoreTransportError(
        'auth',
        `Rust core 驗證失敗（HTTP ${status}）：請確認 API Token。`,
        status,
      );
    case 404:
      return new CoreTransportError(
        'not_found',
        '此伺服器版本不支援 Rust core 端點（HTTP 404）。',
        status,
      );
    default:
      return new CoreTransportError('protocol', `Rust core 請求失敗（HTTP ${status}）。`, status);
  }
}

/** Pick the transport from `BackendInfo.mode` (token overrides the bootstrap token). */
export function createCoreClient(
  backend: BackendInfo,
  options: { token?: string | null; fetch?: typeof fetch } = {},
): CoreClient {
  if (backend.mode === 'tauri' && backend.coreInvoke) {
    return createTauriCoreClient(backend.coreInvoke);
  }
  const http: HttpCoreClientOptions = {
    baseUrl: backend.baseUrl,
    token: options.token === undefined ? backend.token : options.token,
  };
  if (options.fetch) http.fetch = options.fetch;
  return createHttpCoreClient(http);
}

/** Typed helpers over a {@link CoreClient}. `status: "error"` becomes a {@link CoreError}. */
export interface CoreApi {
  /** See {@link CoreClient.verbatimDocuments}: `false` means numbers may be altered. */
  readonly verbatimDocuments: boolean;
  validate(document: DocumentInput, completed?: boolean): Promise<ResultOf<'validate'>>;
  describe(document: DocumentInput): Promise<ResultOf<'describe'>>;
  solve(document: DocumentInput): Promise<ResultOf<'solve'>>;
  normalize(document: DocumentInput): Promise<ResultOf<'normalize'>>;
  upgrade(document: DocumentInput): Promise<ResultOf<'upgrade'>>;
  downgrade(document: DocumentInput): Promise<ResultOf<'downgrade'>>;
  /** A rejected patch is a normal return value: inspect `outcome.status`. */
  applyPatch(
    document: DocumentInput,
    patch: RawObject,
    origin: PatchOrigin,
    extra?: { logSeq?: number; timestamp?: string | null },
  ): Promise<ResultOf<'apply_patch'>>;
  version(): Promise<ResultOf<'version'>>;
}

/** The parsed document for `req.document`, plus its verbatim text when there is one. */
function documentParts(document: DocumentInput): [RawObject, CoreRequestOptions] {
  return document instanceof DocumentText
    ? [document.value, { documentText: document.text }]
    : [stripExampleKeys(document), {}];
}

type DocumentOnlyOp = 'describe' | 'solve' | 'normalize' | 'upgrade' | 'downgrade';

export function createCoreApi(client: CoreClient): CoreApi {
  async function call<Op extends CoreResult['op']>(
    req: CoreRequest,
    op: Op,
    options: CoreRequestOptions = {},
  ): Promise<ResultOf<Op>> {
    const response =
      options.documentText === undefined
        ? await client.request(req)
        : await client.request(req, options);
    if (response.status === 'error') {
      throw new CoreError(response.code, response.message, response.path, response.violations);
    }
    if (response.result.op !== op) {
      throw new CoreTransportError(
        'protocol',
        `Rust core 回傳了非預期的結果：${response.result.op}。`,
      );
    }
    return response.result as ResultOf<Op>;
  }

  function documentOp<Op extends DocumentOnlyOp>(op: Op, input: DocumentInput) {
    const [document, options] = documentParts(input);
    return call({ op, document } as CoreRequest, op, options);
  }

  return {
    verbatimDocuments: client.verbatimDocuments === true,
    validate: (input, completed) => {
      const [document, options] = documentParts(input);
      return call(
        completed === undefined
          ? { op: 'validate', document }
          : { op: 'validate', document, completed },
        'validate',
        options,
      );
    },
    describe: (input) => documentOp('describe', input),
    solve: (input) => documentOp('solve', input),
    normalize: (input) => documentOp('normalize', input),
    upgrade: (input) => documentOp('upgrade', input),
    downgrade: (input) => documentOp('downgrade', input),
    applyPatch: (input, patch, origin, extra = {}) => {
      const [document, options] = documentParts(input);
      return call(
        {
          op: 'apply_patch',
          document,
          patch,
          origin,
          ...(extra.logSeq !== undefined ? { log_seq: extra.logSeq } : {}),
          ...(extra.timestamp !== undefined ? { timestamp: extra.timestamp } : {}),
        },
        'apply_patch',
        options,
      );
    },
    version: () => call({ op: 'version' }, 'version'),
  };
}
