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
} from './generated/promptcad';

import type { GenerateRequest, JobManifest, Severity } from './generated/promptcad';

// The report is generated from the Rust core's schema (ADR 0008), where the enum is `Severity`.
export type ValidationSeverity = Severity;

export type OutputFormat = NonNullable<GenerateRequest['formats']>[number];
export type PlannerChoice = NonNullable<GenerateRequest['planner']>;
export type JobStatus = JobManifest['status'];
