import { describe, expect, it, vi } from 'vitest';
import { plateDescribe, unsupportedVersionError, versionResponse } from '../test/fixtures';
import { resolveBackend } from './bootstrap';
import {
  CoreError,
  CoreTransportError,
  createCoreApi,
  createCoreClient,
  createHttpCoreClient,
  createTauriCoreClient,
  DocumentText,
  requestBody,
  stripExampleKeys,
  type CoreRequest,
} from './coreClient';

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json' },
  });
}

const DOC = { schema_version: '2.0', name: 'x', expected_resolved: { note: 'example only' } };
const CLEAN = { schema_version: '2.0', name: 'x' };

describe('stripExampleKeys', () => {
  it('removes only expected_resolved and does not mutate the input', () => {
    expect(stripExampleKeys(DOC)).toEqual(CLEAN);
    expect(DOC).toHaveProperty('expected_resolved');
    const plain = { a: 1 };
    expect(stripExampleKeys(plain)).toBe(plain);
  });
});

describe('HTTP transport', () => {
  it('POSTs the request to /api/v1/core with the Bearer token and returns the envelope', async () => {
    const fetchMock = vi.fn<typeof fetch>(async () => json(plateDescribe));
    const client = createHttpCoreClient({
      baseUrl: 'http://127.0.0.1:8000/',
      token: ' tok ',
      fetch: fetchMock,
    });
    const req: CoreRequest = { op: 'describe', document: { schema_version: '2.0' } };

    await expect(client.request(req)).resolves.toEqual(plateDescribe);

    const [url, init] = fetchMock.mock.calls[0]!;
    expect(url).toBe('http://127.0.0.1:8000/api/v1/core');
    expect(init?.method).toBe('POST');
    const headers = init?.headers as Record<string, string>;
    expect(headers.Authorization).toBe('Bearer tok');
    expect(headers['Content-Type']).toBe('application/json');
    expect(JSON.parse(String(init?.body))).toEqual(req);
  });

  it('omits Authorization without a token and uses a same-origin path', async () => {
    const fetchMock = vi.fn<typeof fetch>(async () => json(versionResponse));
    await createHttpCoreClient({ baseUrl: '', fetch: fetchMock }).request({ op: 'version' });
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(url).toBe('/api/v1/core');
    expect((init?.headers as Record<string, string>).Authorization).toBeUndefined();
  });

  it.each([
    [503, 'unavailable', /未安裝 Rust core/],
    [504, 'timeout', /逾時/],
    [502, 'core_failed', /執行失敗/],
    [413, 'too_large', /太大/],
    [401, 'auth', /Token/],
    [403, 'auth', /Token/],
    [404, 'not_found', /不支援/],
    [500, 'protocol', /HTTP 500/],
  ])('maps HTTP %i to a CoreTransportError(%s)', async (status, kind, message) => {
    const client = createHttpCoreClient({
      baseUrl: '',
      fetch: vi.fn<typeof fetch>(async () => json({ detail: 'x' }, status)),
    });
    const error = await client.request({ op: 'version' }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreTransportError);
    expect(error).toMatchObject({ kind, status });
    expect((error as Error).message).toMatch(message);
  });

  it('turns a network failure and a malformed body into transport errors', async () => {
    const down = createHttpCoreClient({
      baseUrl: '',
      fetch: vi.fn<typeof fetch>(async () => {
        throw new TypeError('Failed to fetch');
      }),
    });
    await expect(down.request({ op: 'version' })).rejects.toMatchObject({ kind: 'network' });

    const junk = createHttpCoreClient({
      baseUrl: '',
      fetch: vi.fn<typeof fetch>(async () => json({ hello: 1 })),
    });
    await expect(junk.request({ op: 'version' })).rejects.toMatchObject({ kind: 'protocol' });
  });
});

describe('Tauri transport', () => {
  it('passes the request object to invoke and returns the response', async () => {
    const invoke = vi.fn(async (_req: unknown) => plateDescribe);
    const client = createTauriCoreClient(invoke);
    const req: CoreRequest = { op: 'version' };
    await expect(client.request(req)).resolves.toEqual(plateDescribe);
    expect(invoke).toHaveBeenCalledWith(req);
  });

  it('turns a rejected invoke into a CoreTransportError', async () => {
    const client = createTauriCoreClient(async () => {
      throw 'command core_request not found';
    });
    const error = await client.request({ op: 'version' }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreTransportError);
    expect((error as Error).message).toContain('command core_request not found');
  });

  it('is selected by BackendInfo.mode, going through bootstrap.coreInvoke', async () => {
    const invoke = vi.fn(async (cmd: string) =>
      cmd === 'backend_info' ? { base_url: 'http://127.0.0.1:1', token: 't' } : versionResponse,
    );
    const win = { __TAURI__: { core: { invoke } } } as unknown as Parameters<
      typeof resolveBackend
    >[0];
    const backend = await resolveBackend(win);
    await createCoreApi(createCoreClient(backend)).version();
    expect(invoke).toHaveBeenLastCalledWith('core_request', { request: { op: 'version' } });

    const fetchMock = vi.fn<typeof fetch>(async () => json(versionResponse));
    await createCoreApi(
      createCoreClient(
        { mode: 'browser', baseUrl: '', token: null },
        { token: 'abc', fetch: fetchMock },
      ),
    ).version();
    expect(fetchMock).toHaveBeenCalledOnce();
    expect((fetchMock.mock.calls[0]![1]?.headers as Record<string, string>).Authorization).toBe(
      'Bearer abc',
    );
  });
});

describe('createCoreApi', () => {
  it('strips example keys before sending and narrows the result by op', async () => {
    const request = vi.fn(async (_req: CoreRequest) => plateDescribe);
    const result = await createCoreApi({ request }).describe(DOC);
    expect(request).toHaveBeenCalledWith({ op: 'describe', document: CLEAN });
    // `result` is the describe result: the union narrowing is checked by tsc here.
    expect(result.op).toBe('describe');
    expect(result.name).toBe('plate-four-holes');
  });

  it('sends `completed` only when given, and apply_patch fields under their wire names', async () => {
    const request = vi.fn(async (_req: CoreRequest) => versionResponse);
    const api = createCoreApi({ request });
    // The canned version response does not match these ops; only the sent requests matter.
    await api.validate(DOC).catch(() => undefined);
    await api.validate(DOC, false).catch(() => undefined);
    await api.applyPatch(DOC, { x: 1 }, 'human', { logSeq: 3, timestamp: null }).catch(() => undefined);
    expect(request.mock.calls[0]![0]).toEqual({ op: 'validate', document: CLEAN });
    expect(request.mock.calls[1]![0]).toEqual({ op: 'validate', document: CLEAN, completed: false });
    expect(request.mock.calls[2]![0]).toEqual({
      op: 'apply_patch',
      document: CLEAN,
      patch: { x: 1 },
      origin: 'human',
      log_seq: 3,
      timestamp: null,
    });
  });

  it('turns status:"error" into a CoreError carrying code, path and violations', async () => {
    const api = createCoreApi({ request: async () => unsupportedVersionError });
    const error = await api.validate(DOC).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(CoreError);
    expect(error).toMatchObject({
      code: 'unsupported_schema_version',
      path: '/document/schema_version',
      violations: [],
    });
  });

  it('returns domain outcomes (a rejected patch) as normal values', async () => {
    const rejected = {
      status: 'ok',
      result: { op: 'apply_patch', outcome: { status: 'rejected', reason: { code: 'x' } } },
    };
    const api = createCoreApi({ request: async () => rejected as never });
    const result = await api.applyPatch(DOC, {}, 'human');
    expect(result.outcome.status).toBe('rejected');
  });

  it('rejects a result whose op does not match the request', async () => {
    const api = createCoreApi({ request: async () => versionResponse });
    await expect(api.describe(DOC)).rejects.toBeInstanceOf(CoreTransportError);
  });
});

// Editor text is sent verbatim over HTTP: JSON.parse would turn 2^64-1 into
// 18446744073709552000, 1e21-size integers into `1e+21` (rejected by the core) and -0 into 0.
const BIG_TEXT =
  '{"schema_version": "2.0", "revision": 18446744073709551615, "huge": 1000000000000000000000,' +
  ' "z": -0, "expected_resolved": {"revision": 1}}';
const BIG_SENT =
  '{"schema_version": "2.0","revision": 18446744073709551615,"huge": 1000000000000000000000,"z": -0}';

describe('DocumentText', () => {
  it('keeps the source text verbatim and strips expected_resolved textually', () => {
    const doc = new DocumentText(BIG_TEXT);
    expect(doc.text).toBe(BIG_SENT);
    expect(doc.value).not.toHaveProperty('expected_resolved');
    expect(doc.value.schema_version).toBe('2.0');
    const plain = '{"a": 18446744073709551615}';
    expect(new DocumentText(plain).text).toBe(plain);
  });

  it('rejects non-objects', () => {
    expect(() => new DocumentText('[1]')).toThrow(TypeError);
    expect(() => new DocumentText('null')).toThrow(TypeError);
    expect(() => new DocumentText('{')).toThrow(SyntaxError);
  });
});

describe('verbatim documents', () => {
  it('requestBody splices the document text into the request JSON', () => {
    const req: CoreRequest = { op: 'validate', document: {}, completed: false };
    expect(requestBody(req, '{"revision": 18446744073709551615}')).toBe(
      '{"op":"validate","completed":false,"document":{"revision": 18446744073709551615}}',
    );
    expect(requestBody(req)).toBe(JSON.stringify(req));
    expect(requestBody({ op: 'version' }, '{"x":1}')).toBe('{"op":"version"}');
  });

  it('the HTTP transport sends editor text byte-for-byte', async () => {
    const fetchMock = vi.fn<typeof fetch>(async () => json(plateDescribe));
    const api = createCoreApi(createHttpCoreClient({ baseUrl: '', fetch: fetchMock }));
    expect(api.verbatimDocuments).toBe(true);
    await api.describe(new DocumentText(BIG_TEXT));
    expect(String(fetchMock.mock.calls[0]![1]?.body)).toBe(
      `{"op":"describe","document":${BIG_SENT}}`,
    );
    // parsed objects are still serialized normally
    await api.describe({ schema_version: '2.0' });
    expect(String(fetchMock.mock.calls[1]![1]?.body)).toBe(
      '{"op":"describe","document":{"schema_version":"2.0"}}',
    );
  });

  it('every document op forwards the text; the Tauri transport gets the parsed object', async () => {
    const request = vi.fn(async (_req: CoreRequest, _options?: unknown) => versionResponse);
    const api = createCoreApi({ request });
    expect(api.verbatimDocuments).toBe(false);
    const doc = new DocumentText(BIG_TEXT);
    for (const call of [
      () => api.validate(doc),
      () => api.describe(doc),
      () => api.solve(doc),
      () => api.normalize(doc),
      () => api.upgrade(doc),
      () => api.downgrade(doc),
      () => api.applyPatch(doc, {}, 'human'),
    ]) {
      await call().catch(() => undefined);
    }
    for (const [req, options] of request.mock.calls) {
      expect((req as { document: unknown }).document).toBe(doc.value);
      expect(options).toEqual({ documentText: BIG_SENT });
    }

    const invoke = vi.fn(async (_req: unknown) => plateDescribe);
    const tauri = createCoreApi(createTauriCoreClient(invoke));
    expect(tauri.verbatimDocuments).toBe(false);
    await tauri.describe(doc);
    expect(invoke).toHaveBeenCalledWith({ op: 'describe', document: doc.value });
  });
});
