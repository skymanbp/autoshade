//! The reverse-fit's stepped strength: ONE table of five steps, each naming
//! the honesty budget and the optional passes it turns on.
//!
//! The desktop app shows this as its single 「Reverse-fit strength」 control,
//! and `autoshade match --level` reads the same rows, so a step means the same
//! fit on every surface. The individual knobs (`--strength`, `--zoned`,
//! `--regions`, `--ai-judge`, `--deep`) stay available on the command line;
//! a step is a shorthand for one combination of them, never a new behaviour.
//!
//! [`FitLevel::Standard`] is the default and reproduces the desktop app's
//! historical defaults exactly: the calibrated strength
//! ([`GradeStrength::DEFAULT`]), local areas on with the historical two
//! semantic regions, and no paid review.

use crate::fit_zoned::semantic::{DEFAULT_SEMANTIC_REGIONS, MAX_SEMANTIC_REGIONS};
use crate::recipe::GradeStrength;

/// One step of the reverse-fit strength (see the module docs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FitLevel {
    /// Whole-frame fit at the calibrated strength; no local areas.
    Light,
    /// The default: whole-frame fit at the calibrated strength plus local
    /// areas (the historical two semantic regions).
    #[default]
    Standard,
    /// A wider budget (85 %: unsupported moves are disclosed, not withheld)
    /// plus local areas.
    Strong,
    /// The widest budget (100 %) plus local areas with up to four semantic
    /// regions.
    Full,
    /// [`FitLevel::Full`] plus the paid AI review, which may act once (up to
    /// two vision calls).
    Reviewed,
}

/// What one [`FitLevel`] turns on — the five values the fit reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitLevelSettings {
    /// The honesty budget, 0..1 (fed to [`GradeStrength::new`]).
    pub strength: f32,
    /// Fit local areas (sky, tonal ranges, spatial tiles) after the global fit.
    pub zoned: bool,
    /// Maximum accepted semantic regions for the local-area pass.
    pub regions: usize,
    /// Score the fit with the paid vision review.
    pub ai_judge: bool,
    /// Let that review act once (implies `ai_judge`).
    pub deep: bool,
}

/// Strength of [`FitLevel::Strong`]: the first point at which the budget
/// discloses unsupported moves instead of withholding them.
pub const STRONG_STRENGTH: f32 = 0.85;
/// Strength of [`FitLevel::Full`] and [`FitLevel::Reviewed`]: the top of the dial.
pub const FULL_STRENGTH: f32 = 1.0;
/// [`FitLevel::from_switches`]' strength cut-offs: the midpoints between the
/// steps' strengths (0.65 | 0.85 | 1.0), written out so the comparison is
/// exact in `f32`.
const STRONG_FROM: f32 = 0.75;
const FULL_FROM: f32 = 0.925;

impl FitLevel {
    /// Every step, in order (step 1 first).
    pub const ALL: [FitLevel; 5] =
        [FitLevel::Light, FitLevel::Standard, FitLevel::Strong, FitLevel::Full, FitLevel::Reviewed];

    /// The table. Step 2 equals the desktop app's historical defaults.
    pub fn settings(self) -> FitLevelSettings {
        let (strength, zoned, regions, review) = match self {
            FitLevel::Light => (GradeStrength::DEFAULT, false, DEFAULT_SEMANTIC_REGIONS, false),
            FitLevel::Standard => (GradeStrength::DEFAULT, true, DEFAULT_SEMANTIC_REGIONS, false),
            FitLevel::Strong => (STRONG_STRENGTH, true, DEFAULT_SEMANTIC_REGIONS, false),
            FitLevel::Full => (FULL_STRENGTH, true, MAX_SEMANTIC_REGIONS, false),
            FitLevel::Reviewed => (FULL_STRENGTH, true, MAX_SEMANTIC_REGIONS, true),
        };
        FitLevelSettings { strength, zoned, regions, ai_judge: review, deep: review }
    }

    /// The step's number, 1..=5.
    pub fn step(self) -> u8 {
        match self {
            FitLevel::Light => 1,
            FitLevel::Standard => 2,
            FitLevel::Strong => 3,
            FitLevel::Full => 4,
            FitLevel::Reviewed => 5,
        }
    }

    /// The step with this number; `None` outside 1..=5.
    pub fn from_step(step: u8) -> Option<FitLevel> {
        FitLevel::ALL.get(usize::from(step).checked_sub(1)?).copied()
    }

    /// The command-line word for the step (`match --level <word>`).
    pub fn word(self) -> &'static str {
        match self {
            FitLevel::Light => "light",
            FitLevel::Standard => "standard",
            FitLevel::Strong => "strong",
            FitLevel::Full => "full",
            FitLevel::Reviewed => "reviewed",
        }
    }

    /// The step that best stands for a set of separate switches — how a
    /// settings file written before the stepped control existed is read.
    /// The paid review decides first (it is the only step that spends), then
    /// the four-region pass, then local areas, then the strength.
    pub fn from_switches(strength: f32, zoned: bool, four_regions: bool, ai_judge: bool) -> FitLevel {
        if ai_judge {
            FitLevel::Reviewed
        } else if four_regions {
            FitLevel::Full
        } else if !zoned {
            FitLevel::Light
        } else if strength >= FULL_FROM {
            FitLevel::Full
        } else if strength >= STRONG_FROM {
            FitLevel::Strong
        } else {
            FitLevel::Standard
        }
    }
}

impl std::str::FromStr for FitLevel {
    type Err = String;

    /// `1`..`5` or the step's word (case-insensitive).
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if let Ok(n) = s.parse::<u8>() {
            return FitLevel::from_step(n).ok_or_else(|| format!("level {n} is not between 1 and 5"));
        }
        FitLevel::ALL.into_iter().find(|l| l.word().eq_ignore_ascii_case(s)).ok_or_else(|| {
            let words: Vec<_> = FitLevel::ALL.iter().map(|l| l.word()).collect();
            format!("`{s}` is not a level: use 1-5 or one of {}", words.join(", "))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each step's row, read back field by field.
    ///
    /// MUTATION THIS CATCHES: any cell of the table changed (a strength, a
    /// switch, a region count) fails its row here.
    #[test]
    fn each_step_turns_on_what_its_row_names() {
        let row = |l: FitLevel| {
            let s = l.settings();
            (s.strength, s.zoned, s.regions, s.ai_judge, s.deep)
        };
        assert_eq!(row(FitLevel::Light), (0.65, false, 2, false, false));
        assert_eq!(row(FitLevel::Standard), (0.65, true, 2, false, false));
        assert_eq!(row(FitLevel::Strong), (0.85, true, 2, false, false));
        assert_eq!(row(FitLevel::Full), (1.0, true, 4, false, false));
        assert_eq!(row(FitLevel::Reviewed), (1.0, true, 4, true, true));
        // The strength only ever rises and every switch only turns on along
        // the steps, so a higher step never does less than a lower one.
        for w in FitLevel::ALL.windows(2) {
            let (a, b) = (w[0].settings(), w[1].settings());
            assert!(b.strength >= a.strength && b.regions >= a.regions, "{w:?}");
            assert!(b.zoned >= a.zoned && b.ai_judge >= a.ai_judge && b.deep >= a.deep, "{w:?}");
        }
    }

    /// The default step is the desktop app's historical default fit, read off
    /// the same constants the defaults were made of.
    ///
    /// MUTATION THIS CATCHES: `#[default]` moved off `Standard`, or the
    /// Standard row's strength / region count written as a literal that drifts
    /// from `GradeStrength::DEFAULT` / `DEFAULT_SEMANTIC_REGIONS`.
    #[test]
    fn the_default_step_is_the_historical_default_fit() {
        assert_eq!(FitLevel::default(), FitLevel::Standard);
        let s = FitLevel::default().settings();
        assert_eq!(s.strength, GradeStrength::DEFAULT);
        assert!(s.zoned);
        assert_eq!(s.regions, DEFAULT_SEMANTIC_REGIONS);
        assert!(!s.ai_judge && !s.deep, "spending is never a default");
    }

    /// Numbers, words and the switch migration agree with the table.
    ///
    /// MUTATION THIS CATCHES: a threshold in `from_switches` moved (0.925 /
    /// 0.75), its order changed (review no longer first), or `from_step` /
    /// `FromStr` off by one.
    #[test]
    fn steps_parse_and_old_switches_map_to_the_nearest_step() {
        for l in FitLevel::ALL {
            assert_eq!(FitLevel::from_step(l.step()), Some(l));
            assert_eq!(l.step().to_string().parse::<FitLevel>(), Ok(l));
            assert_eq!(l.word().parse::<FitLevel>(), Ok(l));
            assert_eq!(l.word().to_uppercase().parse::<FitLevel>(), Ok(l));
            // Each step's own switches read back as that step.
            let s = l.settings();
            assert_eq!(FitLevel::from_switches(s.strength, s.zoned, s.regions > 2, s.ai_judge), l);
        }
        assert_eq!(FitLevel::from_step(0), None);
        assert_eq!(FitLevel::from_step(6), None);
        assert!("0".parse::<FitLevel>().is_err() && "max".parse::<FitLevel>().is_err());
        let d = GradeStrength::DEFAULT;
        // review on → 5, whatever else is set
        assert_eq!(FitLevel::from_switches(0.2, false, false, true), FitLevel::Reviewed);
        // four regions → 4
        assert_eq!(FitLevel::from_switches(d, false, true, false), FitLevel::Full);
        // not zoned → 1, even at a high strength
        assert_eq!(FitLevel::from_switches(1.0, false, false, false), FitLevel::Light);
        // zoned: by strength
        assert_eq!(FitLevel::from_switches(0.925, true, false, false), FitLevel::Full);
        assert_eq!(FitLevel::from_switches(0.92, true, false, false), FitLevel::Strong);
        assert_eq!(FitLevel::from_switches(0.75, true, false, false), FitLevel::Strong);
        assert_eq!(FitLevel::from_switches(0.74, true, false, false), FitLevel::Standard);
        assert_eq!(FitLevel::from_switches(0.0, true, false, false), FitLevel::Standard);
    }
}
