/// Local build and environment artifacts.
pub const EXCLUDES: &[&str] = &[
    "node_modules/",
    "target/",
    ".direnv/",
    ".devenv/",
    "result",
    "result-*",
];

pub fn rsync_filter_args() -> Vec<String> {
    EXCLUDES
        .iter()
        .flat_map(|pattern| ["--exclude".to_owned(), (*pattern).to_owned()])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{rsync_filter_args, EXCLUDES};

    #[test]
    fn filters_are_constructed_as_rsync_argument_pairs() {
        let args = rsync_filter_args();
        assert_eq!(args.len(), EXCLUDES.len() * 2);
        assert_eq!(
            &args[..4],
            ["--exclude", "node_modules/", "--exclude", "target/"]
        );
        assert!(args
            .windows(2)
            .any(|pair| pair == ["--exclude", "result-*"]));
    }
}
