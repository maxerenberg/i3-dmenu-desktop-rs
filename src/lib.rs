use std::collections::HashMap;
use std::env::VarError;
use std::fs;
use std::path::Path;

use lazy_static::lazy_static;
use regex::Regex;

pub mod app_launcher;
pub mod desktop_entry;
mod utils;
mod desktop_entry_cache;

use app_launcher::ChildProcessError;
use desktop_entry::DesktopEntry;
use desktop_entry_cache::{get_cached_desktop_entries, save_desktop_entries_to_cache};
use utils::{join_path, log_warn};

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

    fn get_cache_dir(&self) -> String {
        match (self.get_env)("XDG_CACHE_HOME") {
            Ok(val) => val,
            Err(_) => join_path(&self.home, ".cache"),
        }
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
                log_warn(&format!("Could not parse {}: {}", path_str, err));
                return None;
            },
        };
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
        let mut apps_by_name = HashMap::new();
        let cache_dir = self.get_cache_dir();
        let mut cached_apps_by_path = get_cached_desktop_entries(&cache_dir);
        let mut at_least_one_app_not_in_cache = false;
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
                if !path.is_file() || !path_str.ends_with(".desktop") {
                    continue;
                }
                let mtime = match path.metadata() {
                    Ok(metadata) => match metadata.modified() {
                        Ok(mtime) => mtime,
                        Err(_) => continue,
                    },
                    Err(_) => continue,
                };
                let mut app_opt: Option<DesktopEntry> = None;
                if let Some(app) = cached_apps_by_path.remove(path_str) {
                    if app.mtime == mtime {
                        app_opt = Some(app);
                    }
                }
                if app_opt.is_none() {
                    if let Some(app) = Self::get_desktop_entry_from_file(&path, &locale_keys, &env_paths) {
                        app_opt = Some(app);
                        at_least_one_app_not_in_cache = true;
                    }
                }
                if let Some(app) = app_opt {
                    let name = Self::get_unique_name_for_desktop_entry(&app, &apps_by_name);
                    apps_by_name.insert(name, app);
                }
            }
        }
        if at_least_one_app_not_in_cache {
            save_desktop_entries_to_cache(&cache_dir, apps_by_name.values());
        }
        // Only keep apps which do not have Hidden or NoDisplay set to true.
        // We still want to cache these entries to avoid reading them again on the next run.
        apps_by_name.retain(|_, app| app.Type == "Application" && !app.Hidden && !app.NoDisplay);
        apps_by_name
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
}
