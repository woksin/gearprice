//! Where gearprice keeps its cache, its configuration and its update stamp.
//!
//! Every location is overridable by environment variable so a test run, a CI job or a
//! throwaway shell can be pointed somewhere harmless without touching the real cache.
//! The resolution itself is a pure function of the environment it is handed, which is
//! what the tests exercise — the process environment is never mutated.

use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

/// A lookup of environment variables, so resolution can be tested without touching the
/// real process environment.
pub trait Environment {
    fn get(&self, key: &str) -> Option<OsString>;
}

pub struct ProcessEnvironment;

impl Environment for ProcessEnvironment {
    fn get(&self, key: &str) -> Option<OsString> {
        env::var_os(key)
    }
}

impl Environment for Vec<(&str, &str)> {
    fn get(&self, key: &str) -> Option<OsString> {
        self.iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| OsString::from(*value))
    }
}

pub fn home_dir_in(environment: &impl Environment) -> PathBuf {
    environment
        .get("HOME")
        .or_else(|| environment.get("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Directory holding cached Reverb responses and the update-check stamp.
pub fn cache_dir_in(environment: &impl Environment) -> PathBuf {
    if let Some(path) = environment.get("GEARPRICE_CACHE_DIR") {
        return PathBuf::from(path);
    }
    if let Some(path) = environment.get("XDG_CACHE_HOME") {
        return PathBuf::from(path).join("gearprice");
    }
    if cfg!(windows)
        && let Some(path) = environment.get("LOCALAPPDATA")
    {
        return PathBuf::from(path).join("gearprice/cache");
    }
    home_dir_in(environment).join(".cache/gearprice")
}

pub fn cache_dir() -> PathBuf {
    cache_dir_in(&ProcessEnvironment)
}

pub fn response_cache_dir() -> PathBuf {
    cache_dir().join("responses")
}

/// The user's configuration file.
///
/// Under the config directory rather than the cache: this one is written by hand and
/// nothing regenerates it, so `gearprice cache --clear` must never be able to reach it.
///
/// `GEARPRICE_CONFIG` names the file itself, not a directory holding it, so a caller can
/// point a run at one specific set of settings.
pub fn config_path_in(environment: &impl Environment) -> PathBuf {
    if let Some(path) = environment.get("GEARPRICE_CONFIG") {
        return PathBuf::from(path);
    }
    if let Some(path) = environment.get("XDG_CONFIG_HOME") {
        return PathBuf::from(path).join("gearprice/config.toml");
    }
    home_dir_in(environment).join(".config/gearprice/config.toml")
}

pub fn config_path() -> PathBuf {
    config_path_in(&ProcessEnvironment)
}

pub fn update_check_path() -> PathBuf {
    if let Some(path) = env::var_os("GEARPRICE_UPDATE_CACHE") {
        return PathBuf::from(path);
    }
    cache_dir().join("update-check.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_cache_directory_wins_over_every_default() {
        let environment = vec![
            ("GEARPRICE_CACHE_DIR", "/tmp/gearprice-test"),
            ("XDG_CACHE_HOME", "/tmp/xdg"),
            ("HOME", "/home/test"),
        ];
        assert_eq!(
            PathBuf::from("/tmp/gearprice-test"),
            cache_dir_in(&environment)
        );
    }

    #[test]
    fn cache_directory_falls_back_through_xdg_to_home() {
        let xdg = vec![("XDG_CACHE_HOME", "/tmp/xdg"), ("HOME", "/home/test")];
        assert_eq!(PathBuf::from("/tmp/xdg/gearprice"), cache_dir_in(&xdg));

        let home = vec![("HOME", "/home/test")];
        assert_eq!(
            PathBuf::from("/home/test/.cache/gearprice"),
            cache_dir_in(&home)
        );
    }

    #[test]
    fn an_explicit_configuration_file_wins_over_xdg_and_home() {
        let environment = vec![
            ("GEARPRICE_CONFIG", "/tmp/elsewhere.toml"),
            ("XDG_CONFIG_HOME", "/tmp/xdg"),
            ("HOME", "/home/test"),
        ];
        assert_eq!(
            PathBuf::from("/tmp/elsewhere.toml"),
            config_path_in(&environment)
        );
    }

    #[test]
    fn the_configuration_file_falls_back_through_xdg_to_home() {
        let xdg = vec![("XDG_CONFIG_HOME", "/tmp/xdg"), ("HOME", "/home/test")];
        assert_eq!(
            PathBuf::from("/tmp/xdg/gearprice/config.toml"),
            config_path_in(&xdg)
        );

        let home = vec![("HOME", "/home/test")];
        assert_eq!(
            PathBuf::from("/home/test/.config/gearprice/config.toml"),
            config_path_in(&home)
        );
    }
}
