//! Pyrealium fall damage policy, not a recovered retail damage formula. Measure the
//! accepted movement stream; charge once only at DAT-verified physics contact.
use crate::movement::Placement;

#[derive(Clone, Debug, Default)]
pub struct Fall {
    previous: Option<(f64, Placement)>,
    apex: Option<f32>,
    fastest: f32,
}
impl Fall {
    /// Every tracked descent can end at contact, including harmless steps.
    /// Damage eligibility must never gate clearing an earlier apex/speed.
    /// Defer the DAT query only when there is no descent to resolve.
    pub fn resolve_landing(
        &mut self,
        height: f32,
        max_health: u32,
        contact: impl FnOnce() -> bool,
    ) -> u32 {
        if self.apex.is_some() && contact() {
            self.grounded(height, max_health)
        } else {
            0
        }
    }
    pub fn observe(&mut self, time: f64, point: Placement) {
        let previous = self.previous.replace((time, point));
        let Some((then, old)) = previous else { return };
        let dt = time - then;
        let dz = old.coordinates[2] - point.coordinates[2];
        if !(0.01..=0.75).contains(&dt)
            || old
                .distance(point)
                .is_none_or(|d| d > 150.0 * dt as f32 + 1.0)
            || dz < -0.25
        {
            self.apex = None;
            self.fastest = 0.0;
            return;
        }
        if dz > 0.02 {
            self.apex.get_or_insert(old.coordinates[2]);
            self.fastest = self.fastest.max(dz / dt as f32);
        }
    }
    fn grounded(&mut self, height: f32, max_health: u32) -> u32 {
        let drop = self.apex.take().map_or(0.0, |top| top - height);
        let speed = std::mem::take(&mut self.fastest);
        if drop <= 6.0 || speed < 9.0 {
            return 0;
        }
        (((drop - 6.0) / 24.0).clamp(0.0, 1.0) * max_health as f32).ceil() as u32
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn p(z: f32) -> Placement {
        Placement {
            cell: 0x56670001,
            coordinates: [10., 10., z],
            heading: 0,
        }
    }
    #[test]
    fn short_jumps_and_slopes_are_safe_but_large_falls_hurt_once() {
        let mut f = Fall::default();
        for i in 0..20 {
            f.observe(i as f64 * 0.1, p(20. - i as f32 * 0.1));
        }
        assert_eq!(f.grounded(18.1, 100), 0);
        let mut f = Fall::default();
        for i in 0..20 {
            f.observe(i as f64 * 0.1, p(40. - i as f32));
        }
        assert_eq!(f.grounded(21., 100), 55);
        assert_eq!(f.grounded(21., 100), 0);
    }
    #[test]
    fn gaps_unconfirmed_contact_and_discontinuous_relocations_do_not_injure() {
        let mut f = Fall::default();
        f.observe(0., p(100.));
        f.observe(0.1, p(0.));
        assert_eq!(f.grounded(0., 100), 0);
        for i in 0..20 {
            f.observe(1. + i as f64 * 0.1, p(40. - i as f32));
        }
        f.observe(9., p(0.));
        assert_eq!(f.grounded(0., 100), 0);
    }
    #[test]
    fn small_grounded_drops_never_accumulate_into_a_large_fall() {
        let mut f = Fall::default();
        f.observe(0., p(100.));
        // Forty separate one-metre drops at impact speed. Old damage-gated
        // contact checks retained the first apex and injured the eighth step.
        for i in 1..=40 {
            let z = 100. - i as f32;
            f.observe(i as f64 * 0.1, p(z));
            assert_eq!(f.resolve_landing(z, 310, || true), 0, "grounded step {i}");
            assert!(f.apex.is_none());
            assert_eq!(f.fastest, 0.);
        }
    }
    #[test]
    fn long_downhill_walk_then_small_drop_is_safe() {
        let mut f = Fall::default();
        f.observe(0., p(100.));
        for i in 1..=400 {
            let z = 100. - i as f32 * 0.1;
            f.observe(i as f64 * 0.1, p(z));
            assert_eq!(f.resolve_landing(z, 310, || true), 0);
        }
        f.observe(40.1, p(58.8));
        assert_eq!(f.resolve_landing(58.8, 310, || true), 0);
    }
    #[test]
    fn verified_contact_is_required_and_ends_both_safe_and_damaging_falls() {
        let mut f = Fall::default();
        f.observe(0., p(100.));
        for i in 1..=15 {
            let z = 100. - 2. * i as f32;
            f.observe(i as f64 * 0.1, p(z));
            assert_eq!(f.resolve_landing(z, 310, || false), 0, "no midair damage");
        }
        assert_eq!(f.resolve_landing(70., 310, || true), 310);
        assert_eq!(
            f.resolve_landing(70., 310, || panic!("no pending descent")),
            0
        );
        // A safe landing must also clear the earlier high speed, so a later
        // slow descent cannot inherit it and produce damage.
        f.observe(1.6, p(69.));
        assert_eq!(f.resolve_landing(69., 310, || true), 0);
        for i in 1..=100 {
            f.observe(1.6 + i as f64 * 0.1, p(69. - i as f32 * 0.1));
        }
        assert_eq!(f.resolve_landing(59., 310, || true), 0);
    }
}
