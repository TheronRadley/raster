//! Command-line parsing and help. No external argument parser is needed for
//! this surface, and keeping it local makes `--help` exact and testable.

use std::env;
use std::path::PathBuf;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const HELP: &str = "\
raster — a predictable terminal text editor

Usage:
  raster [OPTIONS] [FILE[:LINE[:COL]] ...]
  raster --help
  raster --version

Options:
  -h, --help               Show this help and exit.
  -V, --version            Show version and build information and exit.
      --config PATH        Load this TOML configuration file.
      --no-config          Ignore user configuration and use defaults.
      --project DIR        Set the project root for file/project search.
      --line N             Start at line N in the first file.
      --read-only          Open all files in read-only mode.
      --large-file         Force reduced-feature large-file mode.
      --no-session         Do not restore or write session state.
      --recover            Restore unsaved recovery files on startup.
      --clean-recovery     Remove expired recovery data and exit.
      --no-mouse           Disable mouse reporting.
      --no-color           Disable ANSI styling (also respects NO_COLOR).
      --no-lsp             Disable language servers for this process.
      --no-alt-screen      Do not use the terminal alternate screen.
      --bench-startup      Measure engine startup and print one JSON line.
      --health             Validate config/recovery paths and print one JSON line.
      --dump-screen FILE   Write one rendered frame to FILE and exit.
      --replay FILE        Replay one key event per line, then dump the screen.
      --                   Treat remaining arguments as file paths.

Files may also be prefixed with +LINE, e.g. `raster +42 src/main.rs`.
The first launch shows a short welcome buffer with the essential shortcuts.
";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileArg {
    pub path: PathBuf,
    pub line: Option<usize>,
    pub col: Option<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CliOptions {
    pub files: Vec<FileArg>,
    pub config: Option<PathBuf>,
    pub no_config: bool,
    pub project: Option<PathBuf>,
    pub line: Option<usize>,
    pub read_only: bool,
    pub large_file: bool,
    pub no_session: bool,
    pub recover: bool,
    pub clean_recovery: bool,
    pub no_mouse: bool,
    pub no_color: bool,
    pub no_lsp: bool,
    pub no_alt_screen: bool,
    pub bench_startup: bool,
    pub health: bool,
    pub dump_screen: Option<PathBuf>,
    pub replay: Option<PathBuf>,
    pub show_help: bool,
    pub show_version: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliError(pub String);

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for CliError {}

impl CliOptions {
    pub fn from_env() -> Result<Self, CliError> {
        Self::parse(env::args().skip(1))
    }

    pub fn parse<I, S>(args: I) -> Result<Self, CliError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut out = Self::default();
        let mut iter = args.into_iter().map(Into::into).peekable();
        let mut paths_only = false;
        let mut pending_line: Option<usize> = None;
        while let Some(arg) = iter.next() {
            if paths_only {
                out.files.push(parse_file_arg(&arg, pending_line.take())?);
                continue;
            }
            match arg.as_str() {
                "--" => paths_only = true,
                "-h" | "--help" => out.show_help = true,
                "-V" | "--version" => out.show_version = true,
                "--no-config" => out.no_config = true,
                "--read-only" => out.read_only = true,
                "--large-file" => out.large_file = true,
                "--no-session" => out.no_session = true,
                "--recover" => out.recover = true,
                "--clean-recovery" => out.clean_recovery = true,
                "--no-mouse" => out.no_mouse = true,
                "--no-color" => out.no_color = true,
                "--no-lsp" => out.no_lsp = true,
                "--no-alt-screen" => out.no_alt_screen = true,
                "--bench-startup" => out.bench_startup = true,
                "--health" => out.health = true,
                "--config" | "--project" | "--line" | "--dump-screen" | "--replay" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| CliError(format!("missing value for {arg}")))?;
                    match arg.as_str() {
                        "--config" => out.config = Some(PathBuf::from(value)),
                        "--project" => out.project = Some(PathBuf::from(value)),
                        "--line" => out.line = Some(parse_number(&value, "--line")?),
                        "--dump-screen" => out.dump_screen = Some(PathBuf::from(value)),
                        "--replay" => out.replay = Some(PathBuf::from(value)),
                        _ => unreachable!(),
                    }
                }
                _ if arg.starts_with("--config=") => out.config = Some(PathBuf::from(&arg[9..])),
                _ if arg.starts_with("--project=") => out.project = Some(PathBuf::from(&arg[10..])),
                _ if arg.starts_with("--line=") => {
                    out.line = Some(parse_number(&arg[7..], "--line")?)
                }
                _ if arg.starts_with("--dump-screen=") => {
                    out.dump_screen = Some(PathBuf::from(&arg[14..]))
                }
                _ if arg.starts_with("--replay=") => out.replay = Some(PathBuf::from(&arg[9..])),
                _ if arg.starts_with('+') => {
                    pending_line = Some(parse_number(&arg[1..], "+LINE")?);
                }
                _ if arg.starts_with('-') && arg != "-" => {
                    return Err(CliError(format!(
                        "unknown option: {arg}\nTry `raster --help`."
                    )));
                }
                _ => out.files.push(parse_file_arg(&arg, pending_line.take())?),
            }
        }
        if let Some(line) = out.line {
            if let Some(first) = out.files.first_mut() {
                if first.line.is_none() {
                    first.line = Some(line);
                }
            }
        }
        Ok(out)
    }
}

fn parse_number(value: &str, what: &str) -> Result<usize, CliError> {
    value
        .parse::<usize>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| CliError(format!("{what} must be a positive integer, got {value:?}")))
}

fn parse_file_arg(value: &str, pending_line: Option<usize>) -> Result<FileArg, CliError> {
    let (path, line, col) = split_location(value);
    let line = line.or(pending_line);
    Ok(FileArg {
        path: PathBuf::from(path),
        line,
        col,
    })
}

/// Supports `path:line:col` while preserving Windows drive letters.
fn split_location(value: &str) -> (String, Option<usize>, Option<usize>) {
    let bytes = value.as_bytes();
    let mut colon = None;
    for i in (0..bytes.len()).rev() {
        if bytes[i] == b':' {
            colon = Some(i);
            break;
        }
    }
    if let Some(i) = colon {
        let tail = &value[i + 1..];
        if !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit()) {
            let before = &value[..i];
            if let Some(j) = before.rfind(':') {
                let mid = &before[j + 1..];
                if !mid.is_empty() && mid.bytes().all(|b| b.is_ascii_digit()) {
                    if let (Ok(line), Ok(col)) = (mid.parse(), tail.parse()) {
                        return (before[..j].to_string(), Some(line), Some(col));
                    }
                }
            }
            if let Ok(line) = tail.parse() {
                let prefix = &value[..i];
                let is_drive = prefix.len() == 1 && prefix.as_bytes()[0].is_ascii_alphabetic();
                if !is_drive {
                    return (prefix.to_string(), Some(line), None);
                }
            }
        }
    }
    (value.to_string(), None, None)
}

pub fn version_line() -> String {
    format!(
        "raster {} (Rust {}, {})",
        VERSION,
        env!("CARGO_PKG_RUST_VERSION"),
        std::env::consts::OS
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_location_and_options() {
        let opts = CliOptions::parse([
            "--project",
            ".",
            "--no-mouse",
            "+12",
            "src/main.rs",
            "notes.txt:7:3",
            "--",
            "-literal",
        ])
        .unwrap();
        assert!(opts.no_mouse);
        assert_eq!(opts.files[0].path, PathBuf::from("src/main.rs"));
        assert_eq!(opts.files[0].line, Some(12));
        assert_eq!(opts.files[1].line, Some(7));
        assert_eq!(opts.files[1].col, Some(3));
        assert_eq!(opts.files[2].path, PathBuf::from("-literal"));
    }

    #[test]
    fn windows_drive_is_not_a_location() {
        let (path, line, _) = split_location("C:\\src\\main.rs");
        assert_eq!(path, "C:\\src\\main.rs");
        assert_eq!(line, None);
    }

    #[test]
    fn unknown_option_is_an_error() {
        assert!(CliOptions::parse(["--definitely-not-real"]).is_err());
    }
}
