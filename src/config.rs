//! The user's configuration file: the flags they would otherwise retype every run.
//!
//! Someone pricing gear in Norway wants `--currency NOK --ships-to NO` on every single
//! invocation, and a tool that makes you type the same two flags forever is a tool people
//! stop reaching for. The file supplies those as defaults.
//!
//! It only ever supplies defaults. Precedence follows clig.dev — a command-line flag beats
//! an environment variable, which beats this file, which beats gearprice's own built-in
//! defaults — and [`resolve`] is the one place that ordering is written down.
//!
//! Nothing here is allowed to fail a run. A missing file is an empty configuration, and a
//! broken one is a warning carried alongside the defaults: a stray character in a
//! preferences file is no reason to refuse to price a guitar.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde::Deserialize;

use crate::paths::{Environment, ProcessEnvironment, config_path_in};

/// Settings a user can pin in their configuration file.
///
/// Every field is optional and every field names an existing flag, so what the file can
/// say is exactly what the command line can say — there is no second vocabulary to learn.
/// The enum-valued ones are kept as strings so a typo in the file surfaces as the same
/// message `--condition nonsense` would give, rather than as a deserialisation failure
/// that discards the whole file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
pub struct Config {
    pub currency: Option<String>,
    #[serde(alias = "ships-to")]
    pub ships_to: Option<String>,
    pub condition: Option<String>,
    pub region: Option<String>,
    pub format: Option<String>,
    #[serde(alias = "cache-ttl")]
    pub cache_ttl: Option<u64>,
    #[serde(alias = "no-pager")]
    pub no_pager: Option<bool>,
    #[serde(alias = "sold-sample")]
    pub sold_sample: Option<u32>,
}

/// Every key the file may contain, in both the underscored spelling and the hyphenated one
/// the flags use, because someone copying `--ships-to` into a file will keep the hyphen.
/// Anything outside this list is a typo worth naming.
const KNOWN_KEYS: [&str; 12] = [
    "currency",
    "ships_to",
    "ships-to",
    "condition",
    "region",
    "format",
    "cache_ttl",
    "cache-ttl",
    "no_pager",
    "no-pager",
    "sold_sample",
    "sold-sample",
];

/// A configuration file as read, with whatever was wrong with it recorded rather than
/// raised.
///
/// The warnings are shaped for [`Diagnostics`](crate::report::Diagnostics): plain
/// sentences the caller can append to its warning list and show alongside the answer.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Loaded {
    pub config: Config,
    pub warnings: Vec<String>,
}

/// Reads the configuration file for this process.
pub fn load() -> Loaded {
    load_in(&ProcessEnvironment)
}

/// Reads the configuration file the given environment points at.
pub fn load_in(environment: &impl Environment) -> Loaded {
    load_from(&config_path_in(environment))
}

/// Reads one configuration file by path.
///
/// An absent file is the ordinary case — most people never write one — so it is silently
/// an empty configuration. Every other failure is reported, because a file that exists and
/// is being ignored is something the user needs told about.
pub fn load_from(path: &Path) -> Loaded {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text, path),
        Err(error) if error.kind() == ErrorKind::NotFound => Loaded::default(),
        Err(error) => Loaded {
            config: Config::default(),
            warnings: vec![format!(
                "cannot read the configuration file {}: {error} — carrying on with gearprice's \
                 own defaults",
                path.display()
            )],
        },
    }
}

/// Parses configuration text. `path` only names the file in any warning.
///
/// The text is read twice: once as a bare table, to see which keys are actually present,
/// and once as a [`Config`], to get the values. Serde discards unknown keys silently, so
/// without the first pass a mistyped `curency` would be indistinguishable from an empty
/// file — which is the one failure a preferences file must not have.
pub fn parse(text: &str, path: &Path) -> Loaded {
    let table = match text.parse::<toml::Table>() {
        Ok(table) => table,
        Err(error) => {
            return Loaded {
                config: Config::default(),
                warnings: vec![format!(
                    "ignoring the configuration file {}: it is not valid TOML — {} — so this run \
                     uses gearprice's own defaults",
                    path.display(),
                    describe(&error, text)
                )],
            };
        }
    };

    let mut warnings = Vec::new();
    let unknown: Vec<&str> = table
        .keys()
        .map(String::as_str)
        .filter(|key| !KNOWN_KEYS.contains(key))
        .collect();
    if !unknown.is_empty() {
        warnings.push(format!(
            "the configuration file {} sets {}, which gearprice does not recognise — check the \
             spelling; those settings are being ignored",
            path.display(),
            quoted_list(&unknown)
        ));
    }

    let config = match toml::from_str::<Config>(text) {
        Ok(config) => config,
        Err(error) => {
            warnings.push(format!(
                "ignoring the configuration file {}: {} — so this run uses gearprice's own \
                 defaults",
                path.display(),
                describe(&error, text)
            ));
            Config::default()
        }
    };

    Loaded { config, warnings }
}

/// Resolves one setting the way clig.dev orders it: the flag, then the environment, then
/// the configuration file, then the built-in default.
///
/// Clap already folds the first two together for any argument declared with `env = …`, so
/// a caller using that passes clap's value as `flag` and `None` as `environment`.
pub fn resolve<T>(flag: Option<T>, environment: Option<T>, file: Option<T>, default: T) -> T {
    flag.or(environment).or(file).unwrap_or(default)
}

/// A commented sample file, for `gearprice config --example`.
///
/// Every setting is present but commented out, so the sample can be redirected straight
/// into the config path and edited by uncommenting rather than by remembering key names.
pub fn example_config() -> &'static str {
    "\
# gearprice configuration.
#
# Defaults for the flags you would otherwise retype every run. Anything given on the
# command line, or in a GEARPRICE_* environment variable, still wins over what is set
# here.
#
# Read from $XDG_CONFIG_HOME/gearprice/config.toml, or ~/.config/gearprice/config.toml
# when that is unset. Set GEARPRICE_CONFIG to read it from somewhere else instead.
#
# Uncomment a line to use it.

# Currency to report in. Reverb converts every listing to it.
# currency = \"NOK\"

# Only listings whose seller ships to here, priced delivered. A country code.
# ships_to = \"NO\"

# Which condition to price: all, used, new, or a single grade such as excellent.
# condition = \"used\"

# Only listings from sellers in this region, as a country code.
# region = \"US\"

# How to print the answer: table, json or csv.
# format = \"table\"

# How long a cached Reverb response stays good, in minutes.
# cache_ttl = 360

# Never page long output, even on a terminal.
# no_pager = false

# How many completed sales to read when working out what gear actually sells for.
# sold_sample = 100
"
}

/// A TOML error as one line.
///
/// [`toml::de::Error`]'s own `Display` is a three-line snippet with a caret under the
/// offending character, which reads well on its own and badly in a list of warnings beside
/// everything else the run noticed. The line number carries enough to find the problem.
fn describe(error: &toml::de::Error, text: &str) -> String {
    match error.span() {
        Some(span) => format!("{} (line {})", error.message(), line_of(text, span.start)),
        None => error.message().to_string(),
    }
}

/// The one-based line a byte offset falls on.
///
/// Counts bytes rather than slicing the string, so an offset landing mid-character — a
/// caret pointed at the middle of a `å` in a Norwegian brand name — cannot panic.
fn line_of(text: &str, offset: usize) -> usize {
    let end = offset.min(text.len());
    text.as_bytes()[..end]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1
}

fn quoted_list(keys: &[&str]) -> String {
    keys.iter()
        .map(|key| format!("`{key}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn parsed(text: &str) -> Loaded {
        parse(text, Path::new("config.toml"))
    }

    #[test]
    fn a_missing_file_is_an_empty_configuration_and_not_a_complaint() {
        let directory = tempfile::tempdir().unwrap();
        let loaded = load_from(&directory.path().join("nothing-here.toml"));
        assert_eq!(Config::default(), loaded.config);
        assert!(loaded.warnings.is_empty());
    }

    #[test]
    fn a_norwegian_can_stop_retyping_currency_and_ships_to() {
        let loaded = parsed("currency = \"NOK\"\nships_to = \"NO\"\n");
        assert!(loaded.warnings.is_empty());
        assert_eq!(Some("NOK".to_string()), loaded.config.currency);
        assert_eq!(Some("NO".to_string()), loaded.config.ships_to);
    }

    #[test]
    fn every_setting_survives_the_file_in_either_spelling() {
        let loaded = parsed(
            "currency = \"NOK\"\n\
             ships-to = \"NO\"\n\
             condition = \"excellent\"\n\
             region = \"GB\"\n\
             format = \"json\"\n\
             cache-ttl = 60\n\
             no-pager = true\n\
             sold-sample = 250\n",
        );
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        assert_eq!(
            Config {
                currency: Some("NOK".into()),
                ships_to: Some("NO".into()),
                condition: Some("excellent".into()),
                region: Some("GB".into()),
                format: Some("json".into()),
                cache_ttl: Some(60),
                no_pager: Some(true),
                sold_sample: Some(250),
            },
            loaded.config
        );
    }

    #[test]
    fn the_example_is_valid_toml_and_says_nothing_by_itself() {
        let loaded = parsed(example_config());
        assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
        // Every setting is commented out, so pasting the sample in changes nothing until
        // the user actually chooses something.
        assert_eq!(Config::default(), loaded.config);
    }

    #[test]
    fn a_valid_file_on_disk_reads_back_through_the_environment() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "currency = \"NOK\"\n").unwrap();
        let environment = vec![("GEARPRICE_CONFIG", path.to_str().unwrap())];
        let loaded = load_in(&environment);
        assert!(loaded.warnings.is_empty());
        assert_eq!(Some("NOK".to_string()), loaded.config.currency);
    }

    #[test]
    fn a_malformed_config_is_a_warning_rather_than_a_dead_run() {
        let loaded = parsed("currency = \"NOK\"\nships_to = \n");
        assert_eq!(Config::default(), loaded.config);
        assert_eq!(1, loaded.warnings.len());
        let warning = &loaded.warnings[0];
        assert!(warning.contains("config.toml"), "{warning}");
        assert!(warning.contains("not valid TOML"), "{warning}");
        assert!(warning.contains("line 2"), "{warning}");
    }

    #[test]
    fn a_setting_of_the_wrong_type_costs_the_file_and_not_the_run() {
        let loaded = parsed("cache_ttl = \"six hours\"\n");
        assert_eq!(Config::default(), loaded.config);
        assert_eq!(1, loaded.warnings.len());
        assert!(
            loaded.warnings[0].contains("config.toml"),
            "{:?}",
            loaded.warnings
        );
    }

    #[test]
    fn an_unknown_key_is_named_so_the_typo_can_be_found() {
        let loaded = parsed("curency = \"NOK\"\nshpis_to = \"NO\"\n");
        assert_eq!(1, loaded.warnings.len());
        let warning = &loaded.warnings[0];
        assert!(warning.contains("`curency`"), "{warning}");
        assert!(warning.contains("`shpis_to`"), "{warning}");
        // The rest of the file is still honoured; one typo does not void the lot.
        assert_eq!(None, loaded.config.currency);
    }

    #[test]
    fn a_typo_beside_a_good_setting_keeps_the_good_setting() {
        let loaded = parsed("curency = \"NOK\"\nships_to = \"NO\"\n");
        assert_eq!(1, loaded.warnings.len());
        assert_eq!(Some("NO".to_string()), loaded.config.ships_to);
    }

    #[test]
    fn an_unreadable_file_is_reported_rather_than_silently_ignored() {
        // A directory where a file is expected: it exists, so silence would leave the user
        // wondering why their settings never take.
        let directory = tempfile::tempdir().unwrap();
        let loaded = load_from(directory.path());
        assert_eq!(Config::default(), loaded.config);
        assert_eq!(1, loaded.warnings.len());
        assert!(
            loaded.warnings[0].contains("cannot read the configuration file"),
            "{:?}",
            loaded.warnings
        );
    }

    #[test]
    fn a_flag_beats_the_environment_which_beats_the_file_which_beats_the_default() {
        let default = || "USD".to_string();
        let flag = || Some("GBP".to_string());
        let environment = || Some("EUR".to_string());
        let file = || Some("NOK".to_string());

        assert_eq!(
            "GBP",
            resolve(flag(), environment(), file(), default()),
            "a flag wins over everything"
        );
        assert_eq!(
            "EUR",
            resolve(None, environment(), file(), default()),
            "the environment wins when no flag was given"
        );
        assert_eq!(
            "NOK",
            resolve(None, None, file(), default()),
            "the file wins when neither flag nor environment said anything"
        );
        assert_eq!(
            "USD",
            resolve(None, None, None, default()),
            "the built-in default is the last resort"
        );
    }

    #[test]
    fn precedence_holds_for_settings_that_are_not_strings() {
        assert_eq!(30, resolve(Some(30), None, Some(60), 360));
        assert_eq!(60, resolve(None, None, Some(60), 360));
        assert_eq!(360, resolve(None, None, None, 360));
        // A `false` in the file is a choice, not an absence: only `None` falls through.
        assert!(!resolve(None, None, Some(false), true));
    }

    #[test]
    fn the_configured_file_is_the_one_that_gets_read() {
        let directory = tempfile::tempdir().unwrap();
        let chosen = directory.path().join("chosen.toml");
        fs::write(&chosen, "currency = \"NOK\"\n").unwrap();
        let ignored = directory.path().join("xdg");
        fs::create_dir_all(ignored.join("gearprice")).unwrap();
        fs::write(
            ignored.join("gearprice/config.toml"),
            "currency = \"SEK\"\n",
        )
        .unwrap();

        let environment = vec![
            ("GEARPRICE_CONFIG", chosen.to_str().unwrap()),
            ("XDG_CONFIG_HOME", ignored.to_str().unwrap()),
        ];
        assert_eq!(
            Some("NOK".to_string()),
            load_in(&environment).config.currency
        );

        let xdg = vec![("XDG_CONFIG_HOME", ignored.to_str().unwrap())];
        assert_eq!(Some("SEK".to_string()), load_in(&xdg).config.currency);
    }

    #[test]
    fn a_line_number_survives_a_multibyte_character_above_it() {
        // The offset of the `1` on the second line falls after a two-byte `ø`.
        let text = "make = \"Bjørn\"\n1";
        assert_eq!(2, line_of(text, text.find('1').unwrap()));
        assert_eq!(1, line_of("", 99));
    }

    #[test]
    fn the_default_path_is_under_the_config_directory() {
        let environment = vec![("HOME", "/home/test")];
        assert_eq!(
            PathBuf::from("/home/test/.config/gearprice/config.toml"),
            config_path_in(&environment)
        );
    }
}
