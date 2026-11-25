import { describe, expect, it, vi } from 'vitest';
import { resolveBackend } from './bootstrap';

describe('resolveBackend', () => {
  it('uses invoke("backend_info") when window.__TAURI__ is present', async () => {
    const invoke = vi.fn(async () => ({ base_url: 'http://127.0.0.1:50123/', token: 'tok-123' }));
    const fakeWindow = { __TAURI__: { core: { invoke } } } as unknown as Parameters<
      typeof resolveBackend
    >[0];

    const backend = await resolveBackend(fakeWindow);

    expect(invoke).toHaveBeenCalledWith('backend_info');
    expect(backend).toEqual({
      mode: 'tauri',
      baseUrl: 'http://127.0.0.1:50123',
      token: 'tok-123',
    });
  });

  it('falls back to same-origin with no token in a plain browser', async () => {
    const backend = await resolveBackend({});
    expect(backend).toEqual({ mode: 'browser', baseUrl: '', token: null });
  });

  it('defaults to the real window (jsdom has no __TAURI__)', async () => {
    await expect(resolveBackend()).resolves.toMatchObject({ mode: 'browser', baseUrl: '' });
  });

  it('rejects a malformed backend_info reply', async () => {
    const invoke = vi.fn(async () => ({ token: 'x' }));
    const fakeWindow = { __TAURI__: { core: { invoke } } } as unknown as Parameters<
      typeof resolveBackend
    >[0];
    await expect(resolveBackend(fakeWindow)).rejects.toThrow('backend_info');
  });
});
