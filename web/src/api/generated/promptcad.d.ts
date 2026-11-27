/* eslint-disable */
/**
 * AUTO-GENERATED FILE: do not edit — generated from schemas/promptcad.schema.json.
 * Regenerate with `npm run gen:types` (in web/) after running scripts/export_schema.py.
 */

/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "Axis".
 */
export type Axis = 'x' | 'y' | 'z';
export type EdgeSelector = 'all' | 'vertical' | 'top' | 'bottom';
export type EdgeSelector1 = 'all' | 'vertical' | 'top' | 'bottom';
export type Axis1 = 'x' | 'y' | 'z';
export type HoleType = 'through' | 'blind' | 'clearance' | 'tapped' | 'counterbore' | 'countersink';
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "Material".
 */
export type Material = 'aluminum' | 'steel' | 'stainless_steel' | 'plastic' | 'wood' | 'other';
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "EdgeSelector".
 */
export type EdgeSelector2 = 'all' | 'vertical' | 'top' | 'bottom';
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "HoleType".
 */
export type HoleType1 =
  'through' | 'blind' | 'clearance' | 'tapped' | 'counterbore' | 'countersink';
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "ValidationSeverity".
 */
export type ValidationSeverity = 'error' | 'warning' | 'info';

/**
 * Generated from app/models by scripts/export_schema.py. Do not edit by hand.
 */
export interface PromptCADStudio {
  [k: string]: unknown;
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "Artifact".
 */
export interface Artifact {
  filename: string;
  media_type: string;
  size: number;
  url: string;
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "CadDocument".
 */
export interface CadDocument {
  /**
   * @maxItems 32
   */
  assumptions?: string[];
  base: PlateBase | CylinderBase | RingBase | LBracketBase | EnclosureBase;
  /**
   * @maxItems 8
   */
  chamfers?: ChamferFeature[];
  /**
   * @maxItems 8
   */
  fillets?: FilletFeature[];
  /**
   * @maxItems 64
   */
  holes?: HoleFeature[];
  material?: Material | null;
  name?: string;
  /**
   * @maxItems 32
   */
  notes?: string[];
  planner: PlannerMetadata;
  schema_version?: '1.0';
  source_prompt: string;
  unit?: 'mm';
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "PlateBase".
 */
export interface PlateBase {
  kind?: 'plate';
  length: number;
  thickness: number;
  width: number;
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "CylinderBase".
 */
export interface CylinderBase {
  diameter: number;
  height: number;
  kind?: 'cylinder';
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "RingBase".
 */
export interface RingBase {
  height: number;
  inner_diameter: number;
  kind?: 'ring';
  outer_diameter: number;
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "LBracketBase".
 */
export interface LBracketBase {
  depth: number;
  kind?: 'l_bracket';
  thickness: number;
  vertical_height: number;
  width: number;
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "EnclosureBase".
 */
export interface EnclosureBase {
  height: number;
  kind?: 'enclosure';
  length: number;
  wall_thickness: number;
  width: number;
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "ChamferFeature".
 */
export interface ChamferFeature {
  distance: number;
  kind?: 'chamfer';
  selector?: EdgeSelector;
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "FilletFeature".
 */
export interface FilletFeature {
  kind?: 'fillet';
  radius: number;
  selector?: EdgeSelector1;
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "HoleFeature".
 */
export interface HoleFeature {
  axis?: Axis1;
  counterbore_depth?: number | null;
  counterbore_diameter?: number | null;
  countersink_angle?: number | null;
  countersink_diameter?: number | null;
  depth?: number | null;
  diameter: number;
  hole_type?: HoleType;
  kind?: 'hole';
  thread?: string | null;
  x?: number;
  y?: number;
  z?: number;
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "PlannerMetadata".
 */
export interface PlannerMetadata {
  confidence?: number;
  planner: string;
  review_required?: boolean;
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "CapabilityResponse".
 */
export interface CapabilityResponse {
  base_features: string[];
  cadquery_available: boolean;
  configured_planner_mode: string;
  configured_render_backend: string;
  core_api_version?: number | null;
  core_available?: boolean;
  core_crate_version?: string | null;
  core_schema_versions?: string[];
  formats: string[];
  hole_types: string[];
  openscad_available: boolean;
  planners: string[];
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "GenerateFromSpecRequest".
 */
export interface GenerateFromSpecRequest {
  /**
   * @minItems 1
   * @maxItems 7
   */
  formats?: ('step' | 'stl' | 'dxf' | 'svg' | 'py' | 'scad' | 'json')[];
  render?: boolean;
  spec: CadDocument;
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "GenerateRequest".
 */
export interface GenerateRequest {
  /**
   * @minItems 1
   * @maxItems 7
   */
  formats?: ('step' | 'stl' | 'dxf' | 'svg' | 'py' | 'scad' | 'json')[];
  planner?: 'auto' | 'rule' | 'llm';
  prompt: string;
  render?: boolean;
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "JobListItem".
 */
export interface JobListItem {
  created_at: string;
  job_id: string;
  name: string;
  prompt: string;
  renderer_used: string;
  status: string;
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "JobManifest".
 */
export interface JobManifest {
  artifacts?: Artifact[];
  created_at: string;
  error?: string | null;
  job_id: string;
  planner_used: string;
  prompt: string;
  renderer_used: string;
  requested_formats: ('step' | 'stl' | 'dxf' | 'svg' | 'py' | 'scad' | 'json')[];
  spec: CadDocument;
  status: 'completed' | 'source_only' | 'failed';
  validation: ValidationReport;
  warnings?: string[];
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "ValidationReport".
 */
export interface ValidationReport {
  issues?: ValidationIssue[];
  review_required: boolean;
  valid: boolean;
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "ValidationIssue".
 */
export interface ValidationIssue {
  code: string;
  feature_index?: number | null;
  message: string;
  severity: ValidationSeverity;
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "PlanRequest".
 */
export interface PlanRequest {
  planner?: 'auto' | 'rule' | 'llm';
  prompt: string;
}
/**
 * This interface was referenced by `PromptCADStudio`'s JSON-Schema
 * via the `definition` "PlanResponse".
 */
export interface PlanResponse {
  planner_used: string;
  spec: CadDocument;
  validation: ValidationReport;
}
