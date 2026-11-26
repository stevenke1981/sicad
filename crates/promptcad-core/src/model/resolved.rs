//! `resolved.json` (spec §5.6): solver output, derived data never written back to `spec.json`.
//!
//! Maps are `BTreeMap`s keyed by [`Id`] / [`PortRef`] (ordered by string), so serialization is
//! byte-for-byte deterministic.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::geom::{Axis, AxisDir, Orientation, Vec3};
use super::ids::{Id, PortRef};
use super::issue::ValidationIssue;
use super::v2::SchemaVersion2;

/// The whole `resolved.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Resolved {
    /// `"2.0"`.
    pub schema_version: SchemaVersion2,
    /// Document revision that was solved.
    pub revision: u64,
    /// World placement of every component.
    pub components: BTreeMap<Id, ResolvedComponent>,
    /// Ports referenced by constraints plus every component's frame port.
    pub ports: BTreeMap<PortRef, ResolvedPort>,
    /// Hole cut directions and main-hole cutter spans (§7).
    pub holes: BTreeMap<Id, ResolvedHole>,
    /// Solver diagnostics only (§5.4, §5.5, §7.1 codes).
    pub issues: Vec<ValidationIssue>,
}

impl Resolved {
    /// An empty result for `revision`.
    pub fn new(revision: u64) -> Self {
        Resolved {
            schema_version: SchemaVersion2::V2_0,
            revision,
            components: BTreeMap::new(),
            ports: BTreeMap::new(),
            holes: BTreeMap::new(),
            issues: Vec::new(),
        }
    }
}

/// World placement of one component.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolvedComponent {
    /// World translation `t` (the frame port's world position for independent components).
    pub origin: Vec3,
    /// World direction of local X.
    pub x_axis: AxisDir,
    /// World direction of local Z.
    pub z_axis: AxisDir,
    /// Axes defaulted to 0 by the under-constrained rule (§5.5 step 4), in axis order.
    pub defaulted_axes: Vec<Axis>,
}

impl ResolvedComponent {
    /// Builds a placement from an orientation.
    pub fn new(origin: Vec3, orientation: Orientation, defaulted_axes: Vec<Axis>) -> Self {
        ResolvedComponent {
            origin,
            x_axis: orientation.x_axis(),
            z_axis: orientation.z_axis(),
            defaulted_axes,
        }
    }

    /// The orientation; `None` if the stored axes are not perpendicular.
    pub fn orientation(&self) -> Option<Orientation> {
        Orientation::new(self.x_axis, self.z_axis)
    }
}

/// A port's world geometry, discriminated by `type` (§3.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResolvedPort {
    /// `point`
    Point {
        /// World position.
        position: Vec3,
    },
    /// `axis`
    Axis {
        /// World position.
        position: Vec3,
        /// World direction.
        direction: AxisDir,
    },
    /// `face`
    Face {
        /// Face center.
        origin: Vec3,
        /// Outward normal.
        normal: AxisDir,
        /// In-face u direction.
        u: AxisDir,
        /// In-face v direction (`u × v = normal`).
        v: AxisDir,
        /// `[length along u, length along v]`.
        extent: [f64; 2],
    },
    /// `region` (axis-aligned bounding box).
    Region {
        /// Minimum corner.
        min: Vec3,
        /// Maximum corner.
        max: Vec3,
    },
}

/// The four port types of §3.4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PortType {
    /// `point`
    Point,
    /// `axis`
    Axis,
    /// `face`
    Face,
    /// `region`
    Region,
}

impl ResolvedPort {
    /// The port type.
    pub fn port_type(&self) -> PortType {
        match self {
            ResolvedPort::Point { .. } => PortType::Point,
            ResolvedPort::Axis { .. } => PortType::Axis,
            ResolvedPort::Face { .. } => PortType::Face,
            ResolvedPort::Region { .. } => PortType::Region,
        }
    }
}

/// Solved hole (§7).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ResolvedHole {
    /// Inward cut direction.
    pub direction: AxisDir,
    /// Main-hole cutter start (world).
    pub cutter_start: Vec3,
    /// Main-hole cutter end (world).
    pub cutter_end: Vec3,
    /// Main-hole diameter.
    pub diameter: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn spec_example_parses() {
        // §5.6 example
        let j = json!({
          "schema_version": "2.0",
          "revision": 5,
          "components": {
            "v_lid": {"origin": [0, 0, 33], "x_axis": "+x", "z_axis": "+z", "defaulted_axes": []}
          },
          "ports": {
            "v_lid.bottom": {"type": "face", "origin": [0, 0, 33], "normal": "-z", "u": "+x", "v": "-y", "extent": [100, 70]},
            "e_pcb.region": {"type": "region", "min": [-40, -25, 7], "max": [40, 25, 32]}
          },
          "holes": {
            "e_h1": {"direction": "-z", "cutter_start": [-50, -20, 11], "cutter_end": [-50, -20, -1], "diameter": 6.6}
          },
          "issues": []
        });
        let r: Resolved = serde_json::from_value(j.clone()).unwrap();
        assert_eq!(r.ports.len(), 2);
        let first = r.ports.keys().next().unwrap();
        assert_eq!(first.as_str(), "e_pcb.region");
        assert_eq!(
            r.components["v_lid"].orientation(),
            Some(Orientation::IDENTITY)
        );
        let back = serde_json::to_value(&r).unwrap();
        assert_eq!(
            crate::canonical::normalize_numbers(&back),
            crate::canonical::normalize_numbers(&j)
        );
        assert!(serde_json::from_value::<ResolvedPort>(
            json!({"type": "point", "position": [0, 0, 0], "direction": "+z"})
        )
        .is_err());
    }
}
