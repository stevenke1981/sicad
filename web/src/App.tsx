import { useCallback, useEffect, useMemo, useState } from 'react';
import type { BackendInfo } from './api/bootstrap';
import type { ApiClient } from './api/client';
import { createHttpClient, type HttpClientOptions } from './api/httpClient';
import type {
  Artifact,
  CadDocument,
  CapabilityResponse,
  JobListItem,
  JobManifest,
  OutputFormat,
  PlannerChoice,
  ValidationReport,
} from './api/types';
import { ArtifactList } from './components/ArtifactList';
import { JobHistory } from './components/JobHistory';
import { StlViewer } from './components/StlViewer';
import { SvgPreview } from './components/SvgPreview';
import { ValidationPanel } from './components/ValidationPanel';
import { saveBlob } from './download';

export interface AppProps {
  backend: BackendInfo;
  /** Injected for tests; defaults to the HTTP client. */
  createClient?: (options: HttpClientOptions) => ApiClient;
}

const DEFAULT_PROMPT =
  '畫一個鋁合金固定板，長120mm、寬60mm、厚10mm，四角各一個 M6 通孔，孔中心離邊緣10mm，四角 R5。';

const FORMAT_LABELS: Record<OutputFormat, string> = {
  step: 'STEP',
  stl: 'STL',
  dxf: 'DXF',
  svg: 'SVG',
  py: 'CadQuery',
  scad: 'OpenSCAD',
  json: 'JSON',
};
const ALL_FORMATS = Object.keys(FORMAT_LABELS) as OutputFormat[];

const PLANNERS: { value: PlannerChoice; label: string }[] = [
  { value: 'auto', label: '自動' },
  { value: 'rule', label: '本地規則' },
  { value: 'llm', label: 'LLM' },
];

const EXAMPLES: { label: string; prompt: string }[] = [
  { label: '墊圈', prompt: '做一個墊圈，外徑30mm、內徑15mm、厚5mm。' },
  { label: '外殼', prompt: '做一個100x70x30mm的開口盒，壁厚2mm，底部四角3mm通孔。' },
  {
    label: 'L 支架',
    prompt:
      'Create an L bracket 80 mm wide, 50 mm deep, 60 mm tall, 4 mm thick, with two 5 mm holes.',
  },
];

type BadgeKind = 'neutral' | 'ok' | 'warn' | 'fail';
type ViewTab = '2d' | '3d';

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function findArtifact(manifest: JobManifest | null, ...names: string[]): Artifact | null {
  const artifacts = manifest?.artifacts ?? [];
  for (const name of names) {
    const found = artifacts.find((item) => item.filename === name);
    if (found) return found;
  }
  return null;
}

export function App({ backend, createClient = createHttpClient }: AppProps) {
  const isTauri = backend.mode === 'tauri';
  const [tokenDraft, setTokenDraft] = useState('');
  const [token, setToken] = useState('');
  const client = useMemo(
    () =>
      createClient({ baseUrl: backend.baseUrl, token: isTauri ? backend.token : token || null }),
    [createClient, backend.baseUrl, backend.token, isTauri, token],
  );

  const [prompt, setPrompt] = useState(DEFAULT_PROMPT);
  const [planner, setPlanner] = useState<PlannerChoice>('auto');
  const [formats, setFormats] = useState<Set<OutputFormat>>(() => new Set(ALL_FORMATS));
  const [render, setRender] = useState(true);

  const [status, setStatus] = useState<{ text: string; error: boolean }>({
    text: '準備完成。',
    error: false,
  });
  const [badge, setBadge] = useState<{ text: string; kind: BadgeKind }>({
    text: 'IDLE',
    kind: 'neutral',
  });
  const [busy, setBusy] = useState(false);

  const [manifest, setManifest] = useState<JobManifest | null>(null);
  const [specText, setSpecText] = useState('{}');
  const [report, setReport] = useState<{ source: 'job' | 'validate'; value: ValidationReport } | null>(
    null,
  );
  const [tab, setTab] = useState<ViewTab>('2d');

  const [capabilities, setCapabilities] = useState<CapabilityResponse | null>(null);
  const [capabilitiesError, setCapabilitiesError] = useState<string | null>(null);
  const [jobs, setJobs] = useState<JobListItem[] | null>(null);
  const [jobsError, setJobsError] = useState<string | null>(null);

  const say = (text: string, error = false) => setStatus({ text, error });

  const loadJobs = useCallback(async () => {
    try {
      setJobs(await client.listJobs());
      setJobsError(null);
    } catch (error) {
      setJobsError(errorText(error));
    }
  }, [client]);

  useEffect(() => {
    let cancelled = false;
    client
      .capabilities()
      .then((caps) => {
        if (cancelled) return;
        setCapabilities(caps);
        setCapabilitiesError(null);
      })
      .catch((error: unknown) => {
        if (!cancelled) setCapabilitiesError(errorText(error));
      });
    void loadJobs();
    return () => {
      cancelled = true;
    };
  }, [client, loadJobs]);

  const availableFormats = useMemo(
    () =>
      capabilities
        ? ALL_FORMATS.filter((format) => capabilities.formats.includes(format))
        : ALL_FORMATS,
    [capabilities],
  );
  const selectedFormats = availableFormats.filter((format) => formats.has(format));

  const showManifest = (data: JobManifest) => {
    setManifest(data);
    setSpecText(JSON.stringify(data.spec, null, 2));
    setReport({ source: 'job', value: data.validation });
    setBadge({
      text: data.validation.review_required ? 'REVIEW' : data.status.toUpperCase(),
      kind:
        data.status === 'failed' ? 'fail' : data.validation.review_required ? 'warn' : 'ok',
    });
  };

  const runJob = async (label: string, run: () => Promise<JobManifest>) => {
    setBusy(true);
    say(label);
    setBadge({ text: 'RUNNING', kind: 'neutral' });
    try {
      const data = await run();
      showManifest(data);
      say(`完成：${data.renderer_used} / ${data.status}`, data.status === 'failed');
      await loadJobs();
    } catch (error) {
      say(errorText(error), true);
      setBadge({ text: 'FAILED', kind: 'fail' });
    } finally {
      setBusy(false);
    }
  };

  const onGenerate = () => {
    const text = prompt.trim();
    if (text.length < 3) return say('請先輸入零件描述。', true);
    if (!selectedFormats.length) return say('請至少選擇一種輸出格式。', true);
    void runJob('正在解析、驗證並建立 CAD…', () =>
      client.generate({ prompt: text, planner, formats: selectedFormats, render }),
    );
  };

  const parseSpec = (): CadDocument | null => {
    try {
      const parsed: unknown = JSON.parse(specText);
      if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) {
        say('JSON 格式錯誤：最上層必須是物件。', true);
        return null;
      }
      // Shape is checked by the server (extra="forbid"); the client never re-implements rules.
      return parsed as CadDocument;
    } catch (error) {
      say(`JSON 格式錯誤：${errorText(error)}`, true);
      return null;
    }
  };

  const onValidate = async () => {
    const spec = parseSpec();
    if (!spec) return;
    setBusy(true);
    say('正在驗證 JSON…');
    try {
      const value = await client.validate(spec);
      setReport({ source: 'validate', value });
      const issues = value.issues ?? [];
      say(
        value.valid
          ? `驗證通過${issues.length ? `（${issues.length} 項提示）` : ''}。`
          : `驗證未通過：${issues.filter((i) => i.severity === 'error').length} 個錯誤。`,
        !value.valid,
      );
    } catch (error) {
      say(errorText(error), true);
    } finally {
      setBusy(false);
    }
  };

  const onRegenerate = () => {
    if (!selectedFormats.length) return say('請至少選擇一種輸出格式。', true);
    const spec = parseSpec();
    if (!spec) return;
    void runJob('正在驗證修改後的 DSL 並重新建立 CAD…', () =>
      client.generateFromSpec({ spec, formats: selectedFormats, render }),
    );
  };

  const onOpenJob = async (jobId: string) => {
    try {
      showManifest(await client.getJob(jobId));
      say(`已開啟工作 ${jobId}`);
      window.scrollTo?.({ top: 0, behavior: 'smooth' });
    } catch (error) {
      say(errorText(error), true);
    }
  };

  const onDownload = async (url: string, filename: string) => {
    try {
      say(`正在下載 ${filename}…`);
      saveBlob(await client.fetchArtifact(url), filename);
      say(`已下載 ${filename}`);
    } catch (error) {
      say(`下載失敗：${errorText(error)}`, true);
    }
  };

  const toggleFormat = (format: OutputFormat, checked: boolean) =>
    setFormats((current) => {
      const next = new Set(current);
      if (checked) next.add(format);
      else next.delete(format);
      return next;
    });

  const commitToken = () => {
    if (tokenDraft.trim() !== token) setToken(tokenDraft.trim());
  };

  const svgArtifact = findArtifact(manifest, 'model.svg', 'preview.svg');
  const stlArtifact = findArtifact(manifest, 'model.stl');
  const stlEmptyMessage = !manifest
    ? '生成後會顯示 3D 模型'
    : manifest.status === 'source_only'
      ? '此工作為 source_only（未執行 CAD 核心），沒有 STL 可顯示。'
      : '此工作沒有 STL 輸出（未勾選 STL 或渲染失敗）。';

  return (
    <>
      <header className="topbar">
        <div>
          <div className="eyebrow">PROMPT → PARAMETRIC CAD</div>
          <h1>PromptCAD Studio</h1>
        </div>
        <div className="topbar-side">
          <span className="mode">{isTauri ? '桌面版' : 'Web'}</span>
          <a className="docs" href={`${backend.baseUrl}/docs`} target="_blank" rel="noreferrer">
            API Docs
          </a>
        </div>
      </header>

      <main className="layout">
        <section className="panel composer">
          <label htmlFor="prompt">描述你要的零件</label>
          <textarea
            id="prompt"
            rows={8}
            value={prompt}
            onChange={(event) => setPrompt(event.target.value)}
          />

          <div className={`controls${isTauri ? ' single' : ''}`}>
            <label>
              規劃器
              <select
                value={planner}
                onChange={(event) => setPlanner(event.target.value as PlannerChoice)}
              >
                {PLANNERS.map((item) => (
                  <option key={item.value} value={item.value}>
                    {item.label}
                  </option>
                ))}
              </select>
            </label>
            {!isTauri && (
              <label className="token">
                API Token（選填）
                <input
                  type="password"
                  autoComplete="off"
                  placeholder="伺服器有啟用時才需要"
                  value={tokenDraft}
                  onChange={(event) => setTokenDraft(event.target.value)}
                  onBlur={commitToken}
                  onKeyDown={(event) => {
                    if (event.key === 'Enter') commitToken();
                  }}
                />
              </label>
            )}
          </div>

          <fieldset>
            <legend>輸出</legend>
            <div className="checks">
              {availableFormats.map((format) => (
                <label key={format}>
                  <input
                    type="checkbox"
                    value={format}
                    checked={formats.has(format)}
                    onChange={(event) => toggleFormat(format, event.target.checked)}
                  />{' '}
                  {FORMAT_LABELS[format]}
                </label>
              ))}
            </div>
            <label className="render-toggle">
              <input
                type="checkbox"
                checked={render}
                onChange={(event) => setRender(event.target.checked)}
              />{' '}
              執行 CAD 核心（取消則只輸出 DSL、原始碼與預覽）
            </label>
          </fieldset>

          <button type="button" id="generate" disabled={busy} onClick={onGenerate}>
            產生 CAD
          </button>
          <p className={`status${status.error ? ' error' : ''}`} role="status" data-testid="status">
            {status.text}
          </p>

          <p className="caps" data-testid="capabilities">
            {capabilitiesError
              ? `無法取得後端能力：${capabilitiesError}`
              : capabilities
                ? `CadQuery：${capabilities.cadquery_available ? '可用' : '未安裝'} · OpenSCAD：${
                    capabilities.openscad_available ? '可用' : '未安裝'
                  } · 規劃模式：${capabilities.configured_planner_mode} · 渲染：${
                    capabilities.configured_render_backend
                  }`
                : '正在讀取後端能力…'}
          </p>

          <div className="examples">
            <span>快速範例：</span>
            {EXAMPLES.map((example) => (
              <button key={example.label} type="button" onClick={() => setPrompt(example.prompt)}>
                {example.label}
              </button>
            ))}
          </div>
        </section>

        <section className="panel result">
          <div className="result-head">
            <div>
              <div className="eyebrow">RESULT</div>
              <h2>{manifest?.spec.name ?? '尚未生成'}</h2>
            </div>
            <span className={`badge ${badge.kind}`} data-testid="badge">
              {badge.text}
            </span>
          </div>

          <div className="tabs" role="tablist" aria-label="預覽模式">
            <button
              type="button"
              role="tab"
              aria-selected={tab === '2d'}
              className={tab === '2d' ? 'active' : ''}
              onClick={() => setTab('2d')}
            >
              2D 預覽
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === '3d'}
              className={tab === '3d' ? 'active' : ''}
              onClick={() => setTab('3d')}
            >
              3D 檢視
            </button>
          </div>
          <div hidden={tab !== '2d'}>
            <SvgPreview artifact={svgArtifact} fetchArtifact={client.fetchArtifact} />
          </div>
          <div hidden={tab !== '3d'}>
            <StlViewer
              artifact={stlArtifact}
              fetchArtifact={client.fetchArtifact}
              emptyMessage={stlEmptyMessage}
            />
          </div>

          {manifest && (
            <dl className="meta" data-testid="job-meta">
              <div>
                <dt>狀態</dt>
                <dd>{manifest.status}</dd>
              </div>
              <div>
                <dt>規劃器</dt>
                <dd>{manifest.planner_used}</dd>
              </div>
              <div>
                <dt>渲染器</dt>
                <dd>{manifest.renderer_used}</dd>
              </div>
              <div>
                <dt>工作 ID</dt>
                <dd className="mono">{manifest.job_id}</dd>
              </div>
            </dl>
          )}
          {manifest?.error && <p className="status error">{manifest.error}</p>}

          <ValidationPanel
            report={report?.value ?? null}
            source={report?.source ?? 'job'}
            warnings={manifest?.warnings ?? []}
          />

          {manifest && (
            <ArtifactList
              jobId={manifest.job_id}
              artifacts={manifest.artifacts ?? []}
              bundleUrl={client.bundleUrl(manifest.job_id)}
              onDownload={(url, filename) => void onDownload(url, filename)}
            />
          )}

          <details>
            <summary>可編輯 CAD DSL / JSON</summary>
            <textarea
              className="spec-editor"
              aria-label="CAD DSL JSON"
              rows={18}
              spellCheck={false}
              value={specText}
              onChange={(event) => setSpecText(event.target.value)}
            />
            <div className="spec-actions">
              <button type="button" className="secondary" disabled={busy} onClick={() => void onValidate()}>
                驗證 JSON
              </button>
              <button type="button" className="secondary" disabled={busy} onClick={onRegenerate}>
                套用 JSON 並重新輸出
              </button>
            </div>
          </details>
        </section>
      </main>

      <JobHistory
        jobs={jobs}
        error={jobsError}
        activeJobId={manifest?.job_id ?? null}
        onOpen={(jobId) => void onOpenJob(jobId)}
        onRefresh={() => void loadJobs()}
      />
    </>
  );
}
