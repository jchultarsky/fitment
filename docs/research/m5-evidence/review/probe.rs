#[cfg(test)]
mod m5_review_probe {
    use super::*;
    use crate::features::{FeatureId, FeatureKind, Sense};
    use crate::features::Axis;
    use crate::geom::{Dir3, Point3};
    use crate::matching::{ShortlistIndex, IndexTokens, shortlist};
    use crate::verdict::RequirementId;

    fn line(id: &str, from: f64, to: f64, w: Window, z_off: f64) -> ReqLine {
        ReqLine { ids: vec![RequirementId::new(id)], sense: Sense::Shaft, point: [z_off, 0.0, 0.0],
                  dir: [0.0, 0.0, 1.0], windows: vec![w], from, to, openings: 0 }
    }

    /// Two coaxial required lines in one group (SG5), one candidate stack
    /// serving both: level 2 keeps; does level 1 admit?
    #[test]
    fn one_stack_serving_a_coaxial_group() {
        let sigma = 0.01;
        let sigma_ang = 0.01_f64.to_radians();
        let w = Window::new(Some(10.0 - sigma), Some(10.0 + sigma));
        let mut sock = SocketSignature {
            lines: vec![line("shaft:a", 0.0, 5.0, w, 0.0), line("shaft:b", 15.0, 20.0, w, 0.0)],
            seats: vec![], cones: vec![], blocked_by: vec![], removed: vec![], demands: 2,
            lever_mm: 0.0, sigma, sigma_ang,
        };
        sock.lever_mm = diagonal(&[[0.0, 0.0, 0.0], [0.0, 0.0, 20.0]]);
        let stack = SigStack {
            feature: FeatureId::new(FeatureKind::Shaft, 7),
            sense: Sense::Shaft,
            axis: Axis { point_mm: Point3::new([0.0, 0.0, 0.0]).unwrap(), direction: Dir3::from_unit([0.0, 0.0, 1.0]).unwrap() },
            diameters_mm: vec![10.0],
            extent_mm: [Some(-1.0), Some(21.0)],
            openings: 0,
            unknown_end: false,
        };
        let part = PartSignature { version: SIGNATURE_VERSION, stacks: vec![stack], planes: vec![], cones: vec![],
            lever_mm: diagonal(&[[0.0, 0.0, -1.0], [0.0, 0.0, 21.0]]), uncertainty_mm: 1e-6, unread: Unread::default() };
        let l2 = shortlist(&sock, &part);
        let mut idx = ShortlistIndex::new();
        idx.insert(1, &IndexTokens::of(&part));
        let l1 = idx.query(&sock);
        println!("groups {:?}", sock.groups(sock.linear_window(part.lever_mm()), sock.angular_window() + 1e-9));
        println!("level 2: {:?}", l2.as_ref().map(|k| k.budget_exhausted));
        println!("level 1: {:?}", l1);
        assert!(l2.is_ok(), "level 2 keeps");
        assert_eq!(l1, Ok(vec![]), "level 1 drops what level 2 keeps");
    }
}
