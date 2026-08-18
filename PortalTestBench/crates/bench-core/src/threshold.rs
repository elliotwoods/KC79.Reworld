//! Choosing the optical home-switch threshold.
//!
//! # Why this is not a constant
//!
//! The home feature is a **reflector** -- a bright spot on a dark ring -- and the comparator
//! threshold is a duty count that must be *crossed*. Crossing duty is therefore **inverse to
//! reflectance**: the switch reads active when the local crossing duty is *below* the
//! threshold, and a lower crossing means a more reflective surface.
//!
//! On the pre-production ring the whole reflection profile drifted 15-25 duty counts across
//! days, which already made a fixed threshold untenable. On the final injection-moulded ring
//! it is worse than drift: the dark moulding **never crosses at any threshold in 0..=255**,
//! covered or uncovered. The background is not merely noisy, it is *unmeasurable* -- so the
//! old `T = background - k` rule and the `background - 6` depth gate are not miscalibrated,
//! they are structurally inapplicable, permanently.
//!
//! What can still be measured is the range of thresholds at which the flag itself is seen. A
//! census sweep finds a contiguous segment: below `floor` the flag stops registering, above
//! `shoulder` the comparator dithers on noise. That segment is the only ground truth
//! available, and the operating point is taken from *within* it.
//!
//! # The rule
//!
//! `T_op = floor + round(0.55 * span)`, slightly above centre. Validated against the final
//! ring: the rule picks 247 where the empirical optimum was 246-248, and an 18-hour bake of
//! 3,593 homes in that range had zero failures.
//!
//! # Measuring the band
//!
//! Only a **census** may feed this -- a fixed, settled threshold read by the moving
//! comparator, which is what homing actually does. A grid scan sweeps the RC-filtered
//! threshold DAC (tau ~= 100 ms) and reads 10-20 counts high; a settled binary-search probe
//! returned "not found" while parked on a flag the census found every lap. Instrument trust
//! order is census > static level ladder > grid scan > settled probe.

use serde::{Deserialize, Serialize};

/// The contiguous range of thresholds at which the home flag registers.
///
/// `floor` and `shoulder` are inclusive duty counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Band {
    /// Lowest threshold at which the flag still registers.
    pub floor: u8,
    /// Highest threshold before the comparator starts dithering on noise.
    pub shoulder: u8,
}

/// Why a measured band cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum BandError {
    #[error("threshold band is inverted: floor {floor} is above shoulder {shoulder}")]
    Inverted { floor: u8, shoulder: u8 },

    /// A band this narrow has no room for drift between calibration and use.
    ///
    /// Measured reference points: the unpainted production ring holds 9-11 counts overnight in
    /// an unlit room, but collapses to **2** under a physical cover; a silver-painted flag
    /// gives about 52. Two counts is not a working band -- it is one thermal minute from being
    /// none.
    #[error(
        "threshold band is only {width} counts wide (need {min}); re-run the census, and if it \
         is genuinely this narrow the flag needs paint or the cover needs removing"
    )]
    TooNarrow { width: u16, min: u16 },
}

/// Narrowest band this code will operate on. See [`BandError::TooNarrow`].
pub const MIN_BAND_WIDTH: u16 = 4;

/// Where in the band the operating point sits. Slightly above centre; see the module docs.
const BAND_POSITION: f32 = 0.55;

impl Band {
    pub fn new(floor: u8, shoulder: u8) -> Result<Self, BandError> {
        if floor > shoulder {
            return Err(BandError::Inverted { floor, shoulder });
        }
        let band = Band { floor, shoulder };
        let width = band.width();
        if width < MIN_BAND_WIDTH {
            return Err(BandError::TooNarrow { width, min: MIN_BAND_WIDTH });
        }
        Ok(band)
    }

    /// Width in duty counts. A floor equal to the shoulder is one count wide, not zero.
    pub fn width(self) -> u16 {
        u16::from(self.shoulder) - u16::from(self.floor) + 1
    }

    /// The threshold to home at.
    ///
    /// Placed at 55% of the span rather than its centre, and rounded to nearest.
    pub fn operating(self) -> u8 {
        let span = f32::from(self.width() - 1);
        let offset = (span * BAND_POSITION).round() as u16;
        // `offset` is at most `span`, so this can never carry `floor` past `shoulder`.
        (u16::from(self.floor) + offset) as u8
    }

    /// Build the band from a census: the thresholds at which the flag registered.
    ///
    /// Takes the **longest contiguous run**, not the outer extremes. A stray detection above
    /// the shoulder is comparator dither, and letting it widen the band drags the operating
    /// point up into the region where homing latches phantom features.
    pub fn from_census(detected: &[u8]) -> Result<Self, BandError> {
        if detected.is_empty() {
            return Err(BandError::TooNarrow { width: 0, min: MIN_BAND_WIDTH });
        }
        let mut sorted = detected.to_vec();
        sorted.sort_unstable();
        sorted.dedup();

        let mut best_start = 0usize;
        let mut best_len = 1usize;
        let mut start = 0usize;
        let mut len = 1usize;
        for i in 1..sorted.len() {
            if sorted[i] == sorted[i - 1] + 1 {
                len += 1;
            } else {
                start = i;
                len = 1;
            }
            if len > best_len {
                best_len = len;
                best_start = start;
            }
        }
        Band::new(sorted[best_start], sorted[best_start + best_len - 1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The final injection-moulded ring, uncovered, measured 2026-08-14: floor 240-242,
    /// shoulder 252. The rule must land inside 246-248, which is the empirical optimum and
    /// where the 3,593-home overnight bake ran without a single failure.
    #[test]
    fn production_ring_operating_point_is_in_the_measured_optimum() {
        for floor in [240u8, 241, 242] {
            let op = Band::new(floor, 252).unwrap().operating();
            assert!((246..=248).contains(&op), "floor {floor} gave {op}, outside 246..=248");
        }
        assert_eq!(Band::new(240, 252).unwrap().width(), 13);
    }

    /// A silver-painted flag opens the band to about 52 counts (200..=252). The operating
    /// point should land near the middle of that; the measured choice was 226.
    #[test]
    fn painted_flag_operating_point_is_near_band_centre() {
        let band = Band::new(200, 252).unwrap();
        assert_eq!(band.width(), 53);
        assert_eq!(band.operating(), 229);
    }

    /// Under a physical cover the unpainted band collapses to about two counts. That must be
    /// refused rather than homed on: it leaves no room for a single count of thermal drift.
    #[test]
    fn covered_ring_band_is_refused() {
        let err = Band::new(252, 253).unwrap_err();
        assert!(matches!(err, BandError::TooNarrow { width: 2, .. }));
    }

    #[test]
    fn inverted_band_is_refused() {
        let err = Band::new(250, 240).unwrap_err();
        assert!(matches!(err, BandError::Inverted { floor: 250, shoulder: 240 }));
    }

    /// A census that also caught one dithering sample at 255 must not be widened by it.
    /// Including it would push the operating point up toward the region where the comparator
    /// latches 90-degree-wide phantom flags.
    #[test]
    fn census_ignores_dither_outliers() {
        let detected = [240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 255];
        let band = Band::from_census(&detected).unwrap();
        assert_eq!(band, Band { floor: 240, shoulder: 252 });
        assert!((246..=248).contains(&band.operating()));
    }

    #[test]
    fn census_takes_the_longest_run_not_the_first() {
        let detected = [10, 11, 12, 240, 241, 242, 243, 244, 245, 246];
        let band = Band::from_census(&detected).unwrap();
        assert_eq!(band, Band { floor: 240, shoulder: 246 });
    }

    #[test]
    fn empty_census_is_refused() {
        assert!(Band::from_census(&[]).is_err());
    }

    /// The operating point is always inside the band it came from -- the property that matters
    /// more than any particular number.
    #[test]
    fn operating_point_is_always_within_the_band() {
        for floor in 0u8..=250 {
            for width in MIN_BAND_WIDTH..=40u16 {
                let Some(shoulder) = floor.checked_add((width - 1) as u8) else {
                    continue;
                };
                let band = Band::new(floor, shoulder).unwrap();
                let op = band.operating();
                assert!(op >= floor && op <= shoulder, "{op} outside {floor}..={shoulder}");
            }
        }
    }
}
