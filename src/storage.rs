use std::env;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const COMPANY_DIR: &str = "NebulaAudio";
const PLUGIN_DIR: &str = "NebulaStereoDelay";
const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct StoredEditorSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct PersistentStore {
    #[serde(default)]
    editor_size: Option<StoredEditorSize>,
}

impl PersistentStore {
    pub fn load() -> Self {
        let Some(path) = settings_path() else {
            return Self::default();
        };

        fs::read_to_string(path)
            .ok()
            .and_then(|contents| serde_json::from_str(&contents).ok())
            .unwrap_or_default()
    }

    pub fn editor_size(&self) -> Option<(f32, f32)> {
        self.editor_size
            .filter(|size| size.width.is_finite() && size.height.is_finite())
            .filter(|size| size.width > 0.0 && size.height > 0.0)
            .map(|size| (size.width, size.height))
    }

    pub fn save_editor_size(width: f32, height: f32) -> io::Result<()> {
        if !width.is_finite() || !height.is_finite() || width <= 0.0 || height <= 0.0 {
            return Ok(());
        }

        let mut store = Self::load();
        store.editor_size = Some(StoredEditorSize { width, height });
        store.save()
    }

    fn save(&self) -> io::Result<()> {
        let Some(path) = settings_path() else {
            return Ok(());
        };

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let contents = serde_json::to_string_pretty(self)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        fs::write(path, contents)
    }
}

fn settings_path() -> Option<PathBuf> {
    data_dir().map(|base| base.join(COMPANY_DIR).join(PLUGIN_DIR).join(SETTINGS_FILE))
}

#[cfg(target_os = "windows")]
fn data_dir() -> Option<PathBuf> {
    env::var_os("APPDATA").map(PathBuf::from).or_else(|| {
        env::var_os("USERPROFILE").map(|home| PathBuf::from(home).join("AppData").join("Roaming"))
    })
}

#[cfg(target_os = "macos")]
fn data_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join("Library")
            .join("Application Support")
    })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn data_dir() -> Option<PathBuf> {
    env::var_os("XDG_DATA_HOME")
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME").map(|home| PathBuf::from(home).join(".local").join("share"))
        })
}

#[cfg(not(any(target_os = "windows", target_os = "macos", unix)))]
fn data_dir() -> Option<PathBuf> {
    None
}
