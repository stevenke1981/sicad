// Re-exports of the generated schema types plus a few aliases derived *from* them.
// Do not declare document/API model shapes here; edit app/models and regenerate instead.
export type {
  Artifact,
  CadDocument,
  CapabilityResponse,
  GenerateFromSpecRequest,
  GenerateRequest,
  JobListItem,
  JobManifest,
  PlanRequest,
  PlanResponse,
  ValidationIssue,
  ValidationReport,
  ValidationSeverity,
} from './generated/promptcad';

import type { GenerateRequest, JobManifest } from './generated/promptcad';

export type OutputFormat = NonNullable<GenerateRequest['formats']>[number];
export type PlannerChoice = NonNullable<GenerateRequest['planner']>;
export type JobStatus = JobManifest['status'];
