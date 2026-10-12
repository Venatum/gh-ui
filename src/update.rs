//! Self-upgrade: whether `gh` has a newer gh-ui to install, and where the
//! upgrade stands. The `gh` calls live in `gh.rs`; this is the pure part.

/// Where the self-upgrade stands. Starts at `None` and stays there when the
/// check finds nothing — or fails: it must never get in the way.
#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    None,
    /// `gh` would install this version: the header offers `U`.
    Available(String),
    /// Confirmed: `gh extension upgrade` runs once gh-ui quits. Not before:
    /// `gh` rewrites the binary in place, which Linux refuses while it runs.
    Scheduled(String),
}

impl Update {
    /// The header chip, or `None` when there is nothing to say.
    pub fn chip(&self) -> Option<String> {
        match self {
            Update::None => None,
            Update::Available(version) => Some(format!("{version} ↑ U")),
            Update::Scheduled(version) => Some(format!("{version} on quit")),
        }
    }
}

/// The version `gh extension upgrade gh-ui --dry-run` would install, read
/// from its `[ui]: would have upgraded from v0.5.1 to v0.5.2`. Anything else
/// — up to date, a pinned install, a wording we do not know — is `None`:
/// the chip only shows when `U` can actually work. (A local install never
/// gets here: `gh` exits 1 on it, so the dry run already failed.)
pub fn parse_dry_run(stdout: &str) -> Option<String> {
    stdout.lines().find_map(|line| {
        let (_, versions) = line.split_once("would have upgraded from ")?;
        let (_, to) = versions.rsplit_once(" to ")?;
        let to = to.trim();
        is_version(to).then(|| to.to_string())
    })
}

/// What a release tag or a commit is made of (`v0.5.2`, `v1.0.0-rc.1`,
/// `1`, `4f1c2e9a`). Anything more means `gh` reworded its line: no chip,
/// rather than one that carries the rest of the sentence.
fn is_version(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_dry_run_finds_the_new_version() {
        assert_eq!(
            parse_dry_run("[ui]: would have upgraded from v0.5.1 to v0.5.2\n"),
            Some("v0.5.2".to_string())
        );
        // `gh`'s own tests: a version without the `v` prefix.
        assert_eq!(
            parse_dry_run("[hello]: would have upgraded from 0 to 1\n"),
            Some("1".to_string())
        );
    }

    #[test]
    fn parse_dry_run_reads_the_line_that_matters() {
        assert_eq!(
            parse_dry_run("\r\n[ui]: would have upgraded from v0.5.1 to v0.5.2\r\n"),
            Some("v0.5.2".to_string())
        );
    }

    #[test]
    fn nothing_to_install_means_no_version() {
        assert_eq!(parse_dry_run("[ui]: already up to date\n"), None);
        // A pinned extension: `U` could not work, so no chip.
        assert_eq!(
            parse_dry_run("[ui]: pinned extensions can not be upgraded\n"),
            None
        );
        // A wording we do not know.
        assert_eq!(parse_dry_run("[ui]: something new\n"), None);
        // `gh extension install .`: `gh` exits 1 and says why on stderr, so
        // the dry run fails before this; its stdout would be empty anyway.
        assert_eq!(parse_dry_run(""), None);
    }

    #[test]
    fn a_reworded_line_shows_no_chip() {
        // Something after the version: not a version any more.
        assert_eq!(
            parse_dry_run("[ui]: would have upgraded from v0.5.1 to v0.5.2 (latest)\n"),
            None
        );
        // What tags and commits look like still reads.
        assert_eq!(
            parse_dry_run("[ui]: would have upgraded from v0.5.1 to v1.0.0-rc.1\n"),
            Some("v1.0.0-rc.1".to_string())
        );
        assert_eq!(
            parse_dry_run("[ui]: would have upgraded from 1a2b3c4d to 4f1c2e9a\n"),
            Some("4f1c2e9a".to_string())
        );
    }

    #[test]
    fn the_chip_follows_the_upgrade() {
        let v = || "v0.5.2".to_string();
        assert_eq!(Update::None.chip(), None);
        assert_eq!(Update::Available(v()).chip().as_deref(), Some("v0.5.2 ↑ U"));
        assert_eq!(
            Update::Scheduled(v()).chip().as_deref(),
            Some("v0.5.2 on quit")
        );
    }
}
