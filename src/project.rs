use std::{fmt, str::FromStr};

use crate::{BusyNasError, Result};

/// A path-safe project slug.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ProjectName(String);

impl ProjectName {
    pub fn parse(value: impl AsRef<str>) -> Result<Self> {
        let value = value.as_ref();
        let valid = (1..=63).contains(&value.len())
            && (value.as_bytes()[0].is_ascii_lowercase() || value.as_bytes()[0].is_ascii_digit())
            && value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');

        if valid {
            Ok(Self(value.to_owned()))
        } else {
            Err(BusyNasError::InvalidProjectName(value.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProjectName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl FromStr for ProjectName {
    type Err = BusyNasError;

    fn from_str(value: &str) -> Result<Self> {
        Self::parse(value)
    }
}

#[cfg(test)]
mod tests {
    use super::ProjectName;

    #[test]
    fn accepts_simple_lowercase_slugs() {
        assert_eq!(
            ProjectName::parse("busy-nas-2").unwrap().as_str(),
            "busy-nas-2"
        );
        assert_eq!(
            ProjectName::parse("2026-plan").unwrap().as_str(),
            "2026-plan"
        );
    }

    #[test]
    fn rejects_paths_and_non_slug_names() {
        for value in [
            "",
            ".",
            "..",
            "a/b",
            "a\\b",
            "UPPER",
            "-starts-dash",
            "has_space",
        ] {
            assert!(
                ProjectName::parse(value).is_err(),
                "{value} should be rejected"
            );
        }
    }
}
