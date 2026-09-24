//! Where the settings files live: `~/.config/gh-ui/<file>` (Linux/macOS).
//!
//! Every `load`/`save` goes through `path`, and both already treat `None` as
//! "no config": defaults on load, nothing written on save. Under `cargo test`
//! `path` always answers `None`, so no test can read the user's real settings
//! — making results depend on them — or overwrite them.

use std::path::PathBuf;

/// Path of the settings file `name`, or `None` if there is nowhere to put it.
#[cfg(not(test))]
pub fn path(name: &str) -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("gh-ui").join(name))
}

/// Test build: never a path, so the tests stay off the user's real config.
#[cfg(test)]
pub fn path(_name: &str) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tests_never_touch_the_real_config() {
        assert_eq!(path("filters.json"), None);
    }
}
