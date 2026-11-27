import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { App } from './App';
import type { ApiClient } from './api/client';
import { CoreTransportError, type CoreClient, type CoreRequest } from './api/coreClient';
import type { CapabilityResponse, JobManifest } from './api/types';
import { plateDescribe, plateValidate } from './test/fixtures';

const capabilities: CapabilityResponse = {
  planners: ['auto'],
  base_features: ['plate'],
  hole_types: ['through'],
  formats: ['stl', 'svg', 'json'],
  cadquery_available: true,
  openscad_available: false,
  configured_planner_mode: 'auto',
  configured_render_backend: 'cadquery',
};

function mockApi(): ApiClient {
  return {
    health: vi.fn(async () => ({ status: 'ok', version: '0.1.0' })),
    capabilities: vi.fn(async () => capabilities),
    plan: vi.fn(),
    generate: vi.fn(),
    generateFromSpec: vi.fn(),
    generateFromDocument: vi.fn(),
    validate: vi.fn(async () => ({ valid: true, review_required: false, issues: [] })),
    listJobs: vi.fn(async () => []),
    getJob: vi.fn(),
    fetchArtifact: vi.fn(),
    bundleUrl: (id: string) => `/api/v1/jobs/${id}/bundle.zip`,
  };
}

function setup(core: CoreClient) {
  const api = mockApi();
  const createCore = vi.fn(() => core);
  render(
    <App
      backend={{ mode: 'browser', baseUrl: '', token: null }}
      createClient={() => api}
      createCore={createCore}
    />,
  );
  return api;
}

const DOC_2_0 = {
  schema_version: '2.0',
  name: 'plate-four-holes',
  expected_resolved: { note: 'example only' },
};

function paste(value: unknown) {
  fireEvent.change(screen.getByLabelText('CAD DSL JSON'), {
    target: { value: typeof value === 'string' ? value : JSON.stringify(value) },
  });
}

describe('App with the Rust core', () => {
  it('validates a 2.0 document through the core client, shows the tree and enables generate', async () => {
    const request = vi.fn(async (req: CoreRequest) =>
      req.op === 'describe' ? plateDescribe : plateValidate,
    );
    const api = setup({ request });
    paste(DOC_2_0);

    const regenerate = screen.getByRole('button', { name: '套用 JSON 並重新輸出' });
    expect(regenerate).toBeEnabled();
    expect(screen.queryByTestId('no-generate-2')).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: '驗證 JSON' }));

    await screen.findByRole('region', { name: '元件樹' });
    expect(api.validate).not.toHaveBeenCalled();
    expect(request.mock.calls.map(([req]) => req.op)).toEqual(['validate', 'describe']);
    // example-only key is stripped before it reaches the core
    expect(request.mock.calls[0]![0]).toEqual({
      op: 'validate',
      document: { schema_version: '2.0', name: 'plate-four-holes' },
    });
    expect(screen.getByTestId('status')).toHaveTextContent('驗證通過（2.0）');
    expect(screen.getByRole('region', { name: '驗證報告' })).toHaveTextContent('有效');
    expect(api.generateFromSpec).not.toHaveBeenCalled();
  });

  it('shows 2.0 issue fields (target, related, owner, layer) in the validation panel', async () => {
    const report = {
      valid: false,
      review_required: true,
      issues: [
        {
          severity: 'error',
          code: 'ports_unconnected',
          message: '兩個元件未連接',
          feature_index: null,
          target: 'v_lid',
          related: ['v_body'],
          owner: 'joiner',
          layer: 'analytic',
        },
      ],
    };
    const request = vi.fn(async (req: CoreRequest) =>
      req.op === 'validate'
        ? {
            ...plateValidate,
            result: { ...plateValidate.result, report },
          }
        : plateDescribe,
    );
    setup({ request: request as unknown as CoreClient['request'] });
    paste(DOC_2_0);
    fireEvent.click(screen.getByRole('button', { name: '驗證 JSON' }));

    const panel = await screen.findByRole('region', { name: '驗證報告' });
    await waitFor(() => expect(panel).toHaveTextContent('目標：v_lid'));
    expect(panel).toHaveTextContent('相關：v_body');
    expect(panel).toHaveTextContent('負責：joiner');
    expect(panel).toHaveTextContent('層級：解析層');
  });

  it('shows a core error envelope as an invalid report without a tree', async () => {
    const request = vi.fn(async () => ({
      status: 'error' as const,
      code: 'invalid_document' as const,
      message: 'invalid JSON document',
      path: '/document',
      violations: [],
    }));
    setup({ request });
    paste(DOC_2_0);
    fireEvent.click(screen.getByRole('button', { name: '驗證 JSON' }));

    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('2.0 文件無法處理'));
    expect(screen.getByRole('region', { name: '驗證報告' })).toHaveTextContent('/document');
    expect(screen.queryByRole('region', { name: '元件樹' })).not.toBeInTheDocument();
  });

  it('shows a non-blocking message when the core is unavailable and keeps 1.0 usable', async () => {
    const request = vi.fn(async (): Promise<never> => {
      throw new CoreTransportError('unavailable', '伺服器未安裝 Rust core（HTTP 503）。', 503);
    });
    const api = setup({ request });
    paste(DOC_2_0);
    fireEvent.click(screen.getByRole('button', { name: '驗證 JSON' }));

    const note = await screen.findByTestId('core-note');
    expect(note).toHaveTextContent('伺服器未安裝 Rust core');
    expect(note).toHaveTextContent('1.0 文件的驗證與輸出不受影響');

    paste({ schema_version: '1.0', source_prompt: 'x' });
    fireEvent.click(screen.getByRole('button', { name: '驗證 JSON' }));
    await waitFor(() => expect(api.validate).toHaveBeenCalledTimes(1));
    expect(screen.getByRole('button', { name: '套用 JSON 並重新輸出' })).toBeEnabled();
  });

  it('keeps using /validate for a 1.0 document and never calls the core', async () => {
    const request = vi.fn(async () => plateValidate);
    const api = setup({ request });
    const spec = { schema_version: '1.0', source_prompt: 'x', planner: { planner: 'manual' } };
    paste(spec);
    fireEvent.click(screen.getByRole('button', { name: '驗證 JSON' }));

    await waitFor(() => expect(api.validate).toHaveBeenCalledWith(spec));
    expect(request).not.toHaveBeenCalled();
    expect(screen.queryByTestId('no-generate-2')).not.toBeInTheDocument();
    expect(screen.queryByRole('region', { name: '元件樹' })).not.toBeInTheDocument();
  });

  it('loads the inline 2.0 sample into the editor', () => {
    setup({ request: vi.fn() });
    fireEvent.click(screen.getByRole('button', { name: '載入 2.0 範例' }));
    const text = (screen.getByLabelText('CAD DSL JSON') as HTMLTextAreaElement).value;
    expect(JSON.parse(text)).toMatchObject({ schema_version: '2.0' });
    expect(screen.getByRole('button', { name: '套用 JSON 並重新輸出' })).toBeEnabled();
  });
  it('keeps a successful validate result when describe fails (transport error)', async () => {
    const request = vi.fn(async (req: CoreRequest) => {
      if (req.op === 'describe') {
        throw new CoreTransportError('core_failed', 'Rust core 執行失敗或輸出異常（HTTP 502）。', 502);
      }
      return plateValidate;
    });
    setup({ request });
    paste(DOC_2_0);
    fireEvent.click(screen.getByRole('button', { name: '驗證 JSON' }));

    const note = await screen.findByTestId('tree-note');
    expect(note).toHaveTextContent('無法顯示元件樹');
    expect(note).toHaveTextContent('HTTP 502');
    expect(request.mock.calls.map(([req]) => req.op)).toEqual(['validate', 'describe']);
    expect(screen.getByTestId('status')).toHaveTextContent('驗證通過（2.0）');
    expect(screen.getByTestId('status')).not.toHaveClass('error');
    expect(screen.getByRole('region', { name: '驗證報告' })).toHaveTextContent('有效');
    expect(screen.queryByTestId('core-note')).not.toBeInTheDocument();
    expect(screen.queryByRole('region', { name: '元件樹' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: '驗證 JSON' })).toBeEnabled();
  });

  it('keeps a successful validate result when describe returns an error envelope', async () => {
    const request = vi.fn(async (req: CoreRequest) =>
      req.op === 'describe'
        ? {
            status: 'error' as const,
            code: 'response_too_large' as const,
            message: 'the response exceeds the limit',
            path: null,
            violations: [],
          }
        : plateValidate,
    );
    setup({ request });
    paste(DOC_2_0);
    fireEvent.click(screen.getByRole('button', { name: '驗證 JSON' }));

    const note = await screen.findByTestId('tree-note');
    expect(note).toHaveTextContent('the response exceeds the limit');
    expect(screen.getByTestId('status')).toHaveTextContent('驗證通過（2.0）');
    // the note belongs to that text: editing the document hides it
    paste({ ...DOC_2_0, name: 'changed' });
    expect(screen.queryByTestId('tree-note')).not.toBeInTheDocument();
  });

  it('does not call describe when validate returns an error envelope', async () => {
    const request = vi.fn(async () => ({
      status: 'error' as const,
      code: 'schema_violation' as const,
      message: '1 schema violation(s)',
      path: '/document/revision',
      violations: [{ path: '/document/revision', message: 'bad' }],
    }));
    setup({ request });
    paste(DOC_2_0);
    fireEvent.click(screen.getByRole('button', { name: '驗證 JSON' }));

    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('2.0 文件無法處理'));
    expect(request).toHaveBeenCalledTimes(1);
    expect(screen.queryByTestId('tree-note')).not.toBeInTheDocument();
  });

  const BIG = '{"schema_version": "2.0", "name": "x", "revision": 18446744073709551615, "z": -0}';

  it('sends editor text verbatim on a verbatim transport (no precision warning)', async () => {
    const request = vi.fn(async (req: CoreRequest, _options?: unknown) =>
      req.op === 'describe' ? plateDescribe : plateValidate,
    );
    setup({ request, verbatimDocuments: true });
    paste(BIG);
    fireEvent.click(screen.getByRole('button', { name: '驗證 JSON' }));

    await screen.findByRole('region', { name: '元件樹' });
    expect(request.mock.calls[0]![1]).toEqual({ documentText: BIG });
    expect(request.mock.calls[1]![1]).toEqual({ documentText: BIG });
    expect(screen.queryByRole('list', { name: '輸入警告' })).not.toBeInTheDocument();
  });

  it('warns in the validation panel when the transport would alter numbers (Tauri)', async () => {
    const request = vi.fn(async (req: CoreRequest) =>
      req.op === 'describe' ? plateDescribe : plateValidate,
    );
    setup({ request, verbatimDocuments: false });
    paste(BIG);
    fireEvent.click(screen.getByRole('button', { name: '驗證 JSON' }));

    const warnings = await screen.findByRole('list', { name: '輸入警告' });
    expect(warnings).toHaveTextContent('無法精確保留');
    expect(warnings).toHaveTextContent('/revision：18446744073709551615 → 18446744073709552000');
    expect(warnings).toHaveTextContent('/z：-0 → 0');
    // still validated (with the altered values) and the report is shown alongside
    expect(screen.getByRole('region', { name: '驗證報告' })).toHaveTextContent('有效');

    // a document without such numbers has no warning
    paste({ schema_version: '2.0', name: 'x', revision: 3 });
    fireEvent.click(screen.getByRole('button', { name: '驗證 JSON' }));
    await waitFor(() =>
      expect(screen.queryByRole('list', { name: '輸入警告' })).not.toBeInTheDocument(),
    );
  });

  const JOB_ID = '0123456789abcdef0123456789abcdef';
  const manifest2 = (over: Partial<JobManifest> = {}): JobManifest => ({
    job_id: JOB_ID,
    status: 'source_only',
    created_at: '2026-11-27T00:00:00Z',
    prompt: 'x',
    planner_used: 'manual-dsl',
    renderer_used: 'cadquery',
    requested_formats: ['stl'],
    spec_version: '2.0',
    document: { schema_version: '2.0', name: 'canonical-plate' },
    validation: { valid: true, review_required: false, issues: [] },
    artifacts: [],
    warnings: [],
    error: null,
    ...over,
  });

  it('generates from a 2.0 document with the parsed document and loads the canonical one', async () => {
    const request = vi.fn(async () => plateValidate);
    const api = setup({ request });
    vi.mocked(api.generateFromDocument).mockResolvedValue(manifest2());
    paste(DOC_2_0);

    fireEvent.click(screen.getByRole('button', { name: '套用 JSON 並重新輸出' }));

    await waitFor(() => expect(api.generateFromDocument).toHaveBeenCalledTimes(1));
    expect(api.generateFromDocument).toHaveBeenCalledWith({
      document: { schema_version: '2.0', name: 'plate-four-holes' },
      formats: expect.any(Array),
      render: expect.anything(),
    });
    expect(api.generateFromSpec).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(JSON.parse((screen.getByLabelText('CAD DSL JSON') as HTMLTextAreaElement).value)).toEqual({
        schema_version: '2.0',
        name: 'canonical-plate',
      }),
    );
    expect(request).not.toHaveBeenCalled();
  });

  it('surfaces the error of a failed (compile-unsupported) 2.0 job', async () => {
    const api = setup({ request: vi.fn() });
    vi.mocked(api.generateFromDocument).mockResolvedValue(
      manifest2({
        status: 'failed',
        renderer_used: 'compile-unsupported',
        error: '不支援的結構：extrude',
      }),
    );
    paste(DOC_2_0);
    fireEvent.click(screen.getByRole('button', { name: '套用 JSON 並重新輸出' }));

    await waitFor(() =>
      expect(screen.getByTestId('status')).toHaveTextContent('不支援的結構：extrude'),
    );
    expect(screen.getByTestId('status')).toHaveTextContent('compile-unsupported');
  });

  it('warns about numbers JSON.stringify would alter when generating from 2.0', async () => {
    const api = setup({ request: vi.fn(), verbatimDocuments: true });
    vi.mocked(api.generateFromDocument).mockResolvedValue(manifest2());
    paste(BIG);
    fireEvent.click(screen.getByRole('button', { name: '套用 JSON 並重新輸出' }));

    const warnings = await screen.findByRole('list', { name: '輸入警告' });
    expect(warnings).toHaveTextContent('/revision：18446744073709551615 → 18446744073709552000');
  });
});
