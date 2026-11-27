import type {
  CadDocument,
  CapabilityResponse,
  GenerateFromDocumentRequest,
  GenerateFromSpecRequest,
  GenerateRequest,
  JobListItem,
  JobManifest,
  PlanRequest,
  PlanResponse,
  ValidationReport,
} from './types';

/** `GET /api/v1/health` is not a schema model; it is a fixed two-field status probe. */
export interface HealthStatus {
  status: string;
  version: string;
}

/**
 * The single backend interface used by UI components (ADR 0003). It mirrors
 * app/api/routes.py and docs/API.md. Components never call fetch or Tauri directly.
 */
export interface ApiClient {
  health(): Promise<HealthStatus>;
  capabilities(): Promise<CapabilityResponse>;
  plan(request: PlanRequest): Promise<PlanResponse>;
  generate(request: GenerateRequest): Promise<JobManifest>;
  generateFromSpec(request: GenerateFromSpecRequest): Promise<JobManifest>;
  /** Generate from a CadDocument 2.0 (`POST /generate-from-document`). */
  generateFromDocument(request: GenerateFromDocumentRequest): Promise<JobManifest>;
  validate(spec: CadDocument): Promise<ValidationReport>;
  listJobs(): Promise<JobListItem[]>;
  getJob(jobId: string): Promise<JobManifest>;
  /** Fetch an artifact (manifest `artifacts[].url` or {@link bundleUrl}) with auth headers. */
  fetchArtifact(url: string): Promise<Blob>;
  /** URL of the ZIP bundle for a job; pass it to {@link fetchArtifact}. */
  bundleUrl(jobId: string): string;
}

/** Error raised for non-2xx responses; `message` carries FastAPI's `detail` when present. */
export class ApiError extends Error {
  readonly status: number;
  readonly detail: unknown;

  constructor(status: number, message: string, detail?: unknown) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.detail = detail;
  }
}
