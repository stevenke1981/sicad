import { describe, expect, it, vi } from 'vitest';
import { ApiError } from './client';
import { createHttpClient, errorMessageFromBody } from './httpClient';

function jsonResponse(body: unknown, status = 200, statusText = 'OK'): Response {
  return new Response(JSON.stringify(body), {
    status,
    statusText,
    headers: { 'Content-Type': 'application/json' },
  });
}

function headerOf(init: RequestInit | undefined, name: string): string | undefined {
  return (init?.headers as Record<string, string> | undefined)?.[name];
}

describe('createHttpClient', () => {
  it('sends Authorization: Bearer <token> on JSON and artifact requests', async () => {
    const fetchMock = vi.fn<typeof fetch>(async (input) =>
      String(input).endsWith('.stl') ? new Response('solid x\nendsolid x\n') : jsonResponse([]),
    );
    const client = createHttpClient({
      baseUrl: 'http://127.0.0.1:51234',
      token: 'secret-token',
      fetch: fetchMock,
    });

    await client.listJobs();
    await client.fetchArtifact('/api/v1/jobs/abc/files/model.stl');

    expect(fetchMock).toHaveBeenCalledTimes(2);
    const [url, init] = fetchMock.mock.calls[0]!;
    expect(url).toBe('http://127.0.0.1:51234/api/v1/jobs');
    expect(headerOf(init, 'Authorization')).toBe('Bearer secret-token');
    const [artifactUrl, artifactInit] = fetchMock.mock.calls[1]!;
    expect(artifactUrl).toBe('http://127.0.0.1:51234/api/v1/jobs/abc/files/model.stl');
    expect(headerOf(artifactInit, 'Authorization')).toBe('Bearer secret-token');
  });

  it('omits Authorization when no token is configured and posts JSON bodies', async () => {
    const fetchMock = vi.fn<typeof fetch>(async () =>
      jsonResponse({ valid: true, review_required: false, issues: [] }),
    );
    const client = createHttpClient({ baseUrl: '', token: '  ', fetch: fetchMock });

    await client.validate({
      source_prompt: 'x',
      base: { kind: 'plate', length: 1, width: 1, thickness: 1 },
      planner: { planner: 'manual' },
    });

    const [url, init] = fetchMock.mock.calls[0]!;
    expect(url).toBe('/api/v1/validate');
    expect(init?.method).toBe('POST');
    expect(headerOf(init, 'Authorization')).toBeUndefined();
    expect(headerOf(init, 'Content-Type')).toBe('application/json');
    expect(JSON.parse(String(init?.body))).toMatchObject({ source_prompt: 'x' });
  });

  it('posts a 2.0 document to /generate-from-document', async () => {
    const fetchMock = vi.fn<typeof fetch>(async () => jsonResponse({ job_id: 'j' }));
    const client = createHttpClient({ baseUrl: '', fetch: fetchMock });
    const document = { schema_version: '2.0', name: 'p' };

    await client.generateFromDocument({ document, formats: ['stl'], render: true });

    const [url, init] = fetchMock.mock.calls[0]!;
    expect(url).toBe('/api/v1/generate-from-document');
    expect(init?.method).toBe('POST');
    expect(JSON.parse(String(init?.body))).toEqual({ document, formats: ['stl'], render: true });
  });

  it('maps non-2xx responses to an ApiError carrying FastAPI {detail}', async () => {
    const fetchMock = vi.fn<typeof fetch>(async () =>
      jsonResponse({ detail: 'Invalid API token' }, 401, 'Unauthorized'),
    );
    const client = createHttpClient({ baseUrl: '', token: 'bad', fetch: fetchMock });

    const error = await client.capabilities().catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toBeInstanceOf(Error);
    expect((error as ApiError).message).toBe('Invalid API token');
    expect((error as ApiError).status).toBe(401);
  });

  it('formats 422 validation detail arrays and falls back to the status line', async () => {
    const fetchMock = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(
        jsonResponse(
          { detail: [{ loc: ['body', 'prompt'], msg: 'String should have at least 3 characters' }] },
          422,
          'Unprocessable Entity',
        ),
      )
      .mockResolvedValueOnce(new Response('<html>oops</html>', { status: 502, statusText: 'Bad Gateway' }));
    const client = createHttpClient({ baseUrl: '', fetch: fetchMock });

    await expect(client.generate({ prompt: 'x' })).rejects.toThrow(
      'prompt: String should have at least 3 characters',
    );
    await expect(client.getJob('a'.repeat(32))).rejects.toThrow('502 Bad Gateway');
  });

  it('refuses to send the token to a different origin', async () => {
    const fetchMock = vi.fn<typeof fetch>();
    const client = createHttpClient({ baseUrl: 'http://127.0.0.1:9', token: 't', fetch: fetchMock });
    await expect(client.fetchArtifact('https://evil.example/x')).rejects.toBeInstanceOf(ApiError);
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('builds the bundle URL under the jobs path', () => {
    const client = createHttpClient({ baseUrl: '' });
    expect(client.bundleUrl('0123')).toBe('/api/v1/jobs/0123/bundle.zip');
  });
});

describe('errorMessageFromBody', () => {
  it('uses fallback for bodies without detail', () => {
    expect(errorMessageFromBody({ message: 'x' }, '500 Internal Server Error')).toBe(
      '500 Internal Server Error',
    );
    expect(errorMessageFromBody(null, 'fallback')).toBe('fallback');
  });
});
