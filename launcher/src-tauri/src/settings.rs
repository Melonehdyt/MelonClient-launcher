use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
pub struct Settings {
    pub minecraft_dir: String,
    pub java_path: String,
    pub memory_mb: u32,
    pub extra_jvm_args: String,
    pub version: String,
    /// Runtime mod loader: fabric, quilt, forge or neoforge.
    pub mod_loader: String,
    /// Empty string means "use the newest loader available for the version".
    pub loader: String,
    pub azure_client_id: String,
    /// "default" (built-in video), "picture", "video" or "youtube".
    pub background_type: String,
    /// File path for picture/video, or a YouTube link / id.
    pub background_source: String,
    /// "cover", "contain" or "fill".
    pub background_fit: String,
    /// 0-100 percent of black overlay on top of the background.
    pub background_dim: u32,
    /// Blur radius in pixels applied to the background.
    pub background_blur: u32,
    /// 0-80 percent transparency for launcher content blocks.
    pub background_panel_transparency: u32,
    /// 0-100 percent volume for local background videos.
    pub background_volume: u32,
    pub background_mute: bool,
    pub background_loop: bool,
    /// Whether to show the remote launcher news feed on the Play page.
    pub news_enabled: bool,
    /// Hide the launcher to the system tray once Minecraft starts.
    pub hide_after_launch: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            minecraft_dir: default_minecraft_dir(),
            java_path: String::new(),
            memory_mb: 4096,
            extra_jvm_args: String::new(),
            version: "latest-release".to_string(),
            mod_loader: "fabric".to_string(),
            loader: String::new(),
            azure_client_id: String::new(),
            background_type: "default".to_string(),
            background_source: String::new(),
            background_fit: "cover".to_string(),
            background_dim: 35,
            background_blur: 0,
            background_panel_transparency: 0,
            background_volume: 30,
            background_mute: true,
            background_loop: true,
            news_enabled: true,
            hide_after_launch: false,
        }
    }
}

pub fn default_minecraft_dir() -> String {
    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("APPDATA") {
        return PathBuf::from(appdata)
            .join(".minecraft")
            .to_string_lossy()
            .to_string();
    }
    #[cfg(target_os = "macos")]
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("minecraft")
            .to_string_lossy()
            .to_string();
    }
    #[cfg(target_os = "linux")]
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".minecraft")
            .to_string_lossy()
            .to_string();
    }
    ".minecraft".to_string()
}

/// One saved account. `kind` is "msa" or "offline"; `flow` is "live"
/// (browser sign-in against login.live.com), "consumers" (device-code against
/// the Microsoft consumers endpoint) or "offline".
#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct StoredAccount {
    pub id: String,
    pub name: String,
    pub uuid: String,
    pub kind: String,
    pub flow: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offline_name: Option<String>,
}

impl StoredAccount {
    pub fn online(&self) -> bool {
        self.kind == "msa"
    }
}

#[derive(Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct AuthStore {
    pub accounts: Vec<StoredAccount>,
    pub active: Option<String>,
    // Legacy single-account fields, kept only so old auth.json files migrate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub msa_refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offline_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_uuid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_kind: Option<String>,
}

impl AuthStore {
    /// Converts a legacy single-account file into the multi-account shape.
    fn migrate(&mut self) {
        if self.accounts.is_empty() {
            if let (Some(kind), Some(name)) = (self.profile_kind.clone(), self.profile_name.clone())
            {
                let uuid = self.profile_uuid.clone().unwrap_or_default();
                let id = if kind == "msa" {
                    format!("msa:{uuid}")
                } else {
                    format!("offline:{}", name.to_lowercase())
                };
                self.accounts.push(StoredAccount {
                    id: id.clone(),
                    name,
                    uuid,
                    kind: kind.clone(),
                    flow: if kind == "msa" {
                        "live".into()
                    } else {
                        "offline".into()
                    },
                    refresh_token: self.msa_refresh_token.clone(),
                    offline_name: self.offline_name.clone(),
                });
                self.active = Some(id);
            }
        }
        self.msa_refresh_token = None;
        self.offline_name = None;
        self.profile_name = None;
        self.profile_uuid = None;
        self.profile_kind = None;
    }

    pub fn active_account(&self) -> Option<&StoredAccount> {
        let id = self.active.as_ref()?;
        self.accounts.iter().find(|a| &a.id == id)
    }

    pub fn add_account(&mut self, account: StoredAccount) {
        match self.accounts.iter_mut().find(|a| a.id == account.id) {
            Some(existing) => *existing = account.clone(),
            None => self.accounts.push(account.clone()),
        }
        self.active = Some(account.id);
    }

    pub fn remove_account(&mut self, id: &str) {
        self.accounts.retain(|a| a.id != id);
        if self.active.as_deref() == Some(id) {
            self.active = self.accounts.first().map(|a| a.id.clone());
        }
    }
}

fn config_dir(app: &AppHandle) -> Result<PathBuf> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn load_settings(app: &AppHandle) -> Settings {
    let path = match config_dir(app) {
        Ok(dir) => dir.join("settings.json"),
        Err(_) => return Settings::default(),
    };
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => Settings::default(),
    }
}

pub fn save_settings(app: &AppHandle, settings: &Settings) -> Result<()> {
    let path = config_dir(app)?.join("settings.json");
    std::fs::write(path, serde_json::to_string_pretty(settings)?)?;
    Ok(())
}

pub fn load_auth(app: &AppHandle) -> AuthStore {
    let path = match config_dir(app) {
        Ok(dir) => dir.join("auth.json"),
        Err(_) => return AuthStore::default(),
    };
    let mut store: AuthStore = match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
        Err(_) => AuthStore::default(),
    };
    store.migrate();
    store
}

pub fn save_auth(app: &AppHandle, auth: &AuthStore) -> Result<()> {
    let path = config_dir(app)?.join("auth.json");
    std::fs::write(path, serde_json::to_string_pretty(auth)?)?;
    Ok(())
}
