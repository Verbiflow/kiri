use anyhow::{Result, bail};
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ColorMode {
    Auto,
    #[default]
    Always,
    Never,
}

impl FromStr for ColorMode {
    type Err = anyhow::Error;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            _ => bail!("Choose auto, always, or never for --color"),
        }
    }
}

impl ColorMode {
    pub fn enabled(self, no_color: Option<&str>) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::Auto => no_color.is_none_or(|value| value.is_empty()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_choice_overrides_no_color_without_changing_the_environment() {
        assert!(!ColorMode::Auto.enabled(Some("1")));
        assert!(ColorMode::Auto.enabled(None));
        assert!(ColorMode::Auto.enabled(Some("")));
        assert!(ColorMode::Always.enabled(Some("1")));
        assert!(!ColorMode::Never.enabled(None));
    }
}
