//! Was a teleport right? Judged by where the avatar is afterwards, as a viewer's user sees it: in
//! the right region, within 2 m of the target across the map (it may fall to the ground), and
//! still there 1.5 s later, without a jump back toward where it came from on the way. The rules
//! are those of the Tranquillity teleport rig that found the snap back after local teleports.

use glam::Vec3;

/// How far from the target, across the map, still counts as there (metres).
pub const NEAR: f32 = 2.0;

/// Where the avatar was at one moment.
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub region: String,
    pub at: Vec3,
    /// Since the arrival.
    pub ms: u64,
}

/// Where a teleport started and where it aimed.
#[derive(Debug, Clone)]
pub struct Trip {
    pub from_region: String,
    pub from: Vec3,
    pub to_region: String,
    pub to: Vec3,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// At the target, and it stayed.
    Ok,
    /// Never reached the target.
    NotArrived,
    /// At the target in the end, but on the way it jumped back toward the start: the snap back a
    /// viewer shows.
    BouncedForAMoment { worst: f32, at: Vec3, ms: u64 },
    /// Back toward the start, and still there.
    BouncedBack,
    /// Somewhere else in the region.
    Drifted(f32),
}

impl Verdict {
    pub fn ok(&self) -> bool {
        *self == Verdict::Ok
    }
}

/// The distance across the map: height left out, as the avatar may land below the target.
pub fn flat(a: Vec3, b: Vec3) -> f32 {
    (a.truncate() - b.truncate()).length()
}

/// The verdict on a teleport: `arrived` once the avatar was at the target, the samples taken
/// after that (20 ms apart), and where it was at the end.
pub fn judge(trip: &Trip, arrived: bool, samples: &[Sample], end: &Sample) -> Verdict {
    if !arrived {
        return Verdict::NotArrived;
    }
    // A jump back only means something within one region: from another, "toward the start"
    // has no sense on this map.
    let local = trip.from_region == trip.to_region;
    let mut worst: Option<(f32, Vec3, u64)> = None;
    if local {
        for s in samples.iter().filter(|s| s.region == trip.to_region) {
            let d = flat(s.at, trip.to);
            if flat(s.at, trip.from) < d && worst.is_none_or(|(w, _, _)| d > w) {
                worst = Some((d, s.at, s.ms));
            }
        }
    }
    let same_region = end.region == trip.to_region;
    let d_target = flat(end.at, trip.to);
    if same_region && d_target < NEAR {
        return match worst {
            Some((w, at, ms)) if w > NEAR => Verdict::BouncedForAMoment { worst: w, at, ms },
            _ => Verdict::Ok,
        };
    }
    if same_region && local && flat(end.at, trip.from) < d_target {
        return Verdict::BouncedBack;
    }
    Verdict::Drifted(d_target)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trip(local: bool) -> Trip {
        Trip {
            from_region: "A".into(),
            from: Vec3::new(200.0, 200.0, 30.0),
            to_region: if local { "A" } else { "B" }.into(),
            to: Vec3::new(50.0, 50.0, 30.0),
        }
    }

    fn at(region: &str, x: f32, y: f32, ms: u64) -> Sample {
        Sample {
            region: region.into(),
            at: Vec3::new(x, y, 22.0),
            ms,
        }
    }

    #[test]
    fn at_the_target_and_still_there_is_ok() {
        let t = trip(true);
        let s = [at("A", 50.5, 50.0, 0), at("A", 50.4, 50.1, 20)];
        assert_eq!(judge(&t, true, &s, &at("A", 50.4, 50.1, 1500)), Verdict::Ok);
    }

    #[test]
    fn a_moment_back_toward_the_start_is_a_bounce_even_if_it_ends_at_the_target() {
        let t = trip(true);
        let s = [
            at("A", 50.0, 50.0, 0),
            at("A", 190.0, 190.0, 40),
            at("A", 50.0, 50.0, 60),
        ];
        match judge(&t, true, &s, &at("A", 50.0, 50.0, 1500)) {
            Verdict::BouncedForAMoment { worst, ms, .. } => {
                assert!(worst > 190.0);
                assert_eq!(ms, 40);
            }
            v => panic!("{v:?}"),
        }
    }

    #[test]
    fn ending_near_the_start_is_bounced_back_and_elsewhere_drifted() {
        let t = trip(true);
        assert_eq!(
            judge(&t, true, &[], &at("A", 199.0, 199.0, 1500)),
            Verdict::BouncedBack
        );
        assert!(matches!(
            judge(&t, true, &[], &at("A", 50.0, 90.0, 1500)),
            Verdict::Drifted(d) if (d - 40.0).abs() < 0.01
        ));
    }

    #[test]
    fn to_another_region_there_is_no_way_back_on_the_map() {
        let t = trip(false);
        // the start's coordinates in the new region mean nothing
        let s = [at("B", 199.0, 199.0, 20)];
        assert_eq!(judge(&t, true, &s, &at("B", 50.0, 50.0, 1500)), Verdict::Ok);
        assert!(matches!(
            judge(&t, true, &[], &at("B", 199.0, 199.0, 1500)),
            Verdict::Drifted(_)
        ));
        assert!(matches!(
            judge(&t, true, &[], &at("A", 50.0, 50.0, 1500)),
            Verdict::Drifted(_)
        ));
    }

    #[test]
    fn not_arrived_wins() {
        let t = trip(true);
        assert_eq!(
            judge(&t, false, &[], &at("A", 50.0, 50.0, 0)),
            Verdict::NotArrived
        );
    }
}
