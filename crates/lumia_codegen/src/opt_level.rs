//! LLVM codegen opt level, independent of mid-end [`lumia_opt::OptOptions::release`].
//!
//! Product defaults: Debug → [`LlvmOptLevel::O1`] (runnable, `mem2reg`); Release →
//! [`LlvmOptLevel::O3`]. Override from the CLI with `--llvm-opt`.

use inkwell::OptimizationLevel;

/// LLVM new-PM pipeline + instruction-selection level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LlvmOptLevel {
    /// `-O0`: no new-PM pipeline; TargetMachine `None`.
    None,
    /// `default<O1>` — Debug default so binaries are runnable without `--release`.
    O1,
    /// `default<O2>`.
    O2,
    /// `default<O3>` — Release default.
    O3,
}

impl LlvmOptLevel {
    /// Product default: Debug is O1 (runnable); Release is O3.
    #[inline]
    pub fn from_release(release: bool) -> Self {
        if release {
            Self::O3
        } else {
            Self::O1
        }
    }

    /// Parse `--llvm-opt` tokens. `fast` is an alias for `1`, not LLVM `-Ofast`.
    pub fn parse_cli(s: &str) -> Result<Self, String> {
        match s {
            "none" | "0" | "o0" | "O0" => Ok(Self::None),
            "1" | "o1" | "O1" | "fast" => Ok(Self::O1),
            "2" | "o2" | "O2" => Ok(Self::O2),
            "3" | "o3" | "O3" => Ok(Self::O3),
            _ => Err(format!(
                "invalid --llvm-opt `{s}` (expected none|0|1|2|3; `fast` = 1, not LLVM -Ofast)"
            )),
        }
    }

    /// Canonical CLI token (`none` / `1` / `2` / `3`).
    pub fn as_cli(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::O1 => "1",
            Self::O2 => "2",
            Self::O3 => "3",
        }
    }

    /// Instruction-selection / TargetMachine level.
    pub fn inkwell(self) -> OptimizationLevel {
        match self {
            Self::None => OptimizationLevel::None,
            Self::O1 => OptimizationLevel::Less,
            Self::O2 => OptimizationLevel::Default,
            Self::O3 => OptimizationLevel::Aggressive,
        }
    }

    /// New-PM pipeline, or `None` to skip `run_passes`.
    pub fn pass_pipeline(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::O1 => Some("default<O1>"),
            Self::O2 => Some("default<O2>"),
            Self::O3 => Some("default<O3>"),
        }
    }

    /// Loop vectorize / SLP / unroll extra flags (LLVM O1 already keeps these light).
    pub fn aggressive_loop_opts(self) -> bool {
        matches!(self, Self::O2 | Self::O3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_release_defaults() {
        assert_eq!(LlvmOptLevel::from_release(false), LlvmOptLevel::O1);
        assert_eq!(LlvmOptLevel::from_release(true), LlvmOptLevel::O3);
    }

    #[test]
    fn parse_cli_aliases() {
        assert_eq!(LlvmOptLevel::parse_cli("none").unwrap(), LlvmOptLevel::None);
        assert_eq!(LlvmOptLevel::parse_cli("0").unwrap(), LlvmOptLevel::None);
        assert_eq!(LlvmOptLevel::parse_cli("fast").unwrap(), LlvmOptLevel::O1);
        assert_eq!(LlvmOptLevel::parse_cli("O2").unwrap(), LlvmOptLevel::O2);
        assert_eq!(LlvmOptLevel::parse_cli("3").unwrap(), LlvmOptLevel::O3);
        assert!(LlvmOptLevel::parse_cli("ofast").is_err());
    }

    #[test]
    fn pipeline_and_inkwell_line_up() {
        assert!(LlvmOptLevel::None.pass_pipeline().is_none());
        assert_eq!(LlvmOptLevel::O1.pass_pipeline(), Some("default<O1>"));
        assert_eq!(LlvmOptLevel::O3.pass_pipeline(), Some("default<O3>"));
        assert!(!LlvmOptLevel::O1.aggressive_loop_opts());
        assert!(LlvmOptLevel::O3.aggressive_loop_opts());
        assert_eq!(LlvmOptLevel::O1.as_cli(), "1");
    }
}
