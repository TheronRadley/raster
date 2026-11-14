//! Minimal prompt that runs inside an existing terminal host.
//!
//! This is not a terminal emulator or a full shell: non-Raster commands are
//! explicitly delegated to the platform command interpreter in a child process.

use std::env;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::Command;

const HELP: &str = "Raster commands:\n\
  help         Show this list\n\
  cd [path]    Show or change the current directory\n\
  edit [file]  Open the Raster editor\n\
  exit         Close Raster\n\
\nOther commands run through the system command interpreter.\n\
Directory changes made by cd persist; other shell state does not.\n";

pub fn run() -> io::Result<()> {
    let stdin = io::stdin();
    let interactive = stdin.is_terminal() && io::stdout().is_terminal();
    let mut line = String::new();
    loop {
        if interactive {
            let path = env::current_dir()?;
            let prompt = crate::util::sanitize_display(&path.to_string_lossy());
            print!("{prompt}> ");
            io::stdout().flush()?;
        }
        line.clear();
        // Lock only for this read, so interactive child programs can use stdin.
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }
        let input = line.trim();
        if input.is_empty() {
            continue;
        }
        let (verb, argument) = input
            .split_once(char::is_whitespace)
            .map_or((input, ""), |(v, rest)| (v, rest.trim()));
        if verb.eq_ignore_ascii_case("help") && argument.is_empty() {
            print!("{HELP}");
        } else if verb.eq_ignore_ascii_case("exit") && argument.is_empty() {
            break;
        } else if verb.eq_ignore_ascii_case("cd") {
            match change_dir(argument) {
                Ok(Some(path)) => println!("{}", crate::util::sanitize_display(&path)),
                Ok(None) => {}
                Err(error) => eprintln!("raster: cd: {error}"),
            }
        } else if verb.eq_ignore_ascii_case("edit") {
            match edit(argument, interactive) {
                Ok(()) => {}
                Err(error) => eprintln!("raster: edit: {error}"),
            }
        } else if let Err(error) = run_external(input) {
            eprintln!("raster: unable to start command interpreter: {error}");
        }
    }
    Ok(())
}

// The entire argument is one literal path, not shell syntax. In particular,
// metacharacters in a path are never evaluated when opening a file.
fn path_argument(argument: &str) -> io::Result<&str> {
    let argument = argument.trim();
    if let Some(inner) = argument.strip_prefix('"') {
        let Some(inner) = inner.strip_suffix('"') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "missing closing quote",
            ));
        };
        if inner.contains('"') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unexpected quote in path",
            ));
        }
        Ok(inner)
    } else if argument.contains('"') {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unexpected quote in path",
        ))
    } else {
        Ok(argument)
    }
}

fn change_dir(argument: &str) -> io::Result<Option<String>> {
    #[cfg(windows)]
    let argument = if argument
        .as_bytes()
        .get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"/d "))
    {
        argument[3..].trim_start()
    } else {
        argument
    };
    let path = path_argument(argument)?;
    if path.is_empty() {
        return Ok(Some(env::current_dir()?.to_string_lossy().into_owned()));
    }
    env::set_current_dir(Path::new(path))?;
    Ok(None)
}

fn edit(argument: &str, interactive: bool) -> io::Result<()> {
    if !interactive {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "editor requires an interactive terminal",
        ));
    }
    let path = path_argument(argument)?;
    let mut command = Command::new(env::current_exe()?);
    if !path.is_empty() {
        command.arg("--").arg(path);
    }
    let status = command.status()?;
    if !status.success() {
        return Err(io::Error::other(format!("editor exited with {status}")));
    }
    Ok(())
}

fn run_external(input: &str) -> io::Result<()> {
    #[cfg(windows)]
    let mut command = {
        let mut cmd = Command::new("cmd.exe");
        cmd.args(["/D", "/C", input]);
        cmd
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", input]);
        cmd
    };
    let _ = command.status()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn literal_path_arguments_preserve_spaces_and_metacharacters() {
        assert_eq!(
            path_argument("\"my file & name.txt\"").unwrap(),
            "my file & name.txt"
        );
        assert_eq!(path_argument("my file.txt").unwrap(), "my file.txt");
        assert!(path_argument("\"unfinished").is_err());
        assert!(path_argument("x\"y").is_err());
    }
}
