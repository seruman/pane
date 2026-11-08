//! mmenu's command line: the same flags, defaults and ranges, parsed by clap.

use std::io::IsTerminal;

use clap::{Parser, ValueEnum};
use rmenu_core::config::{InputFormat, MenuConfiguration};
use rmenu_core::model::Options;

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Input {
    Lines,
    Jsonl,
    Nul,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, ValueEnum)]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Theme {
    Rex,
    Native,
}

#[derive(Debug, Parser)]
#[command(
    name = "rmenu",
    version,
    about = "A programmable menu drawn inside the terminal.",
    long_about = "A programmable menu drawn inside the terminal.\n\nRead lines, JSONL records, or NUL-delimited values from stdin. Configured commands use argv; candidate values are never shell-interpolated."
)]
pub struct Cli {
    /// JSON configuration for sources, bindings and preview.
    #[arg(long)]
    pub config: Option<String>,
    /// Input format.
    #[arg(long, value_enum, default_value = "lines")]
    pub input: Input,
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
    /// Persistent input prompt; use an empty string to hide it.
    #[arg(long, allow_hyphen_values = true)]
    pub prompt: Option<String>,
    /// Initial query.
    #[arg(long, allow_hyphen_values = true, default_value = "")]
    pub query: String,
    /// mmenu's panel width. The terminal decides rmenu's size, so it's accepted and ignored,
    /// letting mmenu's callers run rmenu unchanged.
    #[arg(long, hide = true)]
    pub width: Option<String>,
    /// Visible rows.
    #[arg(long, default_value_t = 12, value_parser = clap::value_parser!(u8).range(4..=30))]
    pub rows: u8,
    /// Light or dark; system follows the terminal.
    #[arg(long, value_enum, default_value = "system")]
    pub appearance: Appearance,
    /// Accepted for mmenu compatibility; rmenu always draws with the terminal's colours.
    #[arg(long, value_enum)]
    pub theme: Option<Theme>,
    /// Trusted preview shell command. Selection is in MMENU_VALUE, not interpolated into code.
    #[arg(long)]
    pub preview: Option<String>,
}

/// What the menu starts with.
#[derive(Debug)]
pub struct Launch {
    pub options: Options,
    pub appearance: Appearance,
}

impl Cli {
    /// Reads the configuration and checks what clap can't. `stdin_is_tty` is passed in so tests
    /// don't depend on how they are run.
    pub fn launch(self, stdin_is_tty: bool) -> Result<Launch, String> {
        let mut configuration = match &self.config {
            Some(path) => {
                let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
                serde_json::from_slice::<MenuConfiguration>(&bytes).map_err(|e| format!("{path}: {e}"))?
            }
            None => MenuConfiguration::default(),
        };
        if let Some(preview) = self.preview {
            configuration.preview = Some(vec!["/bin/sh".into(), "-c".into(), preview]);
        }
        configuration.validate().map_err(|e| e.0)?;
        if configuration.root_source().is_none() && stdin_is_tty {
            return Err("Pipe input into rmenu, or supply --config with command sources.".into());
        }
        let multi = self.multi || configuration.multi;
        let prompt = self.prompt.or_else(|| configuration.prompt.clone()).unwrap_or_else(|| ">".into());
        let input = match self.input {
            Input::Lines => InputFormat::Lines,
            Input::Jsonl => InputFormat::Jsonl,
            Input::Nul => InputFormat::Nul,
        };
        Ok(Launch {
            options: Options {
                configuration,
                input,
                multi,
                json: self.json,
                print0: self.print0,
                sorted: !self.no_sort,
                prompt,
                query: self.query,
                rows: usize::from(self.rows),
                list_cap: crate::view::LIST_CAP,
            },
            appearance: self.appearance,
        })
    }
}

/// mmenu configs load unchanged, but their cmd chords are dead keys here.
fn warnings(configuration: &MenuConfiguration) -> Vec<String> {
    configuration.command_chords()
        .map(|raw| format!("{raw} can't fire because a terminal never sends Command. Bind it with ctrl+ instead."))
        .collect()
}

/// Parses the process's argv; prints help, version and usage errors and exits as clap does.
/// Prints a warning on stderr for each cmd chord.
pub fn parse() -> Result<Launch, String> {
    let launch = Cli::parse().launch(std::io::stdin().is_terminal())?;
    for warning in warnings(&launch.options.configuration) { eprintln!("rmenu: {warning}"); }
    Ok(launch)
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
            let cli = parse(&["--config", "/tmp/c.json", "--json", "--width", "720", "--query", text, "--prompt", text]).unwrap();
            assert_eq!(cli.query, text);
            assert_eq!(cli.prompt.as_deref(), Some(text));
            assert!(cli.json);
        }
    }

    #[test]
    fn equals_form_is_accepted() {
        let cli = parse(&["--rows=4", "--query=-x", "--input=nul"]).unwrap();
        assert_eq!((cli.rows, cli.query.as_str(), cli.input), (4, "-x", Input::Nul));
    }

    #[test]
    fn informational_flags_exit_cleanly() {
        for flag in ["--help", "-h", "--version"] {
            let error = parse(&[flag]).unwrap_err();
            assert_eq!(error.exit_code(), 0, "{flag}");
            assert!(!error.to_string().is_empty());
        }
    }

    #[test]
    fn out_of_range_and_conflicting_flags_are_usage_errors() {
        for argv in [&["--rows", "3"][..], &["--rows", "31"],
                     &["--json", "--print0"], &["--appearance", "sepia"], &["--theme", "warm"], &["--input", "xml"]] {
            let error = parse(argv).unwrap_err();
            assert_eq!(error.exit_code(), 2, "{argv:?}");
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
            r#"{"bindings":[{"key":"cmd+e","action":"accept"},{"key":"ctrl+d","action":"copy"}]}"#).unwrap();
        assert_eq!(warnings(&config), ["cmd+e can't fire because a terminal never sends Command. Bind it with ctrl+ instead."]);
    }

    #[test]
    fn a_tty_stdin_without_sources_is_refused() {
        let error = parse(&[]).unwrap().launch(true).unwrap_err();
        assert_eq!(error, "Pipe input into rmenu, or supply --config with command sources.");
    }

    #[test]
    fn preview_becomes_a_shell_argv_and_config_supplies_defaults() {
        let dir = std::env::temp_dir().join(format!("rmenu-cli-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("c.json");
        std::fs::write(&path, r#"{"prompt":"Pick","multi":true,"sources":[{"id":"a","label":"A","command":["true"]}]}"#).unwrap();
        let launch = parse(&["--config", path.to_str().unwrap(), "--preview", "cat \"$MMENU_VALUE\""]).unwrap().launch(true).unwrap();
        let o = &launch.options;
        assert_eq!(o.configuration.preview.as_deref(), Some(&["/bin/sh".to_owned(), "-c".into(), "cat \"$MMENU_VALUE\"".into()][..]));
        assert_eq!((o.prompt.as_str(), o.multi), ("Pick", true));
        let flagged = parse(&["--config", path.to_str().unwrap(), "--prompt", "Mine"]).unwrap().launch(true).unwrap();
        assert_eq!(flagged.options.prompt, "Mine");
        std::fs::write(&path, r#"{"theme":"sepia"}"#).unwrap();
        let error = parse(&["--config", path.to_str().unwrap()]).unwrap().launch(false).unwrap_err();
        assert_eq!(error, "Unknown theme: sepia");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
