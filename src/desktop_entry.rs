use std::fs::{self, File};
use std::fmt;
use std::io::{self, BufRead};
use std::os::unix::fs::PermissionsExt;
use std::time::SystemTime;

use lazy_static::lazy_static;
use regex::Regex;
use serde::{Serialize, Deserialize};

use super::utils::join_path;

fn is_executable(path: &str) -> bool {
    fs::metadata(path).map_or(false, |m| m.permissions().mode() & 0o111 == 0o111)
}

// Adapted from https://doc.rust-lang.org/std/convert/trait.From.html#examples
#[derive(Debug)]
pub enum DesktopEntryError {
    IoError(io::Error),
    ParseError(String),
}

impl From<io::Error> for DesktopEntryError {
    fn from(error: io::Error) -> Self { Self::IoError(error) }
}

impl fmt::Display for DesktopEntryError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::IoError(err) => write!(f, "{err}"),
            Self::ParseError(msg) => write!(f, "{msg}"),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DesktopEntry {
    // See https://specifications.freedesktop.org/desktop-entry-spec/latest/ar01s06.html
    pub Name: String,
    pub Exec: Option<String>,
    pub TryExec: Option<String>,
    pub Path: Option<String>,
    pub Type: String,
    // These keys are optional, but we will provide defaults (see parse function)
    pub NoDisplay: bool,
    pub Hidden: bool,
    pub StartupNotify: bool,
    pub Terminal: bool,
    // This is the path of the desktop entry file (not an actual key)
    pub location: String,
    // This is the mtime of the desktop entry file (not an actual key)
    pub mtime: SystemTime,
}

impl DesktopEntry {
    pub fn parse(filepath: &str, locale_keys: &[String]) -> Result<DesktopEntry, DesktopEntryError> {
        // Parsing logic is adapted from the original i3-dmenu-desktop script
        lazy_static! {
            // The 'x' flag enables insignificant whitespace mode
            static ref KV_PAIR: Regex = Regex::new(r"(?x)^
                (
                    [A-Za-z0-9-]+    # key
                    (?:\[[^]]+\])?   # optional locale suffix
                )
                \s* = \s*            # whitespace around '=' is ignored
                (.*)                 # value
                $").unwrap();
            static ref LOCALIZED_NAME: Regex = Regex::new(r"^Name\[([^]]+)\]$").unwrap();
        }
        let mut Name: Option<String> = None;
        let mut Exec: Option<String> = None;
        let mut TryExec: Option<String> = None;
        let mut Path: Option<String> = None;
        let mut Type: Option<String> = None;
        // use sane defaults for these keys
        let mut NoDisplay = false;
        let mut Hidden = false;
        let mut StartupNotify = true;
        let mut Terminal = false;

        let mut in_desktop_entry_section = false;
        let mut localized_name: Option<String> = None;
        // index into locale_keys (lower index = higher priority)
        let mut localized_name_idx = 0;

        let file = File::open(filepath)?;
        let mtime = file.metadata()?.modified()?;
        for line in io::BufReader::new(file).lines() {
            let line = line?;
            let line = line.trim();
            let first_char = match line.chars().next() {
                Some(ch) => ch,
                None => continue,
            };
            if first_char == '[' {
                in_desktop_entry_section = line == "[Desktop Entry]";
                continue;
            }
            if !in_desktop_entry_section {
                continue;
            }
            if first_char == '#' {
                continue;
            }
            let captures = match KV_PAIR.captures(line) {
                Some(caps) => caps,
                None => continue,
            };
            let key = captures.get(1).unwrap().as_str();
            let value = captures.get(2).unwrap().as_str();
            if let Some(captures) = LOCALIZED_NAME.captures(key) {
                let locale = captures.get(1).unwrap().as_str();
                // locale_keys is sorted from highest to lowest priority
                if let Some(idx) = locale_keys.iter().position(|s| s == locale) {
                    if localized_name.is_none() || idx < localized_name_idx {
                        localized_name = Some(value.to_string());
                        localized_name_idx = idx;
                    }
                }
                continue;
            }
            match key {
                "Name" => Name = Some(value.to_string()),
                "Exec" => Exec = Some(value.to_string()),
                "TryExec" => TryExec = Some(value.to_string()),
                "Path" => Path = Some(value.to_string()),
                "Type" => Type = Some(value.to_string()),
                "NoDisplay" => NoDisplay = value == "true",
                "Hidden" => Hidden = value == "true",
                "StartupNotify" => StartupNotify = value == "true",
                "Terminal" => Terminal = value == "true",
                _ => (),
            }
        }
        // Localized name takes priority over default name
        if localized_name.is_some() {
            Name = localized_name;
        }
        if Type.is_none() {
            Err(DesktopEntryError::ParseError("missing Type key".to_string()))
        } else if Name.is_none() {
            Err(DesktopEntryError::ParseError("missing Name key".to_string()))
        } else if Exec.is_none() && Type.as_ref().unwrap() == "Application" {
            Err(DesktopEntryError::ParseError("missing Exec key".to_string()))
        } else {
            Ok(DesktopEntry{
                Name: Name.unwrap(),
                Exec,
                TryExec,
                Path,
                Type: Type.unwrap(),
                NoDisplay,
                Hidden,
                StartupNotify,
                Terminal,
                location: filepath.to_string(),
                mtime,
            })
        }
    }

    fn escape_chars(cmd: &str) -> String {
        let old_chars: Vec<_> = cmd.chars().collect();
        let mut new_chars = Vec::<char>::new();
        let mut i = 0;
        // First, we apply the general escape rule for strings.
        // See https://specifications.freedesktop.org/desktop-entry-spec/latest/ar01s04.html.
        while i < old_chars.len() {
            let ch = old_chars[i];
            if ch == '\\' && i + 1 < old_chars.len() {
                match old_chars[i+1] {
                    's' => new_chars.push(' '),
                    'n' => new_chars.push('\n'),
                    't' => new_chars.push('\t'),
                    'r' => new_chars.push('\r'),
                    '\\' => new_chars.push('\\'),
                    other => {
                        new_chars.push(ch);
                        new_chars.push(other);
                    },
                }
                i += 2;
            } else {
                new_chars.push(ch);
                i += 1;
            }
        }
        // This says that we need to unescape the double quote, backtick and
        // dollar sign characters:
        // https://specifications.freedesktop.org/desktop-entry-spec/latest/ar01s07.html
        // However, because i3 will pass the arguments to sh -c, we will skip this step
        // because sh will unescape them for us.
        String::from_iter(new_chars)
    }

    pub fn escape_chars_for_exec_keys(&mut self) {
        if let Some(ref cmd) = self.TryExec {
            self.TryExec = Some(Self::escape_chars(cmd));
        }
        if let Some(ref cmd) = self.Exec {
            self.Exec = Some(Self::escape_chars(cmd));
        }
    }

    pub fn replace_field_codes(&self, exec_str: &str, extra_args: &[&str]) -> String {
        lazy_static! {
            static ref FIELD_CODE: Regex = Regex::new("%[fFuUdDnNickvm]").unwrap();
        }
        let first_arg = extra_args.first().copied().unwrap_or("");
        let all_args = &extra_args.join(" ");
        FIELD_CODE.replace_all(exec_str, |caps: &regex::Captures| match &caps[0] {
            "%f" => first_arg,
            "%F" => all_args,
            "%u" => first_arg,
            "%U" => all_args,
            "%i" => "",  // icon - not supported for now
            "%c" => &self.Name,
            "%k" => &self.location,
            "%d" | "%D" | "%n" | "%N" | "%v" | "%m" => "",  // deprecated
            "%%" => "%",
            _ => "",
        }).into_owned()
    }

    fn get_arg0(exec_str: &str) -> String {
        lazy_static! {
            static ref NONQUOTED_ARG0: Regex = Regex::new(r#"^([^"]+)(?:\s|$)"#).unwrap();
            static ref QUOTED_ARG0: Regex = Regex::new(r#"^"([^"]+)"(?:\s|$)"#).unwrap();
        }
        if let Some(captures) = NONQUOTED_ARG0.captures(exec_str) {
            captures.get(1).unwrap().as_str().to_string()
        } else if let Some(captures) = QUOTED_ARG0.captures(exec_str) {
            captures.get(1).unwrap().as_str().to_string()
        } else {
            // invalid quoting - return the whole string
            exec_str.to_string()
        }
    }

    pub fn remove_invalid_tryexec(&mut self, env_paths: &[String]) {
        let try_exec = match self.TryExec {
            Some(ref val) => val,
            None => return,
        };
        let arg0 = Self::get_arg0(try_exec);
        let try_exec_is_valid = if arg0.contains('/') {
            is_executable(&arg0)
        } else {
            env_paths.iter().any(|path| is_executable(&join_path(path, &arg0)))
        };
        if !try_exec_is_valid {
            self.TryExec = None;
        }
    }

    pub fn get_exec_str(&self) -> &str {
        match self.TryExec {
            Some(ref val) => val,
            None => self.Exec.as_ref().unwrap(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_general_escape_rule() {
        assert_eq!(DesktopEntry::escape_chars(r"a\nb"), "a\nb");
        assert_eq!(DesktopEntry::escape_chars(r"a\\nb"), "a\\nb");
    }
}
