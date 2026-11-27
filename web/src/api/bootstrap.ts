// The ONLY module allowed to touch the Tauri global (ADR 0002 / 0003). Everything else
// receives a plain BackendInfo and talks HTTP through ApiClient.

export interface BackendInfo {
  /** `tauri`: desktop shell supplied URL + one-time token; `browser`: same origin. */
  mode: 'tauri' | 'browser';
  /** Backend origin; empty string means same origin as the page. */
  baseUrl: string;
  /** Token from the shell (Tauri) or null (browser: user types it in the UI). */
  token: string | null;
  /**
   * Tauri only: sends one CoreRequest object to the in-process Rust core (`core_request`
   * command) and resolves to the CoreResponse object. A rejection is a host failure, not a
   * domain error. `coreClient.ts` consumes this so it never touches the Tauri global.
   */
  coreInvoke?: (request: unknown) => Promise<unknown>;
}

interface TauriBackendInfo {
  base_url: string;
  token: string;
}

interface TauriGlobal {
  core?: { invoke?: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T> };
}

type BootstrapWindow = { __TAURI__?: TauriGlobal };

export async function resolveBackend(win: BootstrapWindow = window as BootstrapWindow): Promise<BackendInfo> {
  const tauri = win.__TAURI__;
  if (!tauri) {
    return { mode: 'browser', baseUrl: '', token: null };
  }
  const invoke = tauri.core?.invoke;
  if (typeof invoke !== 'function') {
    throw new Error('Tauri 環境缺少 core.invoke（需要 withGlobalTauri）。');
  }
  const info = await invoke<TauriBackendInfo>('backend_info');
  if (!info || typeof info.base_url !== 'string' || !info.base_url) {
    throw new Error('backend_info 回傳格式錯誤。');
  }
  return {
    mode: 'tauri',
    baseUrl: info.base_url.replace(/\/+$/, ''),
    token: typeof info.token === 'string' && info.token ? info.token : null,
    coreInvoke: (request) => invoke<unknown>('core_request', { request }),
  };
}
