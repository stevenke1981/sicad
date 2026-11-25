import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { App } from './App';
import type { ApiClient } from './api/client';
import type { CapabilityResponse, JobListItem, JobManifest } from './api/types';

const JOB_ID = '0123456789abcdef0123456789abcdef';

const capabilities: CapabilityResponse = {
  planners: ['auto', 'rule', 'llm'],
  base_features: ['plate', 'cylinder', 'ring', 'l_bracket', 'enclosure'],
  hole_types: ['through'],
  formats: ['step', 'stl', 'dxf', 'svg', 'py', 'scad', 'json'],
  cadquery_available: true,
  openscad_available: false,
  configured_planner_mode: 'auto',
  configured_render_backend: 'cadquery',
};

const manifest: JobManifest = {
  job_id: JOB_ID,
  status: 'completed',
  created_at: '2026-11-26T00:00:00Z',
  prompt: '做一個墊圈',
  planner_used: 'rule',
  renderer_used: 'cadquery',
  requested_formats: ['stl', 'svg', 'json'],
  spec: {
    schema_version: '1.0',
    name: 'washer-30',
    source_prompt: '做一個墊圈',
    unit: 'mm',
    base: { kind: 'ring', outer_diameter: 30, inner_diameter: 15, height: 5 },
    planner: { planner: 'rule', confidence: 1, review_required: false },
  },
  validation: {
    valid: true,
    review_required: false,
    issues: [{ severity: 'info', code: 'assumption', message: '假設材料為鋁合金' }],
  },
  artifacts: [
    { filename: 'preview.svg', media_type: 'image/svg+xml', size: 1200, url: `/api/v1/jobs/${JOB_ID}/files/preview.svg` },
    { filename: 'model.stl', media_type: 'model/stl', size: 4096, url: `/api/v1/jobs/${JOB_ID}/files/model.stl` },
    { filename: 'spec.json', media_type: 'application/json', size: 300, url: `/api/v1/jobs/${JOB_ID}/files/spec.json` },
  ],
  warnings: ['STEP 未輸出'],
  error: null,
};

const jobItem: JobListItem = {
  job_id: JOB_ID,
  status: 'completed',
  created_at: manifest.created_at,
  prompt: manifest.prompt,
  name: 'washer-30',
  renderer_used: 'cadquery',
};

function mockClient(overrides: Partial<ApiClient> = {}): ApiClient {
  return {
    health: vi.fn(async () => ({ status: 'ok', version: '0.1.0' })),
    capabilities: vi.fn(async () => capabilities),
    plan: vi.fn(async () => {
      throw new Error('not used');
    }),
    generate: vi.fn(async () => manifest),
    generateFromSpec: vi.fn(async () => manifest),
    validate: vi.fn(async () => ({ valid: true, review_required: false, issues: [] })),
    listJobs: vi.fn<ApiClient['listJobs']>().mockResolvedValueOnce([]).mockResolvedValue([jobItem]),
    getJob: vi.fn(async () => manifest),
    fetchArtifact: vi.fn(async (url: string) => new Blob([url.endsWith('.svg') ? '<svg/>' : 'solid x'])),
    bundleUrl: (jobId: string) => `/api/v1/jobs/${jobId}/bundle.zip`,
    ...overrides,
  };
}

function renderApp(client: ApiClient) {
  const createClient = vi.fn(() => client);
  render(<App backend={{ mode: 'browser', baseUrl: '', token: null }} createClient={createClient} />);
  return createClient;
}

describe('App', () => {
  it('loads capabilities and runs the generate flow, showing status and artifacts', async () => {
    const client = mockClient();
    renderApp(client);
    const user = userEvent.setup();

    await waitFor(() =>
      expect(screen.getByTestId('capabilities')).toHaveTextContent('CadQuery：可用'),
    );
    expect(await screen.findByText('尚無工作。')).toBeInTheDocument();

    await user.selectOptions(screen.getByLabelText('規劃器'), 'rule');
    await user.click(screen.getByRole('checkbox', { name: /STEP/ }));
    await user.click(screen.getByRole('button', { name: '產生 CAD' }));

    await waitFor(() =>
      expect(screen.getByTestId('status')).toHaveTextContent('完成：cadquery / completed'),
    );
    expect(client.generate).toHaveBeenCalledWith({
      prompt: expect.stringContaining('固定板'),
      planner: 'rule',
      formats: ['stl', 'dxf', 'svg', 'py', 'scad', 'json'],
      render: true,
    });
    expect(screen.getByTestId('badge')).toHaveTextContent('COMPLETED');
    expect(screen.getByRole('heading', { name: 'washer-30' })).toBeInTheDocument();

    const meta = screen.getByTestId('job-meta');
    expect(within(meta).getByText('rule')).toBeInTheDocument();
    expect(within(meta).getByText('cadquery')).toBeInTheDocument();

    const downloads = screen.getByLabelText('輸出檔案');
    for (const name of ['preview.svg', 'model.stl', 'spec.json', '全部打包 ZIP']) {
      expect(within(downloads).getByRole('button', { name: new RegExp(name) })).toBeInTheDocument();
    }
    expect(screen.getByText('假設材料為鋁合金')).toBeInTheDocument();
    expect(screen.getByText('STEP 未輸出')).toBeInTheDocument();

    // SVG preview is fetched through the client and shown via <img> (blob URL).
    const img = await screen.findByRole('img', { name: /preview\.svg/ });
    expect(img.getAttribute('src')).toMatch(/^blob:/);
    expect(client.fetchArtifact).toHaveBeenCalledWith(`/api/v1/jobs/${JOB_ID}/files/preview.svg`);

    // 3D panel explains that WebGL is unavailable in jsdom.
    await user.click(screen.getByRole('tab', { name: '3D 檢視' }));
    expect(within(screen.getByTestId('stl-viewer')).getByRole('status')).toHaveTextContent('WebGL');

    // History refreshed after the job.
    expect(await screen.findByRole('button', { name: /washer-30/ })).toBeInTheDocument();
    expect((screen.getByLabelText('CAD DSL JSON') as HTMLTextAreaElement).value).toContain(
      '"outer_diameter": 30',
    );
  });

  it('shows the API error detail when generate fails', async () => {
    const client = mockClient({
      generate: vi.fn(async () => {
        throw new Error('Invalid API token');
      }),
    });
    renderApp(client);
    fireEvent.click(screen.getByRole('button', { name: '產生 CAD' }));
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('Invalid API token'));
    expect(screen.getByTestId('badge')).toHaveTextContent('FAILED');
  });

  it('validates and regenerates from the edited JSON spec', async () => {
    const client = mockClient({
      validate: vi.fn(async () => ({
        valid: false,
        review_required: true,
        issues: [{ severity: 'error' as const, code: 'hole_outside', message: '孔超出邊界', feature_index: 0 }],
      })),
    });
    renderApp(client);
    const editor = screen.getByLabelText('CAD DSL JSON');
    fireEvent.change(editor, { target: { value: JSON.stringify(manifest.spec) } });

    fireEvent.click(screen.getByRole('button', { name: '驗證 JSON' }));
    await waitFor(() => expect(screen.getByText('孔超出邊界')).toBeInTheDocument());
    expect(screen.getByText('JSON 驗證結果')).toBeInTheDocument();
    expect(client.validate).toHaveBeenCalledWith(manifest.spec);

    fireEvent.click(screen.getByRole('button', { name: '套用 JSON 並重新輸出' }));
    await waitFor(() =>
      expect(screen.getByTestId('status')).toHaveTextContent('完成：cadquery / completed'),
    );
    expect(client.generateFromSpec).toHaveBeenCalledWith({
      spec: manifest.spec,
      formats: ['step', 'stl', 'dxf', 'svg', 'py', 'scad', 'json'],
      render: true,
    });

    fireEvent.change(editor, { target: { value: '{oops' } });
    fireEvent.click(screen.getByRole('button', { name: '驗證 JSON' }));
    expect(screen.getByTestId('status')).toHaveTextContent('JSON 格式錯誤');
  });

  it('opens a job from history and downloads the bundle through the client', async () => {
    const client = mockClient({ listJobs: vi.fn(async () => [jobItem]) });
    const click = vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {});
    renderApp(client);

    fireEvent.click(await screen.findByRole('button', { name: /washer-30/ }));
    await waitFor(() => expect(client.getJob).toHaveBeenCalledWith(JOB_ID));

    fireEvent.click(await screen.findByRole('button', { name: '全部打包 ZIP' }));
    await waitFor(() =>
      expect(screen.getByTestId('status')).toHaveTextContent(`已下載 promptcad-${JOB_ID}.zip`),
    );
    expect(client.fetchArtifact).toHaveBeenCalledWith(`/api/v1/jobs/${JOB_ID}/bundle.zip`);
    expect(click).toHaveBeenCalled();
  });
});
