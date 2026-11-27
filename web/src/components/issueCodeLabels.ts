/**
 * Short zh-TW labels for validation issue codes.
 *
 * Covers the 1.0 legacy checks (crates/promptcad-core/src/validate/legacy.rs) and the §9.2 code
 * table in crates/promptcad-core/src/model/issue.rs (2.0 checks). Terms follow
 * docs/CAD_DSL_2.md (元件, 約束, 綁定, 保留區, 假設, 連接點, 孔, 實體).
 */
const LABELS: Record<string, string> = {
  // structure and references (parse layer)
  duplicate_id: '編號重複',
  dangling_reference: '懸空引用',
  unknown_port: '未知連接點',
  unknown_field: '未知欄位',
  reference_cycle: '引用循環',
  invalid_promotion: '無效提升',
  open_profile: '輪廓不閉合',
  profile_not_axis_aligned: '輪廓未對齊',
  multi_owner: '多重歸屬',
  body_first_not_add: '首項非加法',
  invalid_body_component: '無效本體',
  invalid_root: '無效根件',
  port_type_mismatch: '連接點型別錯',
  invalid_fixed_target: '無效固定目標',
  orphan_component: '孤立元件',
  // parameters and bindings
  duplicate_binding: '綁定重複',
  binding_parse_error: '綁定語法錯',
  binding_unknown_name: '綁定名未知',
  binding_cycle: '綁定循環',
  binding_eval_error: '綁定求值錯',
  binding_out_of_range: '綁定超範圍',
  parameter_out_of_range: '參數超範圍',
  // constraints and solving
  no_fixed_root: '無固定根',
  conflicting_constraints: '約束衝突',
  under_constrained: '欠約束',
  redundant_constraint: '冗餘約束',
  orientation_unsatisfiable: '方向無解',
  hole_direction_conflict: '孔向衝突',
  hole_axis_mismatch: '孔軸不符',
  // empties
  reserve_violated: '侵入保留區',
  keep_out_violated: '侵入禁入區',
  envelope_exceeded: '超出包絡',
  empty_cut_no_effect: '空切無效',
  empty_cut_removes_all: '空切全除',
  reserve_overlap: '保留區重疊',
  reserve_unreachable: '保留區不可達',
  // between parts
  part_interference: '零件干涉',
  clearance_violated: '間隙不足',
  inside_violated: '違反內含',
  floating_part: '零件浮空',
  // carried over from 1.0
  thin_wall: '壁厚偏薄',
  thin_bracket: '支架板薄',
  extreme_aspect_ratio: '長寬比極端',
  hole_outside_part: '孔超出零件',
  hole_in_inner_void: '孔落在內孔',
  hole_intersects_inner_void: '孔切入內孔',
  low_edge_clearance: '孔邊距不足',
  overlapping_holes: '孔位重疊',
  blind_depth_exceeds_material: '盲孔過深',
  counterbore_depth_exceeds_material: '沉孔過深',
  countersink_depth_exceeds_material: '沉頭過深',
  tapped_hole_without_thread: '攻牙孔無螺紋',
  thread_annotation_only: '螺紋僅標註',
  fillet_too_large: '圓角過大',
  chamfer_too_large: '倒角過大',
  assumption: '假設',
  // kernel layer
  brep_invalid: '實體無效',
  empty_solid: '空實體',
  multi_body_part: '多實體',
  feature_skipped: '特徵略過',
};

export const ISSUE_CODE_LABELS: Readonly<Record<string, string>> = Object.freeze(LABELS);

/** zh-TW short label for an issue code, or undefined when the code is unknown. */
export function issueCodeLabel(code: string): string | undefined {
  return Object.hasOwn(LABELS, code) ? LABELS[code] : undefined;
}
