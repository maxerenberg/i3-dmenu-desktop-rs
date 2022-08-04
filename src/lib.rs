use std::collections::HashMap;
use std::env::VarError;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufRead};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use lazy_static::lazy_static;
use regex::Regex;

pub mod app_launcher;
use app_launcher::ChildProcessError;

fn is_executable(path: &str) -> bool {
    fs::metadata(path).map_or(false, |m| m.permissions().mode() & 0o111 == 0o111)
}

fn join_path(s1: &str, s2: &str) -> String {
    if s1.ends_with('/') {
        format!("{}{}", s1, s2)
    } else {
        format!("{}/{}", s1, s2)
    }
}

fn get_locale_keys(lc_messages: &str) -> Vec<String> {
    // Ignore the encoding (e.g. .UTF-8)
    lazy_static! {
        static ref ENCODING: Regex = Regex::new(r"\.[^@]+").unwrap();
        static ref COUNTRY_AND_MODIFIER: Regex = Regex::new(r"_[^@]+@").unwrap();
        static ref MODIFIER: Regex = Regex::new(r"@.*").unwrap();
        static ref COUNTRY: Regex = Regex::new(r"_[^@]+").unwrap();
        static ref COUNTRY_OR_MODIFIER: Regex = Regex::new(r"[_@].*").unwrap();
    }
    let lc_messages = &*ENCODING.replace(lc_messages, "");
    // From https://specifications.freedesktop.org/desktop-entry-spec/latest/ar01s05.html
    //
    // LC_MESSAGES value     | Possible keys in order of matching
    // ----------------------|------------------------------------------------------------------------
    // lang_COUNTRY@MODIFIER | lang_COUNTRY@MODIFIER, lang_COUNTRY, lang@MODIFIER, lang, default value
    // lang_COUNTRY          | lang_COUNTRY, lang, default value
    // lang@MODIFIER         | lang@MODIFIER, lang, default value
    // lang                  | lang, default value
    let mut suffixes = vec![lc_messages.to_string()];
    if COUNTRY_AND_MODIFIER.is_match(lc_messages) {
        let no_modifier = &*MODIFIER.replace(lc_messages, "");
        suffixes.push(no_modifier.to_string());
        let no_country = &*COUNTRY.replace(lc_messages, "");
        suffixes.push(no_country.to_string());
    }
    let lang = &*COUNTRY_OR_MODIFIER.replace(lc_messages, "");
    if lang != lc_messages {
        suffixes.push(lang.to_string());
    }
    suffixes
}

pub struct XDGManager<F>
where
    F: Fn(&str) -> Result<String, VarError>
{
    get_env: F,
    home: String,
}

impl<F> XDGManager<F>
where
    F: Fn(&str) -> Result<String, VarError>
{
    pub fn new(get_env: F) -> Self {
        let home = get_env("HOME").expect("HOME environment variable must be set");

        Self { get_env, home }
    }

    fn get_data_dirs(&self) -> Vec<String> {
        let xdg_data_home = match (self.get_env)("XDG_DATA_HOME") {
            Ok(val) => val,
            Err(_) => format!("{}/.local/share", self.home),
        };
        let xdg_data_dirs = match (self.get_env)("XDG_DATA_DIRS") {
            Ok(val) => val,
            Err(_) => String::from("/usr/local/share/:/usr/share/"),
        };
        let mut dirs = vec![xdg_data_home];
        for dir in xdg_data_dirs.split(':') {
            dirs.push(dir.to_string());
        }
        dirs
    }

    fn get_env_paths(&self) -> Vec<String> {
        match (self.get_env)("PATH") {
            Ok(val) => val.split(':').map(|s| s.to_string()).collect(),
            Err(_) => Vec::new(),
        }
    }

    fn get_lc_messages(&self) -> String {
        // See man:locale(7)
        for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(val) = (self.get_env)(key) {
                return val;
            }
        }
        "C".to_string()
    }

    fn get_desktop_entry_from_file(
        path: &Path,
        locale_keys: &[String],
        env_paths: &[String],
    )  -> Option<DesktopEntry> {
        let path_str = path.to_str().unwrap();
        let mut app = match DesktopEntry::parse(path_str, locale_keys) {
            Ok(app) => app,
            Err(err) => {
                Self::warn(&format!("Could not parse {}: {}", path_str, err));
                return None;
            },
        };
        if app.Type != "Application" {
            return None;
        }
        if app.Hidden || app.NoDisplay {
            return None;
        }
        app.escape_chars_for_exec_keys();
        app.remove_invalid_tryexec(env_paths);
        Some(app)
    }

    fn get_unique_name_for_desktop_entry(
        app: &DesktopEntry,
        existing_apps: &HashMap<String, DesktopEntry>,
    ) -> String {
        let mut name = app.Name.clone();
        let mut counter = 1;
        while existing_apps.contains_key(&name) {
            counter += 1;
            name = format!("{} ({})", &app.Name, counter);
        }
        name
    }

    fn get_app_map(&self) -> HashMap<String, DesktopEntry> {
        let mut apps = HashMap::new();
        let data_dirs = self.get_data_dirs();
        let env_paths = self.get_env_paths();
        let locale_keys = get_locale_keys(&self.get_lc_messages());
        for data_dir in &data_dirs {
            let app_dir = join_path(data_dir, "applications");
            let entries = match fs::read_dir(app_dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries {
                let entry = match entry {
                    Ok(entry) => entry,
                    Err(_) => continue,
                };
                let path = entry.path();
                let path_str = path.to_str().unwrap();
                if path.is_file() && path_str.ends_with(".desktop") {
                    if let Some(app) = Self::get_desktop_entry_from_file(&path, &locale_keys, &env_paths) {
                        let name = Self::get_unique_name_for_desktop_entry(&app, &apps);
                        apps.insert(name, app);
                    }
                }
            }
        }
        apps
    }

    pub fn start_app_launcher(&self) -> Result<(), ChildProcessError> {
        let app_map = self.get_app_map();
        let mut app_names: Vec<_> = app_map.keys().collect();
        app_names.sort();
        let choice = match app_launcher::get_dmenu_choice(&app_names) {
            Ok(choice) => choice,
            Err(err) => return Err(err),
        };
        // The user selected one of the dmenu options.
        if let Some(app) = app_map.get(&choice) {
            return app_launcher::launch_desktop_entry(app, &[]).map_err(Into::into);
        }
        // The user selected one of the dmenu options with one or more extra
        // arguments.
        if let Some((left, right)) = choice.rsplit_once(' ') {
            if let Some(app) = app_map.get(left) {
                return app_launcher::launch_desktop_entry(app, &[right]).map_err(Into::into);
            }
        }
        // The user typed arbitrary input.
        app_launcher::launch_i3_cmd_without_desktop_entry(&choice).map_err(Into::into)
    }

    fn warn(msg: &dyn std::fmt::Debug) {
        eprintln!("WARN: {:?}", msg);
    }
}

#[derive(Debug)]
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
    location: String,
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
    fn test_get_data_dirs_default() {
        let home = "/home/max";
        let mgr = XDGManager::new(
            |s| match s {
                "HOME" => Ok(home.to_string()),
                _ => Err(VarError::NotPresent),
            }
        );
        assert_eq!(
            mgr.get_data_dirs(),
            vec![
                format!("{home}/.local/share"),
                "/usr/local/share/".to_string(),
                "/usr/share/".to_string(),
            ]
        );
    }

    #[test]
    fn test_get_data_dirs_custom() {
        let home = "/home/max";
        let mgr = XDGManager::new(
            |s| match s {
                "HOME" => Ok(home.to_string()),
                "XDG_DATA_HOME" => Ok(format!("{home}/data")),
                "XDG_DATA_DIRS" => Ok("/var/lib/flatpak/exports/share:/usr/local/share/:/usr/share/".to_string()),
                _ => Err(VarError::NotPresent),
            }
        );
        assert_eq!(
            mgr.get_data_dirs(),
            vec![
                format!("{home}/data"),
                "/var/lib/flatpak/exports/share".to_string(),
                "/usr/local/share/".to_string(),
                "/usr/share/".to_string(),
            ]
        );
    }

    #[test]
    fn test_locale_keys() {
        let test_cases = vec![
            ("en_CA.UTF8", vec!["en_CA".to_string(), "en".to_string()]),
            ("en_CA", vec!["en_CA".to_string(), "en".to_string()]),
            ("en_CA@Latn", vec!["en_CA@Latn".to_string(), "en_CA".to_string(), "en@Latn".to_string(), "en".to_string()]),
            ("en.UTF8", vec!["en".to_string()]),
            ("en", vec!["en".to_string()]),
        ];
        for (lc_messages, locale_keys) in test_cases {
            assert_eq!(get_locale_keys(lc_messages), locale_keys);
        }
    }

    #[test]
    fn test_general_escape_rule() {
        assert_eq!(DesktopEntry::escape_chars(r"a\nb"), "a\nb");
        assert_eq!(DesktopEntry::escape_chars(r"a\\nb"), "a\\nb");
    }
}
