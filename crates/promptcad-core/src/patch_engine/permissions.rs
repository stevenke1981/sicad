//! Role schema subsets (spec §8.3), field classes (§8.4) and the permission matrix (§8.5).
//!
//! Two layers, both enforced by [`super::apply_patch`] for agent origins:
//!
//! 1. [`role_allows_op`] — the role's `Patch` schema (§8.3): which `op`s the role's schema
//!    contains and, for `add_component`, which `kind`/`role` its `ComponentInput` admits. A
//!    violation is `schema_invalid` (§8.6 step 1).
//! 2. [`permits`] over [`OpClass`] — the §8.5 matrix, evaluated per op against the document
//!    state as it evolves through the patch (the field class of the *current* target decides).
//!    A violation is `permission_denied` (§8.6 step 3).
//!
//! The schema subsets are a strict refinement of the matrix: every op a role's schema admits
//! is permitted for at least one target class, and the matrix never permits an op that the
//! schema subset rejects.

use crate::fields::{field_class, FieldClass};
use crate::model::issue::Role;
use crate::model::patch::{Patch, PatchOp};
use crate::model::v2::{CadDocument2, ComponentInput, ComponentKind, EmptyRole};

/// A row of the §8.5 permission matrix, i.e. an op together with the class of its target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OpClass {
    /// `set_parameter` in a patch with empty `resolves`.
    SetParameter,
    /// `set_parameter` in a repair patch (`resolves` non-empty; `apply_patch` additionally
    /// requires, for `component_smith`, that a resolved code is present in the pre-patch report).
    SetParameterRepair,
    /// `add_component` of a reserved-class Empty (`reserve`, `keep_out`, `envelope`).
    AddReservedComponent,
    /// `add_component` of any other component (Dot, Line, Plane, Volume, `cut` Empty).
    AddPhysicalComponent,
    /// `remove_component`, `set_field`, `set_binding`, `remove_binding` on a reserved-class
    /// Empty.
    EditReserved,
    /// `remove_component`, `set_field`, `set_binding`, `remove_binding` on a physical
    /// component.
    EditPhysical,
    /// `add_constraint`, `remove_constraint`, and `set_field`/`set_binding`/`remove_binding`
    /// on a constraint `value`/`u`/`v`.
    Constraint,
    /// `add_part`, `set_body`, `add_assembly`.
    Assembly,
    /// `add_assumption`, `add_note`.
    Annotation,
    /// `remove_assumption` (further restricted by §8.7).
    RemoveAssumption,
}

impl OpClass {
    /// All rows in matrix order.
    pub const ALL: [OpClass; 10] = [
        OpClass::SetParameter,
        OpClass::SetParameterRepair,
        OpClass::AddReservedComponent,
        OpClass::AddPhysicalComponent,
        OpClass::EditReserved,
        OpClass::EditPhysical,
        OpClass::Constraint,
        OpClass::Assembly,
        OpClass::Annotation,
        OpClass::RemoveAssumption,
    ];
}

/// The §8.5 permission matrix.
pub fn permits(role: Role, class: OpClass) -> bool {
    use OpClass::*;
    match class {
        Annotation | RemoveAssumption => true,
        SetParameter => role == Role::SpacePlanner,
        SetParameterRepair => matches!(role, Role::SpacePlanner | Role::ComponentSmith),
        AddReservedComponent | EditReserved => role == Role::SpacePlanner,
        AddPhysicalComponent | EditPhysical => role == Role::ComponentSmith,
        Constraint => role == Role::Joiner,
        Assembly => role == Role::Assembler,
    }
}

fn component_is_reserved(c: &ComponentInput) -> bool {
    c.role().is_some_and(EmptyRole::is_reserved_class)
}

/// The matrix row of `op` given the current document state. `None` when the target does not
/// exist (the op then fails later with `unknown_target`, which is not a permission question).
pub fn op_class(doc: &CadDocument2, patch: &Patch, op: &PatchOp) -> Option<OpClass> {
    let edit = |reserved: bool| {
        if reserved {
            OpClass::EditReserved
        } else {
            OpClass::EditPhysical
        }
    };
    Some(match op {
        PatchOp::SetParameter { .. } => {
            if patch.resolves.is_empty() {
                OpClass::SetParameter
            } else {
                OpClass::SetParameterRepair
            }
        }
        PatchOp::AddComponent { component } => {
            if component_is_reserved(component) {
                OpClass::AddReservedComponent
            } else {
                OpClass::AddPhysicalComponent
            }
        }
        PatchOp::RemoveComponent { id } => {
            let c = doc.component(id.as_str())?;
            edit(c.role().is_some_and(EmptyRole::is_reserved_class))
        }
        PatchOp::SetField { target, .. }
        | PatchOp::SetBinding { target, .. }
        | PatchOp::RemoveBinding { target } => match field_class(doc, target).ok()? {
            FieldClass::Reserved => OpClass::EditReserved,
            FieldClass::Physical => OpClass::EditPhysical,
            FieldClass::Constraint => OpClass::Constraint,
        },
        PatchOp::AddConstraint { .. } | PatchOp::RemoveConstraint { .. } => OpClass::Constraint,
        PatchOp::AddPart { .. } | PatchOp::SetBody { .. } | PatchOp::AddAssembly { .. } => {
            OpClass::Assembly
        }
        PatchOp::AddAssumption { .. } | PatchOp::AddNote { .. } => OpClass::Annotation,
        PatchOp::RemoveAssumption { .. } => OpClass::RemoveAssumption,
    })
}

/// `op` names in each role's schema subset (§8.3).
pub fn role_ops(role: Role) -> &'static [&'static str] {
    const EDIT: [&str; 9] = [
        "add_component",
        "remove_component",
        "set_field",
        "set_binding",
        "remove_binding",
        "set_parameter",
        "add_assumption",
        "remove_assumption",
        "add_note",
    ];
    match role {
        Role::SpacePlanner | Role::ComponentSmith => &EDIT,
        Role::Joiner => &[
            "add_constraint",
            "remove_constraint",
            "set_field",
            "set_binding",
            "remove_binding",
            "add_assumption",
            "remove_assumption",
            "add_note",
        ],
        Role::Assembler => &[
            "add_part",
            "set_body",
            "add_assembly",
            "add_assumption",
            "remove_assumption",
            "add_note",
        ],
    }
}

/// `true` when the role's `ComponentInput` subset (§8.3) admits `component`.
pub fn role_allows_component(role: Role, component: &ComponentInput) -> bool {
    match role {
        Role::SpacePlanner => component_is_reserved(component),
        Role::ComponentSmith => {
            component.kind() != ComponentKind::Empty || component.role() == Some(EmptyRole::Cut)
        }
        Role::Joiner | Role::Assembler => false,
    }
}

/// `true` when `op` is expressible in `role`'s `Patch` schema (§8.3): the op is in the role's
/// subset and, for `add_component`, the component is in the role's `ComponentInput` subset.
pub fn role_allows_op(role: Role, op: &PatchOp) -> bool {
    if !role_ops(role).contains(&op.op_name()) {
        return false;
    }
    match op {
        PatchOp::AddComponent { component } => role_allows_component(role, component),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_rows() {
        let allowed = |class| {
            Role::ALL
                .into_iter()
                .filter(|r| permits(*r, class))
                .collect::<Vec<_>>()
        };
        assert_eq!(allowed(OpClass::SetParameter), [Role::SpacePlanner]);
        assert_eq!(
            allowed(OpClass::SetParameterRepair),
            [Role::SpacePlanner, Role::ComponentSmith]
        );
        assert_eq!(allowed(OpClass::Constraint), [Role::Joiner]);
        assert_eq!(allowed(OpClass::Annotation), Role::ALL);
    }

    #[test]
    fn schema_subsets_are_within_matrix() {
        // Every op name in a role's subset is permitted for some class reachable by that op.
        for role in Role::ALL {
            for name in role_ops(role) {
                let classes: &[OpClass] = match *name {
                    "add_component" => {
                        &[OpClass::AddReservedComponent, OpClass::AddPhysicalComponent]
                    }
                    "remove_component" => &[OpClass::EditReserved, OpClass::EditPhysical],
                    "set_field" | "set_binding" | "remove_binding" => &[
                        OpClass::EditReserved,
                        OpClass::EditPhysical,
                        OpClass::Constraint,
                    ],
                    "set_parameter" => &[OpClass::SetParameter, OpClass::SetParameterRepair],
                    "add_constraint" | "remove_constraint" => &[OpClass::Constraint],
                    "add_part" | "set_body" | "add_assembly" => &[OpClass::Assembly],
                    "add_assumption" | "add_note" => &[OpClass::Annotation],
                    "remove_assumption" => &[OpClass::RemoveAssumption],
                    other => panic!("unexpected op {other}"),
                };
                assert!(
                    classes.iter().any(|c| permits(role, *c)),
                    "{role} {name} is in the schema but never permitted"
                );
            }
        }
    }
}
