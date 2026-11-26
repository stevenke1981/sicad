//! Property tests for ids and orientations.

use promptcad_core::model::geom::vapprox_eq;
use promptcad_core::model::{AxisDir, FieldRef, Id, Orientation, PortRef};
use proptest::prelude::*;

proptest! {
    #[test]
    fn valid_ids_round_trip(s in "[a-z][a-z0-9_]{0,31}") {
        let id = Id::new(s.clone()).unwrap();
        let json = serde_json::to_string(&id).unwrap();
        prop_assert_eq!(serde_json::from_str::<Id>(&json).unwrap(), id);
    }

    #[test]
    fn invalid_ids_rejected(s in "([A-Z0-9_.:-][a-z]{0,4}|[a-z][a-z0-9_]{32,40}|[a-z]+[A-Z.:-][a-z]*)") {
        prop_assert!(Id::new(s).is_err());
    }

    #[test]
    fn refs_split_correctly(a in "[a-z][a-z0-9_]{0,31}", b in "[a-z][a-z0-9_]{0,31}") {
        let p = PortRef::new(format!("{a}.{b}")).unwrap();
        prop_assert_eq!(p.id_str(), a.as_str());
        prop_assert_eq!(p.port(), b.as_str());
        let f = FieldRef::new(format!("{a}:{b}")).unwrap();
        prop_assert_eq!(f.id_str(), a.as_str());
        prop_assert_eq!(f.field(), b.as_str());
        let swapped = format!("{}:{}", a, b);
        prop_assert!(PortRef::new(swapped).is_err());
    }

    #[test]
    fn orientation_is_a_rotation(i in 0usize..24, x in -1e3f64..1e3, y in -1e3f64..1e3, z in -1e3f64..1e3) {
        let o = Orientation::all()[i];
        let v = [x, y, z];
        let w = o.apply(v);
        prop_assert!(vapprox_eq(o.apply_inverse(w), v, 1e-9));
        let norm = |a: [f64; 3]| a.iter().map(|c| c * c).sum::<f64>();
        prop_assert!((norm(w) - norm(v)).abs() <= 1e-6 * (1.0 + norm(v)));
        for d in AxisDir::ALL {
            prop_assert_eq!(AxisDir::from_vec(o.apply(d.unit())), Some(o.apply_dir(d)));
        }
    }
}
