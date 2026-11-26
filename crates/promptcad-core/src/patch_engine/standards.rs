//! Standard-part cross-check (spec §8.6 step 6, `standards_mismatch`).
//!
//! Port of the two metric tables of `app/services/standards.py`. A hole Empty that a patch
//! adds or modifies, whose `thread` is metric and whose `hole_type` is `clearance` or
//! `tapped`, must use the table diameter (tolerance 1e-6 mm) whenever its nominal size is in
//! the table. Nominal sizes outside the table are not checked (the Python helpers fall back to
//! a heuristic there, which is a planning default, not a standard).

use crate::model::common::HoleType;
use crate::model::geom::TOL;
use crate::model::v2::HoleSpec;

/// `METRIC_CLEARANCE_MM`: nominal → clearance hole diameter.
const METRIC_CLEARANCE_MM: [(f64, f64); 9] = [
    (2.0, 2.4),
    (2.5, 2.9),
    (3.0, 3.4),
    (4.0, 4.5),
    (5.0, 5.5),
    (6.0, 6.6),
    (8.0, 9.0),
    (10.0, 11.0),
    (12.0, 13.5),
];

/// `METRIC_TAP_DRILL_MM`: nominal → tap drill diameter.
const METRIC_TAP_DRILL_MM: [(f64, f64); 9] = [
    (2.0, 1.6),
    (2.5, 2.05),
    (3.0, 2.5),
    (4.0, 3.3),
    (5.0, 4.2),
    (6.0, 5.0),
    (8.0, 6.8),
    (10.0, 8.5),
    (12.0, 10.2),
];

/// Nominal diameter of a metric thread designation (`M6` → 6, `M2.5x0.45` → 2.5), as
/// `metric_thread_nominal()` in `standards.py` (case-insensitive, leading blanks allowed).
pub(crate) fn metric_thread_nominal(thread: &str) -> Option<f64> {
    let t = thread.trim_start();
    let rest = t.strip_prefix(['M', 'm'])?.trim_start();
    let end = rest
        .char_indices()
        .find(|&(_, ch)| !(ch.is_ascii_digit() || ch == '.'))
        .map_or(rest.len(), |(i, _)| i);
    let num = &rest[..end];
    // `[0-9]+(?:\.[0-9]+)?`: no leading dot, at most one dot, digits after it.
    let mut parts = num.splitn(2, '.');
    let int = parts.next().unwrap_or("");
    if int.is_empty() {
        return None;
    }
    match parts.next() {
        None => int.parse().ok(),
        Some(frac) => {
            let frac_digits: String = frac.chars().take_while(char::is_ascii_digit).collect();
            if frac_digits.is_empty() {
                int.parse().ok()
            } else {
                format!("{int}.{frac_digits}").parse().ok()
            }
        }
    }
}

fn lookup(table: &[(f64, f64)], nominal: f64) -> Option<f64> {
    table
        .iter()
        .find(|(n, _)| (n - nominal).abs() <= 1e-9)
        .map(|&(_, d)| d)
}

/// The table diameter a hole must have, if the standards table covers it.
pub(crate) fn expected_diameter(hole: &HoleSpec) -> Option<(HoleType, f64, f64)> {
    let table: &[(f64, f64)] = match hole.hole_type {
        HoleType::Clearance => &METRIC_CLEARANCE_MM,
        HoleType::Tapped => &METRIC_TAP_DRILL_MM,
        _ => return None,
    };
    let nominal = metric_thread_nominal(hole.thread.as_deref()?)?;
    lookup(table, nominal).map(|d| (hole.hole_type, nominal, d))
}

/// `Some((nominal, expected))` when `hole` contradicts the table.
pub(crate) fn mismatch(hole: &HoleSpec) -> Option<(HoleType, f64, f64)> {
    let (kind, nominal, expected) = expected_diameter(hole)?;
    ((hole.diameter - expected).abs() > TOL).then_some((kind, nominal, expected))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hole(hole_type: HoleType, diameter: f64, thread: Option<&str>) -> HoleSpec {
        HoleSpec {
            axis: Default::default(),
            diameter,
            hole_type,
            depth: None,
            thread: thread.map(str::to_owned),
            counterbore_diameter: None,
            counterbore_depth: None,
            countersink_diameter: None,
            countersink_angle: None,
        }
    }

    #[test]
    fn nominal_parsing() {
        assert_eq!(metric_thread_nominal("M6"), Some(6.0));
        assert_eq!(metric_thread_nominal("M6x1"), Some(6.0));
        assert_eq!(metric_thread_nominal("M2.5x0.45"), Some(2.5));
        assert_eq!(metric_thread_nominal(" m 8"), Some(8.0));
        assert_eq!(metric_thread_nominal("M"), None);
        assert_eq!(metric_thread_nominal("UNC"), None);
    }

    #[test]
    fn table_checks() {
        assert!(mismatch(&hole(HoleType::Clearance, 6.6, Some("M6"))).is_none());
        assert!(mismatch(&hole(HoleType::Clearance, 6.6 + 1e-7, Some("M6"))).is_none());
        assert_eq!(
            mismatch(&hole(HoleType::Clearance, 6.5, Some("M6"))),
            Some((HoleType::Clearance, 6.0, 6.6))
        );
        assert!(mismatch(&hole(HoleType::Tapped, 5.0, Some("M6x1"))).is_none());
        assert!(mismatch(&hole(HoleType::Tapped, 6.6, Some("M6"))).is_some());
        assert!(mismatch(&hole(HoleType::Tapped, 2.05, Some("M2.5"))).is_none());
        // not in the table / not a checked hole type / no thread
        assert!(mismatch(&hole(HoleType::Clearance, 7.0, Some("M7"))).is_none());
        assert!(mismatch(&hole(HoleType::Through, 1.0, Some("M6"))).is_none());
        assert!(mismatch(&hole(HoleType::Clearance, 1.0, None)).is_none());
    }
}
