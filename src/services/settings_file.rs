//! Export and import of Celeste's setup as a JSON file: remotes with their interval, folders and exclusions, and the preferences. Sign-ins stay out: imported remotes wait for the user to sign in again, and the database's record of synced files starts afresh (the first pass compares both sides).

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::{
    domain::{
        ports::{BackendClient, Repository},
        remote::{Backend, Interval, SyncPolicy},
    },
    infrastructure::{client_router::ClientRouter, proton::client::DisabledProtonClient},
    services::{appearance::Appearance, autostart, power},
    util,
};

/// What the file starts with; an import refuses anything else.
const FORMAT: &str = "celeste-settings";
const VERSION: u32 = 1;

/// rclone parameters worth keeping per backend type: where the remote is, never how to get in.
const KEPT_PARAMETERS: &[(&str, &[&str])] = &[("webdav", &["url", "vendor", "user"])];

/// The rclone type the file uses for Celeste's own Proton client.
const NATIVE_PROTON: &str = "native-proton";

#[derive(Serialize, Deserialize)]
struct File {
    format: String,
    version: u32,
    remotes: Vec<RemoteEntry>,
    preferences: Preferences,
}

#[derive(Serialize, Deserialize)]
struct RemoteEntry {
    name: String,
    /// rclone backend type, or [`NATIVE_PROTON`].
    r#type: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    parameters: Map<String, Value>,
    interval_seconds: u64,
    enabled: bool,
    folders: Vec<FolderEntry>,
}

#[derive(Serialize, Deserialize)]
struct FolderEntry {
    local_path: String,
    remote_path: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    exclusions: Vec<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct Preferences {
    /// `appearance.conf` as an object.
    #[serde(default)]
    appearance: Map<String, Value>,
    #[serde(default)]
    power_mode: Option<String>,
    #[serde(default)]
    run_in_background: Option<bool>,
    #[serde(default)]
    autostart: Option<bool>,
}

/// What an import did, for the user.
#[derive(Debug, Clone, Default)]
pub struct Imported {
    pub remotes: usize,
    pub folders: usize,
    /// Remotes left out because one of the same name exists.
    pub skipped: Vec<String>,
}

/// The current setup as the file's text.
pub fn export(repo: &dyn Repository, client: &dyn BackendClient) -> Result<String, String> {
    let remotes = util::await_future(repo.list_remotes()).map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    for remote in remotes {
        let (r#type, parameters) = if remote.backend == Backend::NativeProton {
            (NATIVE_PROTON.to_owned(), Map::new())
        } else {
            let config = client.config(&remote.name).unwrap_or_default();
            let r#type = config.get("type").and_then(Value::as_str).unwrap_or_default().to_owned();
            let kept = KEPT_PARAMETERS.iter().find(|(t, _)| *t == r#type).map_or(&[][..], |(_, keys)| *keys);
            let parameters = config.into_iter().filter(|(key, _)| kept.contains(&key.as_str())).collect();
            (r#type, parameters)
        };
        let mut folders = Vec::new();
        for dir in util::await_future(repo.list_sync_dirs(remote.id)).map_err(|e| e.to_string())? {
            let exclusions = util::await_future(repo.list_exclusions(dir.id)).map_err(|e| e.to_string())?.into_iter().map(|e| e.remote_path).collect();
            folders.push(FolderEntry { local_path: dir.local_path, remote_path: dir.remote_path, exclusions });
        }
        entries.push(RemoteEntry {
            name: remote.name,
            r#type,
            parameters,
            interval_seconds: remote.policy.interval.seconds(),
            enabled: remote.policy.enabled,
            folders,
        });
    }

    let data_dir = util::get_data_dir();
    let power_mode = conf_to_map(&power::PowerSettings::load(&data_dir).to_conf()).get("mode").and_then(Value::as_str).map(str::to_owned);
    let android = cfg!(target_os = "android");
    let preferences = Preferences {
        appearance: conf_to_map(&Appearance::load(&data_dir).to_conf()),
        power_mode: power_mode.filter(|_| android),
        run_in_background: android.then(power::background_enabled),
        autostart: Some(autostart::enabled()),
    };
    let file = File { format: FORMAT.to_owned(), version: VERSION, remotes: entries, preferences };
    serde_json::to_string_pretty(&file).map_err(|e| e.to_string())
}

/// Adds the file's remotes, signed out, and takes over its preferences. Remotes whose name is taken are left out.
pub fn import(text: &str, repo: &dyn Repository, router: &ClientRouter) -> Result<Imported, String> {
    let file: File = serde_json::from_str(text).map_err(|e| format!("Not a Celeste settings file: {e}"))?;
    if file.format != FORMAT {
        return Err("Not a Celeste settings file.".to_owned());
    }
    if file.version > VERSION {
        return Err("The settings file is from a newer Celeste. Update Celeste first.".to_owned());
    }

    let existing = util::await_future(repo.list_remotes()).map_err(|e| e.to_string())?;
    let mut imported = Imported::default();
    for entry in file.remotes {
        if existing.iter().any(|r| r.name == entry.name) {
            imported.skipped.push(entry.name);
            continue;
        }
        let id = if entry.r#type == NATIVE_PROTON {
            // Without a session in the keyring the remote waits for signing in, also after a restart.
            let id = util::await_future(repo.insert_native_proton_remote(entry.name.clone(), crate::services::auth::KEYRING_SESSION_MARKER.to_owned())).map_err(|e| e.to_string())?;
            router.register(entry.name.clone(), std::sync::Arc::new(DisabledProtonClient::new(format!("'{}' was imported without its sign-in. Sign in again to start syncing.", entry.name))));
            id
        } else {
            // Only the type and where the remote is; without a token or password it waits for signing in (see the rclone client's `needs_reauth`).
            let mut parameters = entry.parameters;
            parameters.insert("config_refresh_token".to_owned(), json!(false));
            let payload = json!({ "name": entry.name, "type": entry.r#type, "parameters": parameters, "opt": { "nonInteractive": true, "obscure": true } });
            router.create_config(payload.to_string())?;
            util::await_future(repo.insert_remote(entry.name.clone())).map_err(|e| e.to_string())?
        };
        let policy = SyncPolicy { interval: Interval::from_seconds(entry.interval_seconds), enabled: entry.enabled };
        util::await_future(repo.set_policy(id, policy)).map_err(|e| e.to_string())?;

        for folder in entry.folders {
            util::await_future(repo.insert_sync_dir(id, folder.local_path.clone(), folder.remote_path.clone())).map_err(|e| e.to_string())?;
            imported.folders += 1;
            if folder.exclusions.is_empty() {
                continue;
            }
            let dirs = util::await_future(repo.list_sync_dirs(id)).map_err(|e| e.to_string())?;
            let Some(dir) = dirs.iter().find(|d| d.local_path == folder.local_path && d.remote_path == folder.remote_path) else { continue };
            for exclusion in folder.exclusions {
                util::await_future(repo.insert_exclusion(dir.id, exclusion)).map_err(|e| e.to_string())?;
            }
        }
        imported.remotes += 1;
    }

    apply_preferences(&file.preferences).map_err(|e| format!("Remotes imported, but the preferences could not be saved: {e}"))?;
    Ok(imported)
}

/// Saves what the file sets; what it leaves out stays as it is.
fn apply_preferences(preferences: &Preferences) -> std::io::Result<()> {
    let data_dir = util::get_data_dir();
    if !preferences.appearance.is_empty() {
        Appearance::parse(&map_to_conf(&preferences.appearance)).save(&data_dir)?;
    }
    if let Some(mode) = &preferences.power_mode {
        // Whether Celeste asked about battery optimization belongs to this device.
        let current = power::PowerSettings::load(&data_dir);
        let mode = power::PowerSettings::parse(&format!("mode={mode}\n")).mode;
        power::PowerSettings { mode, ..current }.save(&data_dir)?;
        power::apply();
    }
    if cfg!(target_os = "android")
        && let Some(on) = preferences.run_in_background
    {
        power::set_background(on)?;
    }
    if let Some(on) = preferences.autostart {
        autostart::set(on)?;
    }
    Ok(())
}

/// `key=value` lines as a JSON object.
fn conf_to_map(conf: &str) -> Map<String, Value> {
    conf.lines().filter_map(|line| line.split_once('=')).map(|(k, v)| (k.trim().to_owned(), json!(v.trim()))).collect()
}

fn map_to_conf(map: &Map<String, Value>) -> String {
    map.iter().filter_map(|(k, v)| Some(format!("{k}={}\n", v.as_str()?))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conf_lines_round_trip_through_json() {
        let conf = "window=dark\ntray_icon=system\nsize=larger\n";
        assert_eq!(conf_to_map(&map_to_conf(&conf_to_map(conf))), conf_to_map(conf));
        assert_eq!(Appearance::parse(&map_to_conf(&conf_to_map(conf))), Appearance::parse(conf));
    }

    #[test]
    fn foreign_files_are_refused() {
        let router = ClientRouter::new(std::sync::Arc::new(crate::test_support::FakeBackend::default()));
        let repo = crate::test_support::FakeRepo::default();
        assert!(import("{}", &repo, &router).is_err());
        assert!(import(r#"{"format":"other","version":1,"remotes":[],"preferences":{}}"#, &repo, &router).is_err());
        assert!(import(r#"{"format":"celeste-settings","version":99,"remotes":[],"preferences":{}}"#, &repo, &router).unwrap_err().contains("newer"));
    }
}
