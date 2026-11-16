use clap::{Parser, ValueEnum};
use rmenu_core::config::{ConfigurationError, InputFormat, MenuConfiguration, Theme};
use rmenu_core::model::{Options, OutputFormat};

pub const DEFAULT_PROMPT: &str = ">";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Parser)]
#[command(
    name = "rmenu",
    version,
    about = "A programmable menu drawn inside the terminal.",
    long_about = "A programmable menu drawn inside the terminal.\n\nRead lines, JSONL records, or NUL-delimited values from stdin. Configured commands use argv. Candidate values are never shell-interpolated."
)]
pub struct Cli {
    /// JSON configuration for sources, bindings and preview.
    #[arg(long)]
    pub config: Option<String>,
    /// Input format.
    #[arg(long, value_enum, default_value = "lines")]
    pub input: InputFormat,
    /// Allow multiple marked items (Tab toggles).
    #[arg(long)]
    pub multi: bool,
    /// Return an action context as JSON, rather than selected values.
    #[arg(long, conflicts_with = "print0")]
    pub json: bool,
    /// Separate output values with NUL instead of newline.
    #[arg(long)]
    pub print0: bool,
    /// Preserve input order while filtering.
    #[arg(long)]
    pub no_sort: bool,
    /// Persistent input prompt. An empty string hides it.
    #[arg(long, allow_hyphen_values = true)]
    pub prompt: Option<String>,
    /// Initial query.
    #[arg(long, allow_hyphen_values = true, default_value = "")]
    pub query: String,
    /// Accepted and ignored for mmenu compatibility. The terminal controls the width.
    #[arg(long, hide = true)]
    pub width: Option<String>,
    /// Rows to move with `PageUp` and `PageDown`.
    #[arg(long, default_value_t = 12, value_parser = clap::value_parser!(u8).range(4..=30))]
    pub rows: u8,
    /// Light or dark. System follows the terminal.
    #[arg(long, value_enum, default_value = "system")]
    pub appearance: Appearance,
    #[arg(long, hide = true)]
    pub theme: Option<Theme>,
    /// Trusted preview shell command. Selection is in `MMENU_VALUE`, not interpolated into code.
    #[arg(long)]
    pub preview: Option<String>,
}

#[derive(Debug)]
pub struct Launch {
    pub options: Options,
    pub appearance: Appearance,
}

#[derive(Debug)]
pub enum LaunchError {
    Read { path: String, source: std::io::Error },
    Configuration { path: Option<String>, source: ConfigurationError },
    MissingInput,
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "{path}: {source}"),
            Self::Configuration { path, source } => {
                if let Some(path) = path {
                    write!(f, "{path}: ")?;
                }
                source.fmt(f)
            }
            Self::MissingInput => f.write_str("Pipe input into rmenu, or supply --config with command sources."),
        }
    }
}

impl std::error::Error for LaunchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Configuration { source, .. } => Some(source),
            Self::MissingInput => None,
        }
    }
}

impl Cli {
    pub fn launch(self, stdin_is_tty: bool) -> Result<Launch, LaunchError> {
        let bytes = match &self.config {
            Some(path) => std::fs::read(path).map_err(|source| LaunchError::Read { path: path.clone(), source })?,
            None => b"{}".to_vec(),
        };
        let preview = self.preview.map(|preview| vec!["/bin/sh".into(), "-c".into(), preview]);
        let configuration = MenuConfiguration::parse(&bytes, preview)
            .map_err(|source| LaunchError::Configuration { path: self.config, source })?;
        if configuration.root_source().is_none() && stdin_is_tty {
            return Err(LaunchError::MissingInput);
        }
        let multi = self.multi || configuration.multi();
        let prompt =
            self.prompt.or_else(|| configuration.prompt().map(str::to_owned)).unwrap_or_else(|| DEFAULT_PROMPT.into());
        Ok(Launch {
            options: Options {
                configuration,
                input: self.input,
                multi,
                output: if self.json {
                    OutputFormat::Json
                } else if self.print0 {
                    OutputFormat::Nul
                } else {
                    OutputFormat::Lines
                },
                sorted: !self.no_sort,
                prompt,
                query: self.query,
                rows: usize::from(self.rows),
                list_cap: rmenu_core::model::DEFAULT_LIST_CAP,
            },
            appearance: self.appearance,
        })
    }
}

pub fn warnings(configuration: &MenuConfiguration) -> Vec<String> {
    configuration
        .command_chords()
        .map(|raw| format!("{raw} can't fire because a terminal never sends Command. Bind it with ctrl+ instead."))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(std::iter::once("rmenu").chain(argv.iter().copied()))
    }

    #[test]
    fn free_text_options_accept_any_value() {
        for text in ["-foo", "--not-an-option", "-", "plain words", ""] {
            let cli =
                parse(&["--config", "/tmp/c.json", "--json", "--width", "720", "--query", text, "--prompt", text])
                    .unwrap();
            assert_eq!(cli.query, text);
            assert_eq!(cli.prompt.as_deref(), Some(text));
            assert!(cli.json);
        }
    }

    #[test]
    fn equals_form_is_accepted() {
        let cli = parse(&["--rows=4", "--query=-x", "--input=nul"]).unwrap();
        assert_eq!((cli.rows, cli.query.as_str(), cli.input), (4, "-x", InputFormat::Nul));
    }

    #[test]
    fn informational_flags_exit_cleanly() {
        for (flag, kind, expected) in [
            ("--help", clap::error::ErrorKind::DisplayHelp, "--config"),
            ("-h", clap::error::ErrorKind::DisplayHelp, "--config"),
            ("--version", clap::error::ErrorKind::DisplayVersion, env!("CARGO_PKG_VERSION")),
        ] {
            let error = parse(&[flag]).unwrap_err();
            assert_eq!(error.exit_code(), 0, "{flag}");
            assert_eq!(error.kind(), kind, "{flag}");
            assert!(error.to_string().contains(expected), "{error}");
        }
    }

    #[test]
    fn out_of_range_and_conflicting_flags_are_usage_errors() {
        for argv in [
            &["--rows", "3"][..],
            &["--rows", "31"],
            &["--json", "--print0"],
            &["--appearance", "sepia"],
            &["--theme", "warm"],
            &["--input", "xml"],
        ] {
            let error = parse(argv).unwrap_err();
            assert_eq!(error.exit_code(), 2, "{argv:?}");
        }
    }

    #[test]
    fn theme_is_validated_but_hidden_and_ignored() {
        let help = parse(&["--help"]).unwrap_err().to_string();
        assert!(!help.contains("--theme"));
        let default = parse(&[]).unwrap().launch(false).unwrap();
        for theme in ["rex", "native"] {
            let launch = parse(&["--theme", theme]).unwrap().launch(false).unwrap();
            assert_eq!(format!("{:?}", launch.options), format!("{:?}", default.options));
            assert_eq!(launch.appearance, default.appearance);
        }
    }

    #[test]
    fn output_flags_choose_one_format() {
        for (args, expected) in [
            (&[][..], OutputFormat::Lines),
            (&["--json"][..], OutputFormat::Json),
            (&["--print0"][..], OutputFormat::Nul),
        ] {
            assert_eq!(parse(args).unwrap().launch(false).unwrap().options.output, expected);
        }
    }

    #[test]
    fn defaults_follow_mmenu() {
        let launch = parse(&[]).unwrap().launch(false).unwrap();
        let o = &launch.options;
        assert_eq!((o.prompt.as_str(), o.query.as_str(), o.rows, o.sorted, o.multi), (">", "", 12, true, false));
        assert_eq!((o.input, launch.appearance), (InputFormat::Lines, Appearance::System));
    }

    #[test]
    fn mmenu_width_is_accepted_and_changes_nothing() {
        let with = parse(&["--width", "720"]).unwrap().launch(false).unwrap();
        let without = parse(&[]).unwrap().launch(false).unwrap();
        assert_eq!(format!("{:?}", with.options), format!("{:?}", without.options));
        assert_eq!(with.appearance, without.appearance);
        assert!(parse(&["--width=9999"]).is_ok(), "any value, as mmenu callers may pass one rmenu has no range for");
    }

    #[test]
    fn each_cmd_chord_gets_a_warning_that_names_it() {
        let config: MenuConfiguration = serde_json::from_str(
            r#"{"bindings":[{"key":"cmd+e","action":"accept"},{"key":"ctrl+d","action":"copy"}]}"#,
        )
        .unwrap();
        assert_eq!(
            warnings(&config),
            ["cmd+e can't fire because a terminal never sends Command. Bind it with ctrl+ instead."]
        );
    }

    #[test]
    fn a_tty_stdin_without_sources_is_refused() {
        let error = parse(&[]).unwrap().launch(true).unwrap_err();
        assert!(matches!(error, LaunchError::MissingInput));
        assert!(error.to_string().contains("--config"));
    }

    #[test]
    fn config_read_errors_keep_the_path_and_io_error() {
        let error = parse(&["--config", ""]).unwrap().launch(false).unwrap_err();
        assert!(matches!(error, LaunchError::Read { path, source }
            if path.is_empty() && source.kind() == std::io::ErrorKind::NotFound));
    }

    fn config(contents: &str) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), contents).unwrap();
        file
    }

    #[test]
    fn config_fixture_is_removed_during_unwind() {
        let file = config("{}");
        let path = file.path().to_owned();
        let result = std::panic::catch_unwind(move || {
            let _file = file;
            panic!("injected assertion failure");
        });
        assert!(result.is_err());
        assert!(!path.exists());
    }

    #[test]
    fn preview_becomes_a_shell_argv() {
        let launch = parse(&["--preview", "cat \"$MMENU_VALUE\""]).unwrap().launch(false).unwrap();
        assert_eq!(
            launch.options.configuration.preview(),
            Some(&["/bin/sh".to_owned(), "-c".into(), "cat \"$MMENU_VALUE\"".into()][..])
        );
    }

    #[test]
    fn configuration_supplies_defaults_and_flags_override_them() {
        let file = config(r#"{"prompt":"Pick","multi":true,"sources":[{"id":"a","label":"A","command":["true"]}]}"#);
        let path = file.path().to_str().unwrap();
        let launch = parse(&["--config", path]).unwrap().launch(true).unwrap();
        assert_eq!(launch.options.prompt, "Pick");
        assert!(launch.options.multi);
        let flagged = parse(&["--config", path, "--prompt", "Mine"]).unwrap().launch(true).unwrap();
        assert_eq!(flagged.options.prompt, "Mine");
    }

    #[test]
    fn preview_override_is_validated_instead_of_the_replaced_command() {
        let file = config(r#"{"preview":[]}"#);
        let path = file.path().to_str().unwrap();
        let error = parse(&["--config", path]).unwrap().launch(false).unwrap_err();
        assert!(
            matches!(error, LaunchError::Configuration { path: Some(ref file), source: ConfigurationError::MissingProgram { ref entry } }
            if file == path && entry == "Preview command")
        );
        let overridden = parse(&["--config", path, "--preview", "echo"]).unwrap().launch(false).unwrap();
        assert_eq!(
            overridden.options.configuration.preview(),
            Some(["/bin/sh".into(), "-c".into(), "echo".into()].as_slice())
        );
    }

    #[test]
    fn invalid_configuration_theme_names_the_rejected_value() {
        let file = config(r#"{"theme":"sepia"}"#);
        let error = parse(&["--config", file.path().to_str().unwrap()]).unwrap().launch(false).unwrap_err();
        assert!(matches!(error, LaunchError::Configuration { source: ConfigurationError::Json(_), .. }));
        assert!(error.to_string().contains("sepia"));
    }
}
