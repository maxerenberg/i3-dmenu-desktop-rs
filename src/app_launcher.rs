use std::error::Error;
use std::fmt;
use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::str::Utf8Error;

use super::DesktopEntry;

/// Returns a transformed string which can be passed to i3's exec command.
///
/// # Arguments
///
/// * `cmd`: the string to be transformed. It should already have undergone
///   the general string escape rules as specified in
///   https://specifications.freedesktop.org/desktop-entry-spec/latest/ar01s04.html.
///
/// # Examples
///
/// ```
/// use i3_dmenu_desktop_rs::app_launcher::escape_for_i3_exec;
///
/// let escaped = escape_for_i3_exec("notify-send hello");
/// assert_eq!(escaped, r#""notify-send hello""#);
///
/// let escaped = escape_for_i3_exec(r#"notify-send "hello, world""#);
/// assert_eq!(escaped, r#""notify-send \"hello, world\"""#);
///
/// let escaped = escape_for_i3_exec(r#"notify-send "abc \"def\" ghi""#);
/// assert_eq!(escaped, r#""notify-send \"abc \\\"def\\\" ghi\"""#);
/// ```
pub fn escape_for_i3_exec(cmd: &str) -> String {
    // See https://i3wm.org/docs/userguide.html#exec_quoting
    let mut new_chars = vec!['"'];
    for ch in cmd.chars() {
        if ch == '"' || ch == '\\' {
            new_chars.push('\\');
        }
        new_chars.push(ch);
    }
    new_chars.push('"');
    String::from_iter(new_chars)
}

#[derive(Debug)]
pub enum ChildProcessError {
    IoError(io::Error),
    BadOutputError(Utf8Error),
    ProcessFailed(String),
}

impl From<io::Error> for ChildProcessError {
    fn from(error: io::Error) -> Self { Self::IoError(error) }
}

impl From<Utf8Error> for ChildProcessError {
    fn from(error: Utf8Error) -> Self { Self::BadOutputError(error) }
}

impl fmt::Display for ChildProcessError {
    fn fmt(&self, f: &mut fmt::Formatter) -> Result<(), fmt::Error> {
        match self {
            Self::IoError(err) => err.fmt(f),
            Self::BadOutputError(err) => err.fmt(f),
            Self::ProcessFailed(ref msg) => write!(f, "{}", msg),
        }
    }
}

impl Error for ChildProcessError {}

pub fn get_dmenu_choice<S: AsRef<str>>(app_names: &[S]) -> Result<String, ChildProcessError> {
    let input = app_names.into_iter().map(AsRef::as_ref).collect::<Vec<_>>().join("\n");
    let mut child = Command::new("dmenu")
        .arg("-i")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let _ = child.stdin.take().unwrap().write_all(input.as_bytes())?;
    let output = child.wait_with_output()?;
    if !output.status.success() {
        return Err(ChildProcessError::ProcessFailed("dmenu process failed".to_string()));
    }
    let output = std::str::from_utf8(&output.stdout)?.trim_end();
    Ok(output.to_string())
}

pub fn launch_i3_cmd_without_desktop_entry(cmd: &str) -> Result<(), io::Error> {
    let i3_cmd = escape_for_i3_exec(cmd);
    Command::new("i3-msg").arg("exec").arg(&i3_cmd).spawn().map(|_| ())
}

fn launch_i3_cmd(desktop_entry_exec_str: &str, app: &DesktopEntry) -> Result<(), io::Error> {
    let i3_cmd = escape_for_i3_exec(desktop_entry_exec_str);
    let cmd = if app.Terminal {
        format!("i3-sensible-terminal -e {}", i3_cmd)
    } else {
        i3_cmd
    };
    let no_startup_notify = if app.StartupNotify { "" } else { "--no-startup-id" };
    let arg = format!("exec {} {}", no_startup_notify, cmd);
    Command::new("i3-msg").arg(arg).spawn().map(|_| ())
}

pub fn launch_desktop_entry(app: &DesktopEntry, extra_args: &[&str]) -> Result<(), io::Error> {
    let cmd = app.replace_field_codes(app.get_exec_str(), extra_args);
    launch_i3_cmd(&cmd, app)
}
