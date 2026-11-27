/* eslint-disable */
/**
 * AUTO-GENERATED FILE: do not edit — generated from schemas/cad2/core-response.schema.json.
 * Regenerate with `npm run gen:types` (in web/) after changing the Rust core API (cargo run -- schema).
 */

/**
 * One response, tagged by `status`.
 */
export type CoreResponse =
  | {
      result: CoreResult;
      status: 'ok';
    }
  | {
      code: ErrorCode;
      /**
       * Human-readable English message.
       */
      message: string;
      /**
       * JSON pointer into the request (`/document`, `/document/components/3/shape/diameter`,
       * `/patch`, …); `null` when the error is not about one location.
       */
      path: string | null;
      status: 'error';
      /**
       * All schema violations (`schema_violation` only; empty otherwise), with request-relative
       * paths.
       */
      violations: SchemaViolation[];
    };
/**
 * Op-specific result, tagged by `op` (the request's op name).
 */
export type CoreResult =
  | {
      document: CadDocument2;
      input_version: DocumentVersion;
      op: 'validate';
      report: ValidationReport;
      resolved: Resolved;
    }
  | {
      input_version: DocumentVersion;
      op: 'solve';
      resolved: Resolved;
    }
  | {
      document: CadDocument2;
      input_version: DocumentVersion;
      /**
       * Binding issues of the recomputation (empty when all bindings evaluated).
       */
      issues: ValidationIssue[];
      op: 'normalize';
    }
  | {
      document: CadDocument2;
      op: 'upgrade';
    }
  | {
      /**
       * The 1.0 document; `null` when not expressible.
       */
      document: CadDocument1 | null;
      /**
       * Whether the document is expressible as CadDocument 1.0 (spec §10.2).
       */
      expressible: boolean;
      op: 'downgrade';
    }
  | {
      op: 'apply_patch';
      outcome: PatchOutcomeView;
    }
  | {
      /**
       * Assemblies in document order.
       */
      assemblies: AssemblyView[];
      /**
       * Components in document order.
       */
      components: ComponentView[];
      /**
       * The root the coordinator would compute (spec §6.3); `null` when it would be
       * `invalid_root`.
       */
      computed_root: Id | null;
      /**
       * Constraints in document order.
       */
      constraints: ConstraintView[];
      input_version: DocumentVersion;
      /**
       * Document `name`.
       */
      name: string;
      op: 'describe';
      /**
       * Parameters as stored.
       */
      parameters: Parameter[];
      /**
       * Parts in document order.
       */
      parts: PartView[];
      /**
       * Document `revision`.
       */
      revision: number;
      /**
       * Stored `root` (part or assembly id).
       */
      root: Id | null;
      /**
       * Assembly tree: top-level nodes (assemblies that are no assembly's child, then parts
       * that are no assembly's child), each with its children expanded recursively. Every
       * part / assembly is expanded at its first occurrence (pre-order) only; later
       * occurrences are [`TreeNode::repeated`] leaves, so the tree has at most
       * `assemblies + parts + Σ children` nodes (never more than [`MAX_TREE_NODES`]).
       */
      tree: TreeNode[];
    }
  | {
      /**
       * [`API_VERSION`] of this contract.
       */
      api_version: number;
      /**
       * `promptcad-core` crate version.
       */
      crate_version: string;
      op: 'version';
      /**
       * Accepted document `schema_version`s.
       */
      schema_versions: DocumentVersion[];
    };
export type Id = string;
export type FieldRef = string;
/**
 * Wire form of [`Component`] used for deserialization and JSON Schema.
 */
export type Component =
  | {
      id: Id;
      kind: 'empty';
      label: string | null;
      provenance: Provenance | null;
      role: EmptyRole;
      shape: EmptyShape;
    }
  | {
      id: Id;
      kind: 'dot';
      label: string | null;
      provenance: Provenance | null;
      shape: DotShape;
    }
  | {
      id: Id;
      kind: 'line';
      label: string | null;
      provenance: Provenance | null;
      shape: LineShape;
    }
  | {
      id: Id;
      kind: 'plane';
      label: string | null;
      provenance: Provenance | null;
      shape: PlaneShape;
    }
  | {
      id: Id;
      kind: 'volume';
      label: string | null;
      provenance: Provenance | null;
      shape: VolumeShape;
    };
/**
 * Who created a component (§3.1 `provenance.agent`).
 */
export type ProvenanceAgent =
  'space_planner' | 'component_smith' | 'joiner' | 'assembler' | 'human' | 'migration';
/**
 * Empty `role` (§3.1), required on and exclusive to `kind = "empty"`.
 */
export type EmptyRole = 'cut' | 'reserve' | 'keep_out' | 'envelope';
/**
 * Empty shapes, discriminated by `shape.kind`.
 */
export type EmptyShape =
  | {
      /**
       * Along local Z.
       */
      height: number;
      kind: 'box';
      /**
       * Along local X.
       */
      length: number;
      /**
       * Along local Y.
       */
      width: number;
    }
  | {
      /**
       * Diameter.
       */
      diameter: number;
      /**
       * Along local Z.
       */
      height: number;
      kind: 'cylinder';
    }
  | {
      axis: Axis;
      /**
       * `Dim | null`.
       */
      counterbore_depth: number | null;
      /**
       * `Dim | null`.
       */
      counterbore_diameter: number | null;
      /**
       * `0 < x < 180 | null`.
       */
      countersink_angle: number | null;
      /**
       * `Dim | null`.
       */
      countersink_diameter: number | null;
      /**
       * `Dim | null`; required for `blind`.
       */
      depth: number | null;
      /**
       * `Dim`.
       */
      diameter: number;
      hole_type: HoleType;
      kind: 'hole';
      /**
       * `^M\d{1,3}(\.\d)?(x\d(\.\d{1,2})?)?$ | null`.
       */
      thread: string | null;
    }
  | {
      kind: 'of';
      source: Id;
    };
/**
 * A world/local coordinate axis, serialized as `"x"`, `"y"`, `"z"`.
 */
export type Axis = 'x' | 'y' | 'z';
/**
 * Hole subtype (1.0 `HoleType`); default `through`.
 */
export type HoleType = 'through' | 'blind' | 'clearance' | 'tapped' | 'counterbore' | 'countersink';
/**
 * Dot shapes.
 */
export type DotShape = {
  kind: 'point';
};
/**
 * Line shapes (all derived components).
 */
export type LineShape =
  | {
      end: Id;
      kind: 'segment';
      start: Id;
    }
  | {
      /**
       * Whether the polyline is closed.
       */
      closed: boolean;
      kind: 'polyline';
      /**
       * Dot ids, 2–64.
       *
       * @minItems 2
       * @maxItems 64
       */
      points: Id[];
    }
  | {
      end: Id;
      kind: 'arc';
      start: Id;
      through: Id;
    }
  | {
      center: Id;
      /**
       * `Dim`.
       */
      diameter: number;
      kind: 'circle';
    };
/**
 * Plane shapes.
 */
export type PlaneShape =
  | {
      kind: 'rect';
      /**
       * Along local X.
       */
      length: number;
      /**
       * Along local Y.
       */
      width: number;
    }
  | {
      /**
       * `Dim`.
       */
      diameter: number;
      kind: 'disc';
    }
  | {
      /**
       * Inner closed Lines (holes in the profile), ≤ 16.
       *
       * @maxItems 16
       */
      inners: Id[];
      kind: 'profile';
      outer: Id;
    }
  | {
      kind: 'section';
      /**
       * Local Z of the horizontal section (`Coord`).
       */
      offset: number;
      source: Id;
    };
/**
 * Volume shapes.
 */
export type VolumeShape =
  | {
      kind: 'plate';
      /**
       * Along local X.
       */
      length: number;
      /**
       * Along local Z.
       */
      thickness: number;
      /**
       * Along local Y.
       */
      width: number;
    }
  | {
      /**
       * Diameter.
       */
      diameter: number;
      /**
       * Along local Z.
       */
      height: number;
      kind: 'cylinder';
    }
  | {
      /**
       * Along local Z.
       */
      height: number;
      /**
       * Inner (bore) diameter, must be smaller than `outer_diameter`.
       */
      inner_diameter: number;
      kind: 'ring';
      /**
       * Outer diameter.
       */
      outer_diameter: number;
    }
  | {
      /**
       * Along local Y.
       */
      depth: number;
      kind: 'l_bracket';
      /**
       * Plate thickness.
       */
      thickness: number;
      /**
       * Height of the upright above the base plate.
       */
      vertical_height: number;
      /**
       * Along local X.
       */
      width: number;
    }
  | {
      /**
       * Along local Z.
       */
      height: number;
      kind: 'enclosure';
      /**
       * Along local X.
       */
      length: number;
      /**
       * Wall and floor thickness.
       */
      wall_thickness: number;
      /**
       * Along local Y.
       */
      width: number;
    }
  | {
      /**
       * `Dim`.
       */
      height: number;
      kind: 'extrude';
      profile: Id;
    }
  | {
      /**
       * Degrees, `0 < angle ≤ 360`.
       */
      angle: number;
      axis: Id;
      kind: 'revolve';
      profile: Id;
    };
export type PortRef = string;
/**
 * A signed axis direction, serialized as `"+x"`, `"-x"`, … `"-z"` (spec §1.1).
 *
 * Declaration order (`+x, -x, +y, -y, +z, -z`) is the canonical order used by §5.4.
 */
export type AxisDir = '+x' | '-x' | '+y' | '-y' | '+z' | '-z';
/**
 * Constraint `kind` (§5.1).
 */
export type ConstraintKind =
  | 'fixed'
  | 'coincident'
  | 'on_face'
  | 'coaxial'
  | 'mate'
  | 'flush'
  | 'distance'
  | 'inside'
  | 'clearance';
/**
 * Body boolean operation (§6.1).
 */
export type BodyOpKind = 'add' | 'cut' | 'intersect';
/**
 * `kind: "chamfer"`.
 */
export type ChamferTag = 'chamfer';
/**
 * Edge selector for fillets and chamfers; default `vertical`.
 */
export type EdgeSelector = 'all' | 'vertical' | 'top' | 'bottom';
/**
 * `kind: "fillet"`.
 */
export type FilletTag = 'fillet';
/**
 * Part material (1.0 `Material`).
 */
export type Material = 'aluminum' | 'steel' | 'stainless_steel' | 'plastic' | 'wood' | 'other';
/**
 * Part source (§6.2).
 */
export type PartSource = 'made' | 'purchased';
/**
 * `schema_version: "2.0"`.
 */
export type SchemaVersion2 = '2.0';
/**
 * The unit literal `"mm"`.
 */
export type Unit = 'mm';
/**
 * A document `schema_version`.
 */
export type DocumentVersion = '1.0' | '2.0';
/**
 * Which validation layer produced an issue (§9.1, §9.4).
 */
export type Layer = 'analytic' | 'kernel';
/**
 * Agent roles (§8.1, §8.3). Used as issue `owner` and as the patch `agent`.
 */
export type Role = 'space_planner' | 'component_smith' | 'joiner' | 'assembler';
/**
 * Issue severity.
 */
export type Severity = 'error' | 'warning' | 'info';
/**
 * A port's world geometry, discriminated by `type` (§3.4).
 *
 * This interface was referenced by `undefined`'s JSON-Schema definition
 * via the `patternProperty` "^[a-z][a-z0-9_]{0,31}\.[a-z][a-z0-9_]{0,31}$".
 */
export type ResolvedPort =
  | {
      /**
       * World position.
       *
       * @minItems 3
       * @maxItems 3
       */
      position: number[];
      type: 'point';
    }
  | {
      direction: AxisDir;
      /**
       * World position.
       *
       * @minItems 3
       * @maxItems 3
       */
      position: number[];
      type: 'axis';
    }
  | {
      /**
       * `[length along u, length along v]`.
       *
       * @minItems 2
       * @maxItems 2
       */
      extent: number[];
      normal: AxisDir;
      /**
       * Face center.
       *
       * @minItems 3
       * @maxItems 3
       */
      origin: number[];
      type: 'face';
      u: AxisDir;
      v: AxisDir;
    }
  | {
      /**
       * Maximum corner.
       *
       * @minItems 3
       * @maxItems 3
       */
      max: number[];
      /**
       * Minimum corner.
       *
       * @minItems 3
       * @maxItems 3
       */
      min: number[];
      type: 'region';
    };
/**
 * 1.0 `BaseFeature`, discriminated by `kind`.
 */
export type BaseFeature =
  | {
      kind: 'plate';
      /**
       * Along local X.
       */
      length: number;
      /**
       * Along local Z.
       */
      thickness: number;
      /**
       * Along local Y.
       */
      width: number;
    }
  | {
      /**
       * Diameter.
       */
      diameter: number;
      /**
       * Along local Z.
       */
      height: number;
      kind: 'cylinder';
    }
  | {
      /**
       * Along local Z.
       */
      height: number;
      /**
       * Inner (bore) diameter, must be smaller than `outer_diameter`.
       */
      inner_diameter: number;
      kind: 'ring';
      /**
       * Outer diameter.
       */
      outer_diameter: number;
    }
  | {
      /**
       * Along local Y.
       */
      depth: number;
      kind: 'l_bracket';
      /**
       * Plate thickness.
       */
      thickness: number;
      /**
       * Height of the upright above the base plate.
       */
      vertical_height: number;
      /**
       * Along local X.
       */
      width: number;
    }
  | {
      /**
       * Along local Z.
       */
      height: number;
      kind: 'enclosure';
      /**
       * Along local X.
       */
      length: number;
      /**
       * Wall and floor thickness.
       */
      wall_thickness: number;
      /**
       * Along local Y.
       */
      width: number;
    };
/**
 * `kind: "hole"` of a 1.0 hole.
 */
export type HoleTag = 'hole';
/**
 * `schema_version: "1.0"`.
 */
export type SchemaVersion1 = '1.0';
/**
 * Outcome of `apply_patch`, tagged by `status`.
 */
export type PatchOutcomeView =
  | {
      /**
       * Components created by this patch, in document order.
       */
      created: Id[];
      /**
       * SHA-256 of the canonical `document`.
       */
      doc_sha256_after: string;
      /**
       * SHA-256 of the canonical input document.
       */
      doc_sha256_before: string;
      document: CadDocument2;
      log_entry: OpLogEntry;
      /**
       * `log_entry` as the exact `oplog.jsonl` line (compact JSON, no trailing newline).
       */
      log_line: string;
      /**
       * `r<revision_after>`, the `provenance.patch_id` of created components.
       */
      patch_id: string;
      /**
       * SHA-256 of the canonical patch JSON.
       */
      patch_sha256: string;
      report: ValidationReport;
      resolved: Resolved;
      /**
       * `revision_before + 1`.
       */
      revision_after: number;
      /**
       * Revision of the input document.
       */
      revision_before: number;
      status: 'accepted';
    }
  | {
      /**
       * SHA-256 of the canonical input document (unchanged).
       */
      doc_sha256_before: string;
      /**
       * The P-class issues behind a validation rejection (empty for other reasons).
       */
      issues: ValidationIssue[];
      log_entry: OpLogEntry;
      /**
       * `log_entry` as the exact `oplog.jsonl` line (compact JSON, no trailing newline).
       */
      log_line: string;
      /**
       * SHA-256 of the canonical patch JSON.
       */
      patch_sha256: string;
      reason: RejectReason;
      status: 'rejected';
    };
/**
 * What an entry records.
 */
export type EntryKind = 'patch' | 'undo';
/**
 * Who submitted a patch: an agent role or `human`.
 */
export type PatchOrigin = 'space_planner' | 'component_smith' | 'joiner' | 'assembler' | 'human';
/**
 * Patch reject reason (§9.2 last paragraph): a reject code or a P-class issue code.
 */
export type ReasonCode =
  | 'schema_invalid'
  | 'stale_revision'
  | 'permission_denied'
  | 'unknown_target'
  | 'field_is_bound'
  | 'assumption_not_found'
  | 'annotation_only_removal'
  | 'standards_mismatch'
  | 'duplicate_id'
  | 'dangling_reference'
  | 'unknown_port'
  | 'unknown_field'
  | 'reference_cycle'
  | 'invalid_promotion'
  | 'open_profile'
  | 'profile_not_axis_aligned'
  | 'multi_owner'
  | 'body_first_not_add'
  | 'invalid_body_component'
  | 'invalid_root'
  | 'port_type_mismatch'
  | 'invalid_fixed_target'
  | 'duplicate_binding'
  | 'binding_parse_error'
  | 'binding_unknown_name'
  | 'binding_cycle'
  | 'binding_eval_error'
  | 'binding_out_of_range'
  | 'parameter_out_of_range';
/**
 * Component `kind` (§3.1).
 */
export type ComponentKind = 'empty' | 'dot' | 'line' | 'plane' | 'volume';
/**
 * The four port types of §3.4.
 */
export type PortType = 'point' | 'axis' | 'face' | 'region';
/**
 * `shape.kind` of a component (the name; pair with the component `kind`).
 */
export type ShapeName =
  | 'box'
  | 'cylinder'
  | 'hole'
  | 'of'
  | 'point'
  | 'segment'
  | 'polyline'
  | 'arc'
  | 'circle'
  | 'rect'
  | 'disc'
  | 'profile'
  | 'section'
  | 'plate'
  | 'ring'
  | 'l_bracket'
  | 'enclosure'
  | 'extrude'
  | 'revolve';
/**
 * What a tree node refers to.
 */
export type TreeNodeKind = 'assembly' | 'part' | 'missing';
/**
 * Closed set of request-level error codes.
 */
export type ErrorCode =
  | 'bad_request'
  | 'too_large'
  | 'unsupported_schema_version'
  | 'wrong_schema_version'
  | 'invalid_document'
  | 'schema_violation'
  | 'invalid_patch'
  | 'response_too_large'
  | 'internal';

/**
 * CadDocument 2.0 (§2).
 */
export interface CadDocument2 {
  /**
   * ≤ 16.
   *
   * @maxItems 16
   */
  assemblies: Assembly[];
  /**
   * ≤ 32.
   *
   * @maxItems 32
   */
  assumptions: string[];
  /**
   * ≤ 128.
   *
   * @maxItems 128
   */
  bindings: Binding[];
  /**
   * ≤ 256 (≥ 1 only for completed documents).
   *
   * @maxItems 256
   */
  components: Component[];
  /**
   * ≤ 512.
   *
   * @maxItems 512
   */
  constraints: Constraint[];
  /**
   * 1–80 chars, default `"promptcad-design"`.
   */
  name: string;
  /**
   * ≤ 32.
   *
   * @maxItems 32
   */
  notes: string[];
  /**
   * ≤ 64.
   *
   * @maxItems 64
   */
  parameters: Parameter[];
  /**
   * ≤ 32 (≥ 1 only for completed documents).
   *
   * @maxItems 32
   */
  parts: Part[];
  planner: PlannerMetadata;
  /**
   * Incremented by the coordinator on every accepted patch.
   */
  revision: number;
  /**
   * Root part or assembly, maintained by the coordinator (§6.3). `null` while the document
   * has no parts yet (mid-loop revisions); required for completed documents.
   */
  root: Id | null;
  schema_version: SchemaVersion2;
  /**
   * 1–20000 chars.
   */
  source_prompt: string;
  unit: Unit;
}
/**
 * An assembly (§6.2).
 */
export interface Assembly {
  /**
   * 1–32 N2 ids (parts or assemblies).
   *
   * @minItems 1
   * @maxItems 32
   */
  children: Id[];
  id: Id;
  /**
   * 1–80 chars.
   */
  name: string;
}
/**
 * `{target, expr}` (§4.2). The bound field keeps its cached numeric value.
 */
export interface Binding {
  /**
   * Expression text, 1–160 chars (closed grammar §4.3, parsed by [`crate::expr`]).
   */
  expr: string;
  target: FieldRef;
}
/**
 * `{agent, patch_id}`, stamped by the coordinator (§3.1).
 */
export interface Provenance {
  agent: ProvenanceAgent;
  /**
   * `r<revision_after>` of the creating patch; `null` for `migration`. ≤ 64 chars.
   */
  patch_id: string | null;
}
/**
 * A constraint (§5.1): `{id, kind, a, b, value, u, v, frame}`.
 */
export interface Constraint {
  a: PortRef;
  /**
   * Second port; `null` exactly for `fixed`.
   */
  b: PortRef | null;
  /**
   * `fixed` only (required there).
   */
  frame: Frame | null;
  id: Id;
  kind: ConstraintKind;
  /**
   * `Coord | null` — `on_face` only.
   */
  u: number | null;
  /**
   * `Coord | null` — `on_face` only.
   */
  v: number | null;
  /**
   * `Coord | null` — see [`ConstraintKind::uses_value`].
   */
  value: number | null;
}
/**
 * Wire form of [`Frame`] (also its JSON Schema).
 */
export interface Frame {
  /**
   * @minItems 3
   * @maxItems 3
   */
  origin: number[];
  x_axis: AxisDir;
  z_axis: AxisDir;
}
/**
 * A named parameter (§4.1). `name` is in namespace N3 and may not be `min` / `max`.
 */
export interface Parameter {
  /**
   * Optional upper bound.
   */
  max: number | null;
  /**
   * Optional lower bound (`parameter_out_of_range` is a validator code).
   */
  min: number | null;
  name: Id;
  /**
   * ≤ 120 chars.
   */
  note: string | null;
  /**
   * Current value.
   */
  value: number;
}
/**
 * A part (§6.2).
 */
export interface Part {
  /**
   * Boolean recipe, 1–128 ops, first must be `add`.
   *
   * @minItems 1
   * @maxItems 128
   */
  body: BodyOp[];
  /**
   * ≤ 8.
   *
   * @maxItems 8
   */
  chamfers: ChamferFeature[];
  /**
   * ≤ 8.
   *
   * @maxItems 8
   */
  fillets: FilletFeature[];
  id: Id;
  /**
   * Material or `null`.
   */
  material: Material | null;
  /**
   * 1–80 chars.
   */
  name: string;
  /**
   * 1–999.
   */
  quantity: number;
  source: PartSource;
}
/**
 * `{op, component}` (§6.1).
 */
export interface BodyOp {
  component: Id;
  op: BodyOpKind;
}
/**
 * `{kind: "chamfer", distance, selector}` (1.0 `ChamferFeature`).
 */
export interface ChamferFeature {
  /**
   * `Dim`.
   */
  distance: number;
  kind: ChamferTag;
  selector: EdgeSelector;
}
/**
 * `{kind: "fillet", radius, selector}` (1.0 `FilletFeature`).
 */
export interface FilletFeature {
  kind: FilletTag;
  /**
   * `Dim`.
   */
  radius: number;
  selector: EdgeSelector;
}
/**
 * `{planner, confidence, review_required}` (1.0 `PlannerMetadata`).
 */
export interface PlannerMetadata {
  /**
   * `0 ≤ x ≤ 1`, default 1.
   */
  confidence: number;
  /**
   * Planner name, 1–64 chars.
   */
  planner: string;
  /**
   * Planner's own review flag, default `false`.
   */
  review_required: boolean;
}
/**
 * `{valid, review_required, issues}` (same shape and rules as 1.0).
 */
export interface ValidationReport {
  /**
   * Issues in emission order.
   */
  issues: ValidationIssue[];
  /**
   * Planner flag, or any assumption, or any `error`/`warning`.
   */
  review_required: boolean;
  /**
   * No `error` issues.
   */
  valid: boolean;
}
/**
 * ValidationIssue 2.0 (§9.1).
 */
export interface ValidationIssue {
  /**
   * Issue code (see [`codes::Code`]); kept as a string so 1.0/kernel reports round-trip.
   */
  code: string;
  /**
   * 1.0 feature index, `null` in 2.0 issues.
   */
  feature_index: number | null;
  layer: Layer;
  /**
   * Fixed-template message; may embed only ids, numbers and template text (§9.1).
   */
  message: string;
  /**
   * Role that should fix the issue.
   */
  owner: Role | null;
  /**
   * Other involved ids.
   */
  related: string[];
  severity: Severity;
  /**
   * Component/constraint/part id or `id:field` the issue is about.
   */
  target: string | null;
}
/**
 * The whole `resolved.json`.
 */
export interface Resolved {
  /**
   * World placement of every component.
   */
  components: {
    [k: string]: ResolvedComponent;
  };
  /**
   * Hole cut directions and main-hole cutter spans (§7).
   */
  holes: {
    [k: string]: ResolvedHole;
  };
  /**
   * Solver diagnostics only (§5.4, §5.5, §7.1 codes).
   */
  issues: ValidationIssue[];
  /**
   * Ports referenced by constraints plus every component's frame port.
   */
  ports: {
    [k: string]: ResolvedPort;
  };
  /**
   * Document revision that was solved.
   */
  revision: number;
  schema_version: SchemaVersion2;
}
/**
 * World placement of one component.
 *
 * This interface was referenced by `undefined`'s JSON-Schema definition
 * via the `patternProperty` "^[a-z][a-z0-9_]{0,31}$".
 */
export interface ResolvedComponent {
  /**
   * Axes defaulted to 0 by the under-constrained rule (§5.5 step 4), in axis order.
   */
  defaulted_axes: Axis[];
  /**
   * World translation `t` (the frame port's world position for independent components).
   *
   * @minItems 3
   * @maxItems 3
   */
  origin: number[];
  x_axis: AxisDir;
  z_axis: AxisDir;
}
/**
 * Solved hole (§7).
 *
 * This interface was referenced by `undefined`'s JSON-Schema definition
 * via the `patternProperty` "^[a-z][a-z0-9_]{0,31}$".
 */
export interface ResolvedHole {
  /**
   * Main-hole cutter end (world).
   *
   * @minItems 3
   * @maxItems 3
   */
  cutter_end: number[];
  /**
   * Main-hole cutter start (world).
   *
   * @minItems 3
   * @maxItems 3
   */
  cutter_start: number[];
  /**
   * Main-hole diameter.
   */
  diameter: number;
  direction: AxisDir;
}
/**
 * CadDocument 1.0 (`app/models/cad.py::CadDocument`).
 */
export interface CadDocument1 {
  /**
   * ≤ 32.
   *
   * @maxItems 32
   */
  assumptions: string[];
  base: BaseFeature;
  /**
   * ≤ 8.
   *
   * @maxItems 8
   */
  chamfers: ChamferFeature[];
  /**
   * ≤ 8.
   *
   * @maxItems 8
   */
  fillets: FilletFeature[];
  /**
   * ≤ 64.
   *
   * @maxItems 64
   */
  holes: HoleFeature[];
  /**
   * Default `null`.
   */
  material: Material | null;
  /**
   * 1–80 chars, default `"promptcad-part"`.
   */
  name: string;
  /**
   * ≤ 32.
   *
   * @maxItems 32
   */
  notes: string[];
  planner: PlannerMetadata;
  schema_version: SchemaVersion1;
  /**
   * 1–20000 chars.
   */
  source_prompt: string;
  unit: Unit;
}
/**
 * 1.0 `HoleFeature`.
 */
export interface HoleFeature {
  axis: Axis;
  /**
   * `Dim | null`.
   */
  counterbore_depth: number | null;
  /**
   * `Dim | null`.
   */
  counterbore_diameter: number | null;
  /**
   * `0 < x < 180 | null`.
   */
  countersink_angle: number | null;
  /**
   * `Dim | null`.
   */
  countersink_diameter: number | null;
  /**
   * `Dim | null`.
   */
  depth: number | null;
  /**
   * `Dim`.
   */
  diameter: number;
  hole_type: HoleType;
  kind: HoleTag;
  /**
   * Free text, ≤ 32 chars.
   */
  thread: string | null;
  /**
   * `Coord`, default 0.
   */
  x: number;
  /**
   * `Coord`, default 0.
   */
  y: number;
  /**
   * `Coord`, default 0.
   */
  z: number;
}
/**
 * One line of `oplog.jsonl`.
 */
export interface OpLogEntry {
  /**
   * Whether the document changed.
   */
  accepted: boolean;
  /**
   * Document revision when the entry was made (for patches: the revision the patch was
   * checked against, which differs from `patch.base_revision` exactly when it was
   * rejected as `stale_revision`).
   */
  base_revision: number;
  /**
   * SHA-256 of the canonical document after the entry (`null` when rejected).
   */
  doc_sha256_after: string | null;
  /**
   * SHA-256 of the canonical document before the entry.
   */
  doc_sha256_before: string;
  /**
   * Issues of the outcome: the full validation report when accepted, the attached P-class
   * issues when rejected.
   */
  issues: IssueSummary[];
  kind: EntryKind;
  origin: PatchOrigin;
  /**
   * The patch as submitted (`null` for undo entries).
   *
   * Its JSON Schema is a plain object: a patch rejected as `schema_invalid` is logged as
   * submitted and need not satisfy `patch.schema.json`'s range / length constraints.
   */
  patch: {
    [k: string]: unknown;
  } | null;
  /**
   * SHA-256 of the canonical patch JSON (`null` for undo entries).
   */
  patch_sha256: string | null;
  /**
   * Reject reason (`null` when accepted).
   */
  reject: RejectReason | null;
  /**
   * Reverse delta of an accepted patch (what undo applies); `null` otherwise.
   */
  revert: Revert | null;
  /**
   * Revision after the entry; `null` when rejected.
   */
  revision_after: number | null;
  schema_version: SchemaVersion2;
  /**
   * 0-based position in the log.
   */
  seq: number;
  /**
   * Caller-supplied timestamp (the library never reads a clock).
   */
  timestamp: string | null;
  /**
   * For undo entries: `seq` of the undone patch entry.
   */
  undoes: number | null;
}
/**
 * `{code, severity}` of one validation issue.
 */
export interface IssueSummary {
  /**
   * Issue code.
   */
  code: string;
  severity: Severity;
}
/**
 * Structured reject diagnostics returned to the submitter and written to the op log.
 */
export interface RejectReason {
  code: ReasonCode;
  /**
   * Fixed-template message (ids, numbers and template text only; never free text such as
   * labels, notes or assumptions — §9.1).
   */
  message: string;
  /**
   * Index of the offending op, when one op is to blame.
   */
  op_index: number | null;
  /**
   * Offending `id` / `id:field`, when known.
   */
  target: string | null;
}
/**
 * Previous values of the top-level keys a patch changed.
 */
export interface Revert {
  /**
   * Top-level key → value before the patch (raw JSON: each value is the previous
   * canonical value of that top-level CadDocument 2.0 key).
   */
  sections: {
    [k: string]: unknown;
  };
}
/**
 * One assembly in the read model.
 */
export interface AssemblyView {
  /**
   * Child ids (parts or assemblies) as stored.
   */
  children: Id[];
  id: Id;
  /**
   * Assembly name.
   */
  name: string;
  /**
   * First assembly (document order) listing this assembly as a child.
   */
  parent: Id | null;
}
/**
 * One component in the read model.
 */
export interface ComponentView {
  /**
   * Numeric shape fields (§3.3) in spec order with current values and binding state.
   */
  fields: FieldView[];
  /**
   * Frame port name of an independent component (`null` for derived ones).
   */
  frame_port: string | null;
  id: Id;
  /**
   * Independent (owns a pose) vs derived (§3.2).
   */
  independent: boolean;
  kind: ComponentKind;
  /**
   * Human-only label.
   */
  label: string | null;
  /**
   * First part (document order) whose `body` references this component, if any.
   */
  part: Id | null;
  /**
   * Ports (§3.4) in table order; Empty `of` uses its source Volume's table (empty when the
   * source does not resolve).
   */
  ports: PortView[];
  /**
   * Provenance stamp.
   */
  provenance: Provenance | null;
  /**
   * Ids referenced from `shape` (promotion sources), in field order.
   */
  refs: ShapeRef[];
  /**
   * Empty role (`null` for non-Empties).
   */
  role: EmptyRole | null;
  shape: ShapeName;
}
/**
 * A numeric (bindable) field.
 */
export interface FieldView {
  /**
   * Driven by a binding.
   */
  bound: boolean;
  /**
   * The binding expression when bound.
   */
  expr: string | null;
  /**
   * Field name (`length`, `value`, `u`, …); `id:name` is its field reference.
   */
  name: string;
  /**
   * Current (cached) value; `null` for an optional field that is null (not bindable).
   */
  value: number | null;
}
/**
 * A port.
 */
export interface PortView {
  /**
   * Port name; `id.name` is its port reference.
   */
  name: string;
  type: PortType;
}
/**
 * A reference from a shape to another component.
 */
export interface ShapeRef {
  /**
   * Shape field holding the reference (`source`, `start`, `points`, `profile`, …).
   */
  field: string;
  id: Id;
}
/**
 * One constraint in the read model.
 */
export interface ConstraintView {
  a: PortRef;
  /**
   * Port `b` (`null` for `fixed`).
   */
  b: PortRef | null;
  /**
   * Numeric fields of this kind (`value`, or `u`/`v`) with §5.1 defaults applied.
   */
  fields: FieldView[];
  id: Id;
  kind: ConstraintKind;
}
/**
 * One part in the read model.
 */
export interface PartView {
  /**
   * Boolean recipe.
   */
  body: BodyOp[];
  id: Id;
  /**
   * Material.
   */
  material: Material | null;
  /**
   * Part name.
   */
  name: string;
  /**
   * First assembly (document order) listing this part as a child.
   */
  parent: Id | null;
  /**
   * Quantity.
   */
  quantity: number;
  source: PartSource;
}
/**
 * A node of the assembly tree.
 */
export interface TreeNode {
  /**
   * Children (an assembly's first occurrence only; empty for parts, `missing` and
   * repeated nodes).
   */
  children: TreeNode[];
  id: Id;
  /**
   * Part / assembly name (`null` for `missing`).
   */
  name: string | null;
  node: TreeNodeKind;
  /**
   * `true` when this part / assembly already occurs earlier in the tree (pre-order): the
   * node is a back-reference to that first occurrence and has no children. Only documents
   * whose assembly graph is not a tree have repeated nodes: a child listed twice in one
   * assembly, a child of two assemblies (`multi_owner`) or a cycle (`reference_cycle`).
   * Always `false` for `missing` nodes.
   */
  repeated: boolean;
}
/**
 * One schema-level violation (range, length, list cap, cross-field shape rule, …).
 */
export interface SchemaViolation {
  /**
   * Human-readable, fixed-template message (embeds only ids and numbers).
   */
  message: string;
  /**
   * JSON pointer to the offending value (`""` for the document root).
   */
  path: string;
}
