use std::collections::HashMap;
use std::fs;

use serde::{Serialize, Deserialize};

use super::DesktopEntry;
use super::utils::{join_path, log_warn};

const CACHE_VERSION: u32 = 1;
const CACHE_FILE_NAME: &str = "i3-dmenu-desktop-rs.bincode";

// There is a more concise way to do this using Cow:
// https://stackoverflow.com/a/52733564
// However, using two structs is easier to understand.

#[derive(Serialize)]
struct VersionedCacheForSerialize<'a> {
    version: u32,
    data: Vec<&'a DesktopEntry>,
}

#[derive(Deserialize)]
struct VersionedCacheForDeserialize {
    version: u32,
    data: Vec<DesktopEntry>,
}

/// Returns a map of absolute file paths to XDG desktop entries.
///
/// # Arguments
///
/// * `cache_dir`: the $XDG_CACHE_HOME directory from which the cache file
///   will be read
pub fn get_cached_desktop_entries(cache_dir: &str) -> HashMap<String, DesktopEntry> {
    let mut apps = HashMap::new();
    let file_path = join_path(cache_dir, CACHE_FILE_NAME);
    let contents = match fs::read(&file_path) {
        Ok(data) => data,
        Err(_) => return apps,
    };
    let cache: VersionedCacheForDeserialize = match bincode::deserialize(&contents) {
        Ok(data) => data,
        Err(_) => {
            log_warn(&format!("could not deserialize {}", &file_path));
            return apps;
        },
    };
    if cache.version != CACHE_VERSION {
        return apps;
    }
    for desktop_entry in cache.data {
        apps.insert(desktop_entry.location.clone(), desktop_entry);
    }
    apps
}

/// Saves the desktop entries to a serialized cache file.
///
/// # Arguments
///
/// * `cache_dir`: the $XDG_CACHE_HOME directory where the file will be saved
pub fn save_desktop_entries_to_cache<'a>(cache_dir: &str, apps: impl Iterator<Item=&'a DesktopEntry>) {
    let cache = VersionedCacheForSerialize {
        version: CACHE_VERSION,
        data: apps.collect(),
    };
    let encoded = bincode::serialize(&cache).unwrap();
    let file_path = join_path(cache_dir, CACHE_FILE_NAME);
    if let Err(err) = fs::write(&file_path, encoded) {
        log_warn(&format!("Could not save desktop entries to {}: {}", &file_path, err));
    }
}
