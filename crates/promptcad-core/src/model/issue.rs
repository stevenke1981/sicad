//! Validation issues and reports (spec §9.1), and the frozen code table (§9.2).
//!
//! [`ValidationIssue`] 2.0 is a superset of 1.0 (`severity, code, message, feature_index`) with
//! `target`, `related`, `owner`, `layer`; the new fields default on input so 1.0 reports parse.
//! [`ValidationReport`] computes `valid`/`review_required` exactly like 1.0
//! (the Python `DesignValidator`, removed in Phase 2c).

use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Issue severity.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Blocks rendering (and, for P-class codes, the patch).
    Error,
    /// Does not block; sets `review_required`.
    Warning,
    /// Informational.
    Info,
}

/// Which validation layer produced an issue (§9.1, §9.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Layer {
    /// Rust analytic validator (default).
    #[default]
    Analytic,
    /// Geometry-kernel validation in the renderer sidecar.
    Kernel,
}

/// Agent roles (§8.1, §8.3). Used as issue `owner` and as the patch `agent`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Space planner: reserved-class Empties, parameters.
    SpacePlanner,
    /// Component smith: Dots, Lines, Planes, Volumes, `cut` Empties.
    ComponentSmith,
    /// Joiner: constraints.
    Joiner,
    /// Assembler: parts, bodies, assemblies.
    Assembler,
}

impl Role {
    /// All roles in spec order.
    pub const ALL: [Role; 4] = [
        Role::SpacePlanner,
        Role::ComponentSmith,
        Role::Joiner,
        Role::Assembler,
    ];

    /// The JSON string.
    pub fn as_str(self) -> &'static str {
        match self {
            Role::SpacePlanner => "space_planner",
            Role::ComponentSmith => "component_smith",
            Role::Joiner => "joiner",
            Role::Assembler => "assembler",
        }
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// ValidationIssue 2.0 (§9.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidationIssue {
    /// Severity.
    pub severity: Severity,
    /// Issue code (see [`codes::Code`]); kept as a string so 1.0/kernel reports round-trip.
    pub code: String,
    /// Fixed-template message; may embed only ids, numbers and template text (§9.1).
    pub message: String,
    /// 1.0 feature index, `null` in 2.0 issues.
    #[serde(default)]
    pub feature_index: Option<i64>,
    /// Component/constraint/part id or `id:field` the issue is about.
    #[serde(default)]
    pub target: Option<String>,
    /// Other involved ids.
    #[serde(default)]
    pub related: Vec<String>,
    /// Role that should fix the issue.
    #[serde(default)]
    pub owner: Option<Role>,
    /// Producing layer.
    #[serde(default)]
    pub layer: Layer,
}

impl ValidationIssue {
    /// An analytic-layer issue with the code's default severity and fixed owner. For
    /// [`codes::DefaultOwner::Submitter`] codes the owner is left `None`; use
    /// [`ValidationIssue::for_submitter`] when the submitting role is known.
    pub fn new(code: codes::Code, message: impl Into<String>) -> Self {
        Self::in_layer(code, Layer::Analytic, message)
    }

    /// A kernel-layer issue (§9.4) with the code's kernel severity.
    pub fn kernel(code: codes::Code, message: impl Into<String>) -> Self {
        Self::in_layer(code, Layer::Kernel, message)
    }

    fn in_layer(code: codes::Code, layer: Layer, message: impl Into<String>) -> Self {
        ValidationIssue {
            severity: code.severity_in(layer),
            code: code.as_str().to_owned(),
            message: message.into(),
            feature_index: None,
            target: None,
            related: Vec::new(),
            owner: code.resolve_owner(None),
            layer,
        }
    }

    /// Resolves the owner for a known submitting role (P-class codes → submitter).
    pub fn for_submitter(mut self, submitter: Option<Role>) -> Self {
        if let Some(code) = self.code() {
            self.owner = code.resolve_owner(submitter);
        }
        self
    }

    /// Sets `target`.
    pub fn with_target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    /// Sets `related`.
    pub fn with_related<I, S>(mut self, related: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.related = related.into_iter().map(Into::into).collect();
        self
    }

    /// Overrides `owner`.
    pub fn with_owner(mut self, owner: Option<Role>) -> Self {
        self.owner = owner;
        self
    }

    /// Overrides `severity`.
    pub fn with_severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    /// Sets the 1.0 `feature_index`.
    pub fn with_feature_index(mut self, index: Option<i64>) -> Self {
        self.feature_index = index;
        self
    }

    /// The typed code, if it is in the frozen table.
    pub fn code(&self) -> Option<codes::Code> {
        self.code.parse().ok()
    }
}

/// `{valid, review_required, issues}` (same shape and rules as 1.0).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ValidationReport {
    /// No `error` issues.
    pub valid: bool,
    /// Planner flag, or any assumption, or any `error`/`warning`.
    pub review_required: bool,
    /// Issues in emission order.
    #[serde(default)]
    pub issues: Vec<ValidationIssue>,
}

impl ValidationReport {
    /// 1.0 rule: `valid = no error`; `review_required = review_hint || any error/warning`, where
    /// `review_hint = planner.review_required || !assumptions.is_empty()`.
    pub fn from_issues(issues: Vec<ValidationIssue>, review_hint: bool) -> Self {
        let valid = !issues.iter().any(|i| i.severity == Severity::Error);
        let review_required = review_hint
            || issues
                .iter()
                .any(|i| matches!(i.severity, Severity::Error | Severity::Warning));
        ValidationReport {
            valid,
            review_required,
            issues,
        }
    }

    /// [`ValidationReport::from_issues`] with the hint taken from a 2.0 document.
    pub fn for_document(
        doc: &crate::model::v2::CadDocument2,
        issues: Vec<ValidationIssue>,
    ) -> Self {
        Self::from_issues(
            issues,
            doc.planner.review_required || !doc.assumptions.is_empty(),
        )
    }
}

/// The frozen §9.2 code table.
pub mod codes {
    use super::*;

    /// Code class (§9.2).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    pub enum CodeClass {
        /// **P** — patch-blocking: any such error rejects the patch (and blocks rendering of
        /// human input).
        #[serde(rename = "P")]
        PatchBlocking,
        /// **D** — design: fed back to the repair loop; `error` blocks rendering.
        #[serde(rename = "D")]
        Design,
    }

    /// Default owner of a code (§9.2 "owner" column).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum DefaultOwner {
        /// The role that submitted the patch (all P-class codes except `invalid_root`).
        Submitter,
        /// A fixed role.
        Role(Role),
        /// No owner (`assumption`).
        Nobody,
    }

    /// One row of the code table.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct CodeInfo {
        /// The code.
        pub code: Code,
        /// Default severity in the code's home layer.
        pub severity: Severity,
        /// P or D.
        pub class: CodeClass,
        /// Default owner.
        pub owner: DefaultOwner,
        /// Home layer (`part_interference` is emitted by both layers; its home is analytic).
        pub layer: Layer,
        /// Only checked for completed documents (`invalid_root`).
        pub completed_only: bool,
    }

    use DefaultOwner::{Nobody, Submitter};
    use Role::{Assembler, ComponentSmith, Joiner, SpacePlanner};

    const fn by(role: Role) -> DefaultOwner {
        DefaultOwner::Role(role)
    }

    macro_rules! code_table {
        ($( $(#[$m:meta])* $variant:ident = $name:literal, $sev:ident, $class:ident, $owner:expr, $layer:ident; )*) => {
            /// Every issue code of spec §9.2 (append-only; retired codes are in
            /// [`RETIRED_CODES`]).
            #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
            pub enum Code {
                $( $(#[$m])* #[doc = concat!("`", $name, "`")] #[serde(rename = $name)] $variant, )*
            }

            impl Code {
                /// All codes in table order.
                pub const ALL: &'static [Code] = &[ $( Code::$variant, )* ];

                /// The wire string.
                pub fn as_str(self) -> &'static str {
                    match self { $( Code::$variant => $name, )* }
                }

                /// The table row.
                pub fn info(self) -> CodeInfo {
                    match self {
                        $( Code::$variant => CodeInfo {
                            code: self,
                            severity: Severity::$sev,
                            class: CodeClass::$class,
                            owner: $owner,
                            layer: Layer::$layer,
                            completed_only: matches!(self, Code::InvalidRoot),
                        }, )*
                    }
                }
            }

            impl FromStr for Code {
                type Err = String;
                fn from_str(s: &str) -> Result<Self, Self::Err> {
                    match s {
                        $( $name => Ok(Code::$variant), )*
                        _ => Err(format!("unknown issue code {s:?}")),
                    }
                }
            }
        };
    }

    code_table! {
        // ---- structure and references (parse layer) ----
        DuplicateId = "duplicate_id", Error, PatchBlocking, Submitter, Analytic;
        DanglingReference = "dangling_reference", Error, PatchBlocking, Submitter, Analytic;
        UnknownPort = "unknown_port", Error, PatchBlocking, Submitter, Analytic;
        UnknownField = "unknown_field", Error, PatchBlocking, Submitter, Analytic;
        ReferenceCycle = "reference_cycle", Error, PatchBlocking, Submitter, Analytic;
        InvalidPromotion = "invalid_promotion", Error, PatchBlocking, Submitter, Analytic;
        OpenProfile = "open_profile", Error, PatchBlocking, Submitter, Analytic;
        ProfileNotAxisAligned = "profile_not_axis_aligned", Error, PatchBlocking, Submitter, Analytic;
        MultiOwner = "multi_owner", Error, PatchBlocking, Submitter, Analytic;
        BodyFirstNotAdd = "body_first_not_add", Error, PatchBlocking, Submitter, Analytic;
        InvalidBodyComponent = "invalid_body_component", Error, PatchBlocking, Submitter, Analytic;
        /// P only for completed documents; owner is fixed to `assembler`.
        InvalidRoot = "invalid_root", Error, PatchBlocking, by(Assembler), Analytic;
        PortTypeMismatch = "port_type_mismatch", Error, PatchBlocking, Submitter, Analytic;
        InvalidFixedTarget = "invalid_fixed_target", Error, PatchBlocking, Submitter, Analytic;
        OrphanComponent = "orphan_component", Warning, Design, by(Assembler), Analytic;
        // ---- parameters and bindings ----
        DuplicateBinding = "duplicate_binding", Error, PatchBlocking, Submitter, Analytic;
        BindingParseError = "binding_parse_error", Error, PatchBlocking, Submitter, Analytic;
        BindingUnknownName = "binding_unknown_name", Error, PatchBlocking, Submitter, Analytic;
        BindingCycle = "binding_cycle", Error, PatchBlocking, Submitter, Analytic;
        BindingEvalError = "binding_eval_error", Error, PatchBlocking, Submitter, Analytic;
        BindingOutOfRange = "binding_out_of_range", Error, PatchBlocking, Submitter, Analytic;
        ParameterOutOfRange = "parameter_out_of_range", Error, PatchBlocking, Submitter, Analytic;
        // ---- constraints and solving ----
        NoFixedRoot = "no_fixed_root", Error, Design, by(Joiner), Analytic;
        ConflictingConstraints = "conflicting_constraints", Error, Design, by(Joiner), Analytic;
        UnderConstrained = "under_constrained", Warning, Design, by(Joiner), Analytic;
        RedundantConstraint = "redundant_constraint", Info, Design, by(Joiner), Analytic;
        OrientationUnsatisfiable = "orientation_unsatisfiable", Error, Design, by(Joiner), Analytic;
        HoleDirectionConflict = "hole_direction_conflict", Error, Design, by(Joiner), Analytic;
        HoleAxisMismatch = "hole_axis_mismatch", Error, Design, by(ComponentSmith), Analytic;
        // ---- empties ----
        ReserveViolated = "reserve_violated", Error, Design, by(ComponentSmith), Analytic;
        KeepOutViolated = "keep_out_violated", Error, Design, by(ComponentSmith), Analytic;
        EnvelopeExceeded = "envelope_exceeded", Error, Design, by(ComponentSmith), Analytic;
        EmptyCutNoEffect = "empty_cut_no_effect", Warning, Design, by(ComponentSmith), Analytic;
        EmptyCutRemovesAll = "empty_cut_removes_all", Error, Design, by(ComponentSmith), Analytic;
        ReserveOverlap = "reserve_overlap", Warning, Design, by(SpacePlanner), Analytic;
        ReserveUnreachable = "reserve_unreachable", Info, Design, by(SpacePlanner), Analytic;
        // ---- between parts ----
        /// Analytic warning; kernel error (see [`Code::severity_in`]).
        PartInterference = "part_interference", Warning, Design, by(Joiner), Analytic;
        ClearanceViolated = "clearance_violated", Error, Design, by(Joiner), Analytic;
        InsideViolated = "inside_violated", Error, Design, by(Joiner), Analytic;
        FloatingPart = "floating_part", Warning, Design, by(Joiner), Analytic;
        // ---- carried over from 1.0 (severities as in the removed 1.0 Python validator) ----
        ThinWall = "thin_wall", Warning, Design, by(ComponentSmith), Analytic;
        ThinBracket = "thin_bracket", Warning, Design, by(ComponentSmith), Analytic;
        ExtremeAspectRatio = "extreme_aspect_ratio", Warning, Design, by(ComponentSmith), Analytic;
        HoleOutsidePart = "hole_outside_part", Error, Design, by(ComponentSmith), Analytic;
        HoleInInnerVoid = "hole_in_inner_void", Error, Design, by(ComponentSmith), Analytic;
        HoleIntersectsInnerVoid = "hole_intersects_inner_void", Warning, Design, by(ComponentSmith), Analytic;
        LowEdgeClearance = "low_edge_clearance", Warning, Design, by(ComponentSmith), Analytic;
        OverlappingHoles = "overlapping_holes", Error, Design, by(ComponentSmith), Analytic;
        BlindDepthExceedsMaterial = "blind_depth_exceeds_material", Error, Design, by(ComponentSmith), Analytic;
        CounterboreDepthExceedsMaterial = "counterbore_depth_exceeds_material", Error, Design, by(ComponentSmith), Analytic;
        CountersinkDepthExceedsMaterial = "countersink_depth_exceeds_material", Error, Design, by(ComponentSmith), Analytic;
        TappedHoleWithoutThread = "tapped_hole_without_thread", Warning, Design, by(ComponentSmith), Analytic;
        ThreadAnnotationOnly = "thread_annotation_only", Info, Design, by(ComponentSmith), Analytic;
        FilletTooLarge = "fillet_too_large", Error, Design, by(Assembler), Analytic;
        ChamferTooLarge = "chamfer_too_large", Error, Design, by(Assembler), Analytic;
        Assumption = "assumption", Info, Design, Nobody, Analytic;
        // ---- kernel layer ----
        BrepInvalid = "brep_invalid", Error, Design, by(ComponentSmith), Kernel;
        EmptySolid = "empty_solid", Error, Design, by(ComponentSmith), Kernel;
        MultiBodyPart = "multi_body_part", Warning, Design, by(ComponentSmith), Kernel;
        FeatureSkipped = "feature_skipped", Warning, Design, by(Assembler), Kernel;
    }

    /// Retired codes that must never be reused (§9.3).
    pub const RETIRED_CODES: &[&str] = &["constraint_cycle"];

    impl Code {
        /// Default severity in the code's home layer.
        pub fn default_severity(self) -> Severity {
            self.info().severity
        }

        /// Severity when emitted by `layer` (`part_interference` is an error in the kernel).
        pub fn severity_in(self, layer: Layer) -> Severity {
            match (self, layer) {
                (Code::PartInterference, Layer::Kernel) => Severity::Error,
                _ => self.info().severity,
            }
        }

        /// P or D.
        pub fn class(self) -> CodeClass {
            self.info().class
        }

        /// `true` for P-class codes.
        pub fn is_patch_blocking(self) -> bool {
            self.class() == CodeClass::PatchBlocking
        }

        /// Default owner.
        pub fn default_owner(self) -> DefaultOwner {
            self.info().owner
        }

        /// Concrete owner given the submitting role (if any).
        pub fn resolve_owner(self, submitter: Option<Role>) -> Option<Role> {
            match self.default_owner() {
                DefaultOwner::Submitter => submitter,
                DefaultOwner::Role(r) => Some(r),
                DefaultOwner::Nobody => None,
            }
        }

        /// Home layer.
        pub fn layer(self) -> Layer {
            self.info().layer
        }
    }

    impl fmt::Display for Code {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.as_str())
        }
    }

    /// Patch reject reasons that are not issue codes (§9.2 last paragraph). Any P-class
    /// [`Code`] is also a valid reject reason.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
    #[serde(rename_all = "snake_case")]
    pub enum RejectCode {
        /// The patch failed the role's `Patch` schema.
        SchemaInvalid,
        /// `base_revision` ≠ current `revision`.
        StaleRevision,
        /// §8.5 permission matrix.
        PermissionDenied,
        /// An op's target does not exist.
        UnknownTarget,
        /// `set_field` on a bound field.
        FieldIsBound,
        /// `remove_assumption` text not found.
        AssumptionNotFound,
        /// `remove_assumption` without any design op (§8.7).
        AnnotationOnlyRemoval,
        /// Standard-part cross-check failed.
        StandardsMismatch,
    }

    impl RejectCode {
        /// All reject codes in spec order.
        pub const ALL: [RejectCode; 8] = [
            RejectCode::SchemaInvalid,
            RejectCode::StaleRevision,
            RejectCode::PermissionDenied,
            RejectCode::UnknownTarget,
            RejectCode::FieldIsBound,
            RejectCode::AssumptionNotFound,
            RejectCode::AnnotationOnlyRemoval,
            RejectCode::StandardsMismatch,
        ];

        /// The wire string.
        pub fn as_str(self) -> &'static str {
            match self {
                RejectCode::SchemaInvalid => "schema_invalid",
                RejectCode::StaleRevision => "stale_revision",
                RejectCode::PermissionDenied => "permission_denied",
                RejectCode::UnknownTarget => "unknown_target",
                RejectCode::FieldIsBound => "field_is_bound",
                RejectCode::AssumptionNotFound => "assumption_not_found",
                RejectCode::AnnotationOnlyRemoval => "annotation_only_removal",
                RejectCode::StandardsMismatch => "standards_mismatch",
            }
        }
    }

    impl fmt::Display for RejectCode {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.as_str())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::codes::*;
    use super::*;

    #[test]
    fn code_table_is_complete_and_consistent() {
        assert_eq!(Code::ALL.len(), 60);
        for c in Code::ALL {
            assert_eq!(c.as_str().parse::<Code>().unwrap(), *c);
            assert_eq!(serde_json::to_value(c).unwrap(), c.as_str());
            assert!(!RETIRED_CODES.contains(&c.as_str()));
            if c.is_patch_blocking() && *c != Code::InvalidRoot {
                assert_eq!(c.default_owner(), DefaultOwner::Submitter, "{c}");
                assert_eq!(c.default_severity(), Severity::Error, "{c}");
            }
        }
        assert_eq!(
            Code::PartInterference.severity_in(Layer::Analytic),
            Severity::Warning
        );
        assert_eq!(
            Code::PartInterference.severity_in(Layer::Kernel),
            Severity::Error
        );
        assert_eq!(Code::RedundantConstraint.default_severity(), Severity::Info);
        assert_eq!(
            Code::HoleAxisMismatch.resolve_owner(None),
            Some(Role::ComponentSmith)
        );
        assert_eq!(Code::Assumption.resolve_owner(Some(Role::Joiner)), None);
        assert_eq!(
            Code::DuplicateId.resolve_owner(Some(Role::Joiner)),
            Some(Role::Joiner)
        );
        assert!(Code::InvalidRoot.info().completed_only);
        assert_eq!(Code::BrepInvalid.layer(), Layer::Kernel);
        assert!("constraint_cycle".parse::<Code>().is_err());
        assert_eq!(RejectCode::ALL.len(), 8);
        for r in RejectCode::ALL {
            assert_eq!(serde_json::to_value(r).unwrap(), r.as_str());
            assert!(r.as_str().parse::<Code>().is_err());
        }
    }

    #[test]
    fn issue_builders_and_report() {
        let i = ValidationIssue::new(Code::UnderConstrained, "v_lid z defaulted to 0")
            .with_target("v_lid");
        assert_eq!(i.severity, Severity::Warning);
        assert_eq!(i.owner, Some(Role::Joiner));
        assert_eq!(i.code(), Some(Code::UnderConstrained));
        let k = ValidationIssue::kernel(Code::PartInterference, "p_a overlaps p_b")
            .with_related(["p_b"]);
        assert_eq!(k.severity, Severity::Error);
        assert_eq!(k.layer, Layer::Kernel);
        let d = ValidationIssue::new(Code::DuplicateId, "x").for_submitter(Some(Role::Assembler));
        assert_eq!(d.owner, Some(Role::Assembler));

        let r = ValidationReport::from_issues(vec![], false);
        assert!(r.valid && !r.review_required);
        let r = ValidationReport::from_issues(vec![], true);
        assert!(r.valid && r.review_required);
        let r = ValidationReport::from_issues(vec![i.clone()], false);
        assert!(r.valid && r.review_required);
        let info = ValidationIssue::new(Code::RedundantConstraint, "x");
        let r = ValidationReport::from_issues(vec![info], false);
        assert!(r.valid && !r.review_required);
        let r = ValidationReport::from_issues(vec![k], false);
        assert!(!r.valid && r.review_required);
    }

    #[test]
    fn v1_issue_parses_and_v2_serializes_all_fields() {
        let i: ValidationIssue = serde_json::from_str(
            r#"{"severity":"warning","code":"thin_wall","message":"m","feature_index":null}"#,
        )
        .unwrap();
        assert_eq!(i.layer, Layer::Analytic);
        let v = serde_json::to_value(&i).unwrap();
        let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys.len(), 8);
        assert!(serde_json::from_str::<ValidationIssue>(
            r#"{"severity":"warning","code":"x","message":"m","extra":1}"#
        )
        .is_err());
    }
}
