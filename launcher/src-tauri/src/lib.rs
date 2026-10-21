mod auth;
mod game;
mod modrinth;
mod settings;
mod util;

use serde::Serialize;
use serde_json::Value;
use settings::{
    load_auth, load_settings, save_auth, save_settings, AuthStore, Settings, StoredAccount,
};
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

struct Backend {
    client: reqwest::Client,
    game_processes: Arc<tokio::sync::Mutex<HashMap<u32, tokio::process::Child>>>,
    stopped_processes: Arc<tokio::sync::Mutex<HashSet<u32>>>,
    launch_in_progress: Arc<AtomicBool>,
}

struct LaunchReset(Arc<AtomicBool>);

impl Drop for LaunchReset {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn to_str<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

#[derive(Serialize, serde::Deserialize, Clone)]
struct LauncherProfile {
    id: String,
    name: String,
    game_dir: String,
    version: String,
    mod_loader: String,
    loader: String,
    #[serde(default = "default_profile_avatar")]
    avatar: String,
    #[serde(default)]
    avatar_data: Option<String>,
}

fn default_profile_avatar() -> String {
    "grass".to_string()
}

#[derive(Serialize, serde::Deserialize)]
struct ProfilesStore {
    base_dir: String,
    active_profile: String,
    profiles: Vec<LauncherProfile>,
}

#[derive(Serialize)]
struct ProfileSummary {
    id: String,
    name: String,
    version: String,
    mod_loader: String,
    loader: String,
    avatar: String,
    avatar_data: Option<String>,
}

#[derive(Serialize)]
struct ProfilesState {
    active_profile: String,
    profiles: Vec<ProfileSummary>,
}

fn profiles_file(app: &AppHandle) -> Result<PathBuf, String> {
    let directory = app.path().app_config_dir().map_err(to_str)?;
    std::fs::create_dir_all(&directory).map_err(to_str)?;
    Ok(directory.join("profiles.json"))
}

fn save_profiles(app: &AppHandle, store: &ProfilesStore) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(store).map_err(to_str)?;
    std::fs::write(profiles_file(app)?, bytes).map_err(to_str)
}

fn load_profiles(app: &AppHandle, settings: &Settings) -> Result<ProfilesStore, String> {
    let path = profiles_file(app)?;
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(to_str),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let store = ProfilesStore {
                base_dir: settings.minecraft_dir.clone(),
                active_profile: "default".to_string(),
                profiles: vec![LauncherProfile {
                    id: "default".to_string(),
                    name: "Default".to_string(),
                    game_dir: settings.minecraft_dir.clone(),
                    version: settings.version.clone(),
                    mod_loader: settings.mod_loader.clone(),
                    loader: settings.loader.clone(),
                    avatar: default_profile_avatar(),
                    avatar_data: None,
                }],
            };
            save_profiles(app, &store)?;
            Ok(store)
        }
        Err(error) => Err(to_str(error)),
    }
}

fn profiles_state(store: &ProfilesStore) -> ProfilesState {
    ProfilesState {
        active_profile: store.active_profile.clone(),
        profiles: store
            .profiles
            .iter()
            .map(|profile| ProfileSummary {
                id: profile.id.clone(),
                name: profile.name.clone(),
                version: profile.version.clone(),
                mod_loader: profile.mod_loader.clone(),
                loader: profile.loader.clone(),
                avatar: profile.avatar.clone(),
                avatar_data: profile.avatar_data.clone(),
            })
            .collect(),
    }
}

fn profile_folder_slug(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

fn profile_directory(base_dir: &Path, name: &str, id: &str) -> PathBuf {
    let slug = profile_folder_slug(name);
    base_dir.join("profiles").join(format!(
        "{}-{}",
        if slug.is_empty() { "profile" } else { &slug },
        &id[..8]
    ))
}

fn secure_mojang_skin_url(raw_url: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(raw_url)
        .map_err(|_| "Minecraft returned an invalid skin download URL.".to_string())?;
    let valid_texture_path = url
        .path()
        .strip_prefix("/texture/")
        .is_some_and(|hash| !hash.is_empty() && !hash.contains('/'));
    if url.host_str() != Some("textures.minecraft.net")
        || !matches!(url.scheme(), "https" | "http")
        || !valid_texture_path
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
    {
        return Err("Minecraft returned an unexpected skin download URL.".to_string());
    }
    if url.scheme() == "http" {
        url.set_scheme("https")
            .map_err(|_| "Could not securely normalize Minecraft's skin URL.".to_string())?;
    }
    Ok(url)
}

fn skin_model(texture_profile: &Value) -> &'static str {
    match texture_profile["textures"]["SKIN"]["metadata"]["model"].as_str() {
        Some("slim") => "slim",
        _ => "classic",
    }
}

fn minecraft_profile_url(username: &str) -> Result<reqwest::Url, String> {
    let mut url =
        reqwest::Url::parse("https://api.mojang.com/users/profiles/minecraft").map_err(to_str)?;
    url.path_segments_mut()
        .map_err(|_| "Could not construct the Minecraft profile lookup URL.".to_string())?
        .push(username);
    Ok(url)
}

fn active_profile_settings(app: &AppHandle) -> Result<Settings, String> {
    let mut settings = load_settings(app);
    let store = load_profiles(app, &settings)?;
    let profile = store
        .profiles
        .iter()
        .find(|profile| profile.id == store.active_profile)
        .ok_or_else(|| "The active launcher profile was not found.".to_string())?;
    settings.minecraft_dir = profile.game_dir.clone();
    settings.version = profile.version.clone();
    settings.mod_loader = profile.mod_loader.clone();
    settings.loader = profile.loader.clone();
    Ok(settings)
}

#[derive(Serialize, Clone)]
struct AccountSummary {
    id: String,
    name: String,
    uuid: String,
    kind: String,
    online: bool,
}

impl AccountSummary {
    fn from_stored(account: &StoredAccount) -> Self {
        AccountSummary {
            id: account.id.clone(),
            name: account.name.clone(),
            uuid: account.uuid.clone(),
            kind: account.kind.clone(),
            online: account.online(),
        }
    }
}

#[derive(Serialize)]
struct AccountsState {
    active: Option<String>,
    accounts: Vec<AccountSummary>,
}

#[tauri::command]
fn get_settings(app: AppHandle) -> Result<Settings, String> {
    active_profile_settings(&app)
}

#[tauri::command]
fn set_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let mut store = load_profiles(&app, &load_settings(&app))?;
    let active_id = store.active_profile.clone();
    let profile = store
        .profiles
        .iter_mut()
        .find(|profile| profile.id == active_id)
        .ok_or_else(|| "The active launcher profile was not found.".to_string())?;
    profile.game_dir = settings.minecraft_dir.clone();
    profile.version = settings.version.clone();
    profile.mod_loader = settings.mod_loader.clone();
    profile.loader = settings.loader.clone();
    let is_default = profile.id == "default";
    if is_default {
        store.base_dir = settings.minecraft_dir.clone();
    }
    save_settings(&app, &settings).map_err(to_str)?;
    save_profiles(&app, &store)
}

#[tauri::command]
fn reset_settings(app: AppHandle) -> Result<Settings, String> {
    let current = active_profile_settings(&app)?;
    let mut settings = Settings::default();
    settings.minecraft_dir = current.minecraft_dir;
    settings.version = current.version;
    settings.mod_loader = current.mod_loader;
    settings.loader = current.loader;
    save_settings(&app, &settings).map_err(to_str)?;
    Ok(settings)
}

#[tauri::command]
fn list_profiles(app: AppHandle) -> Result<ProfilesState, String> {
    let settings = load_settings(&app);
    let store = load_profiles(&app, &settings)?;
    Ok(profiles_state(&store))
}

#[tauri::command]
fn select_profile(app: AppHandle, id: String) -> Result<ProfilesState, String> {
    let settings = load_settings(&app);
    let mut store = load_profiles(&app, &settings)?;
    if !store.profiles.iter().any(|profile| profile.id == id) {
        return Err("That launcher profile no longer exists.".to_string());
    }
    store.active_profile = id;
    save_profiles(&app, &store)?;
    Ok(profiles_state(&store))
}

#[tauri::command]
fn create_profile(
    app: AppHandle,
    name: String,
    version: String,
    mod_loader: String,
    loader: String,
    avatar: String,
    avatar_data: Option<String>,
) -> Result<ProfilesState, String> {
    let settings = load_settings(&app);
    let mut store = load_profiles(&app, &settings)?;
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 32 || name.chars().any(char::is_control) {
        return Err("Profile names must be 1-32 visible characters.".to_string());
    }
    if store
        .profiles
        .iter()
        .any(|profile| profile.name.eq_ignore_ascii_case(name))
    {
        return Err("A profile with that name already exists.".to_string());
    }
    if !matches!(
        mod_loader.as_str(),
        "fabric" | "quilt" | "forge" | "neoforge"
    ) {
        return Err("Choose Fabric, Quilt, Forge, or NeoForge.".to_string());
    }
    if !matches!(
        avatar.as_str(),
        "grass" | "nether" | "end" | "ocean" | "redstone" | "cherry"
    ) {
        return Err("Choose a valid profile image.".to_string());
    }
    if let Some(data) = avatar_data.as_deref() {
        validate_profile_icon_data(data)?;
    }
    let id = format!("{:032x}", rand::random::<u128>());
    let directory = profile_directory(Path::new(&store.base_dir), name, &id);
    std::fs::create_dir_all(&directory).map_err(to_str)?;
    store.profiles.push(LauncherProfile {
        id: id.clone(),
        name: name.to_string(),
        game_dir: directory.to_string_lossy().to_string(),
        version,
        mod_loader,
        loader,
        avatar,
        avatar_data,
    });
    store.active_profile = id;
    save_profiles(&app, &store)?;
    Ok(profiles_state(&store))
}

fn validate_profile_icon_data(data: &str) -> Result<(), String> {
    use base64::Engine;
    let encoded = data
        .strip_prefix("data:image/png;base64,")
        .ok_or_else(|| "Profile icons must be PNG images.".to_string())?;
    if encoded.len() > 1_400_000 {
        return Err("Profile icon must be smaller than 1 MB.".to_string());
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| "Profile icon is not valid base64 PNG data.".to_string())?;
    if bytes.len() > 1024 * 1024 || bytes.get(..8) != Some(b"\x89PNG\r\n\x1a\n") || bytes.len() < 24
    {
        return Err("Choose a valid PNG image smaller than 1 MB.".to_string());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().map_err(to_str)?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().map_err(to_str)?);
    if width == 0 || height == 0 || width > 512 || height > 512 {
        return Err("Profile icons must be between 1×1 and 512×512 pixels.".to_string());
    }
    Ok(())
}

#[tauri::command]
fn set_profile_custom_icon(
    app: AppHandle,
    id: String,
    avatar_data: String,
) -> Result<ProfilesState, String> {
    validate_profile_icon_data(&avatar_data)?;
    let mut store = load_profiles(&app, &load_settings(&app))?;
    let profile = store
        .profiles
        .iter_mut()
        .find(|profile| profile.id == id)
        .ok_or_else(|| "That launcher profile no longer exists.".to_string())?;
    profile.avatar_data = Some(avatar_data);
    save_profiles(&app, &store)?;
    Ok(profiles_state(&store))
}

#[tauri::command]
fn set_profile_avatar(app: AppHandle, id: String, avatar: String) -> Result<ProfilesState, String> {
    if !matches!(
        avatar.as_str(),
        "grass" | "nether" | "end" | "ocean" | "redstone" | "cherry"
    ) {
        return Err("Choose a valid profile image.".to_string());
    }
    let mut store = load_profiles(&app, &load_settings(&app))?;
    let profile = store
        .profiles
        .iter_mut()
        .find(|profile| profile.id == id)
        .ok_or_else(|| "That launcher profile no longer exists.".to_string())?;
    profile.avatar = avatar;
    profile.avatar_data = None;
    save_profiles(&app, &store)?;
    Ok(profiles_state(&store))
}

#[tauri::command]
fn delete_profile(app: AppHandle, id: String) -> Result<ProfilesState, String> {
    if id == "default" {
        return Err("The Default profile cannot be deleted.".to_string());
    }
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid profile selection.".to_string());
    }

    let settings = load_settings(&app);
    let mut store = load_profiles(&app, &settings)?;
    let profile = store
        .profiles
        .iter()
        .find(|profile| profile.id == id)
        .ok_or_else(|| "That launcher profile no longer exists.".to_string())?
        .clone();
    let expected_path = profile_directory(Path::new(&store.base_dir), &profile.name, &profile.id);
    let actual_path = Path::new(&profile.game_dir);
    if actual_path != expected_path {
        return Err(
            "Refusing to delete a profile folder outside its managed location.".to_string(),
        );
    }

    if actual_path.exists() {
        let metadata = std::fs::symlink_metadata(actual_path).map_err(to_str)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(
                "Refusing to delete a profile folder that is not a regular directory.".to_string(),
            );
        }
        let canonical_base = Path::new(&store.base_dir)
            .join("profiles")
            .canonicalize()
            .map_err(to_str)?;
        let canonical_profile = actual_path.canonicalize().map_err(to_str)?;
        if canonical_profile.parent() != Some(canonical_base.as_path()) {
            return Err(
                "Refusing to delete a profile folder outside its managed location.".to_string(),
            );
        }
        std::fs::remove_dir_all(&canonical_profile).map_err(to_str)?;
    }

    store.profiles.retain(|entry| entry.id != id);
    if store.active_profile == id {
        store.active_profile = "default".to_string();
    }
    save_profiles(&app, &store)?;
    Ok(profiles_state(&store))
}

#[derive(Serialize, serde::Deserialize)]
struct LauncherNewsItem {
    title: String,
    date: Option<String>,
    body: String,
    url: Option<String>,
}

#[tauri::command]
async fn fetch_launcher_news(state: State<'_, Backend>) -> Result<Vec<LauncherNewsItem>, String> {
    const NEWS_URL: &str = "https://www.melonfr.dev/mod/news.json";
    let response = state
        .client
        .get(NEWS_URL)
        .send()
        .await
        .map_err(to_str)?
        .error_for_status()
        .map_err(|error| format!("Could not load launcher news from {NEWS_URL}: {error}"))?;
    if response
        .content_length()
        .is_some_and(|length| length > 256 * 1024)
    {
        return Err("Launcher news feed exceeds the 256 KB limit.".to_string());
    }
    let bytes = response.bytes().await.map_err(to_str)?;
    if bytes.len() > 256 * 1024 {
        return Err("Launcher news feed exceeds the 256 KB limit.".to_string());
    }
    let items: Vec<LauncherNewsItem> = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Launcher news JSON is invalid: {error}"))?;
    if items.len() > 20
        || items.iter().any(|item| {
            item.title.trim().is_empty()
                || item.title.chars().count() > 120
                || item.body.chars().count() > 2000
                || item
                    .date
                    .as_ref()
                    .is_some_and(|date| date.chars().count() > 40)
                || item.url.as_ref().is_some_and(|url| {
                    reqwest::Url::parse(url)
                        .map(|parsed| parsed.scheme() != "https")
                        .unwrap_or(true)
                })
        })
    {
        return Err("Launcher news feed contains invalid or excessive entries.".to_string());
    }
    Ok(items)
}

#[cfg(test)]
mod profile_tests {
    use super::{
        minecraft_profile_url, profile_directory, profile_folder_slug, secure_mojang_skin_url,
        skin_model,
    };
    use std::path::Path;

    #[test]
    fn reads_slim_arm_model_from_mojang_skin_metadata() {
        let profile = serde_json::json!({
            "textures": {
                "SKIN": {
                    "url": "http://textures.minecraft.net/texture/example",
                    "metadata": { "model": "slim" }
                }
            }
        });

        assert_eq!(skin_model(&profile), "slim");
    }

    #[test]
    fn defaults_missing_skin_model_metadata_to_classic() {
        assert_eq!(
            skin_model(&serde_json::json!({ "textures": {} })),
            "classic"
        );
    }

    #[test]
    fn minecraft_username_is_encoded_as_a_single_url_path_segment() {
        let url = minecraft_profile_url("Any name/with?characters#fragment")
            .expect("valid profile lookup URL");

        assert_eq!(
            url.as_str(),
            "https://api.mojang.com/users/profiles/minecraft/Any%20name%2Fwith%3Fcharacters%23fragment"
        );
    }

    #[test]
    fn profile_folder_slug_is_safe_for_path_segments() {
        assert_eq!(profile_folder_slug("Modded Survival"), "modded-survival");
        assert_eq!(profile_folder_slug("../../outside"), "outside");
        assert_eq!(profile_folder_slug("###"), "");
    }

    #[test]
    fn named_profiles_use_distinct_child_directories() {
        let base = Path::new("C:\\Minecraft");
        let first = profile_directory(base, "Fabric", "0123456789abcdef");
        let second = profile_directory(base, "Forge", "fedcba9876543210");

        assert!(first.starts_with(base.join("profiles")));
        assert!(second.starts_with(base.join("profiles")));
        assert_ne!(first, second);
    }

    #[test]
    fn normalizes_mojang_http_skin_urls_to_https() {
        let url = secure_mojang_skin_url("http://textures.minecraft.net/texture/0123456789abcdef")
            .expect("valid Mojang skin URL");

        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("textures.minecraft.net"));
    }

    #[test]
    fn rejects_skin_urls_outside_the_mojang_texture_path() {
        assert!(
            secure_mojang_skin_url("https://textures.minecraft.net.evil.example/texture/abc")
                .is_err()
        );
        assert!(secure_mojang_skin_url("https://textures.minecraft.net/other/abc").is_err());
        assert!(secure_mojang_skin_url("https://user@textures.minecraft.net/texture/abc").is_err());
    }
}

#[tauri::command]
fn auth_status(app: AppHandle) -> Option<AccountSummary> {
    let store = load_auth(&app);
    store.active_account().map(AccountSummary::from_stored)
}

#[tauri::command]
fn auth_accounts(app: AppHandle) -> AccountsState {
    let store = load_auth(&app);
    AccountsState {
        active: store.active.clone(),
        accounts: store
            .accounts
            .iter()
            .map(AccountSummary::from_stored)
            .collect(),
    }
}

#[tauri::command]
fn auth_select_account(app: AppHandle, id: String) -> Result<Option<AccountSummary>, String> {
    let mut store = load_auth(&app);
    if !store.accounts.iter().any(|a| a.id == id) {
        return Err("That account no longer exists.".to_string());
    }
    store.active = Some(id);
    save_auth(&app, &store).map_err(to_str)?;
    Ok(store.active_account().map(AccountSummary::from_stored))
}

#[tauri::command]
fn auth_remove_account(app: AppHandle, id: String) -> Result<AccountsState, String> {
    let mut store = load_auth(&app);
    store.remove_account(&id);
    save_auth(&app, &store).map_err(to_str)?;
    Ok(AccountsState {
        active: store.active.clone(),
        accounts: store
            .accounts
            .iter()
            .map(AccountSummary::from_stored)
            .collect(),
    })
}

#[tauri::command]
async fn auth_login_microsoft(
    app: AppHandle,
    state: State<'_, Backend>,
) -> Result<AccountSummary, String> {
    let settings = load_settings(&app);
    let client_id = settings.azure_client_id.clone();
    let device = auth::start_device_code(&state.client, &client_id)
        .await
        .map_err(to_str)?;

    let _ = app.emit(
        "auth-code",
        serde_json::json!({
            "user_code": device.user_code,
            "verification_uri": device.verification_uri,
            "message": device.message,
        }),
    );

    let tokens = auth::poll_for_token(
        &state.client,
        &client_id,
        &device.device_code,
        device.interval,
    )
    .await
    .map_err(to_str)?;
    let profile = auth::authenticate_minecraft(&state.client, &tokens.access_token)
        .await
        .map_err(to_str)?;

    let mut store = load_auth(&app);
    store.add_account(StoredAccount {
        id: format!("msa:{}", profile.uuid),
        name: profile.name.clone(),
        uuid: profile.uuid.clone(),
        kind: "msa".to_string(),
        flow: "consumers".to_string(),
        refresh_token: tokens.refresh_token,
    });
    save_auth(&app, &store).map_err(to_str)?;

    let _ = app.emit(
        "auth-code",
        serde_json::json!({ "user_code": "", "verification_uri": "", "message": "" }),
    );

    Ok(AccountSummary {
        id: format!("msa:{}", profile.uuid),
        name: profile.name,
        uuid: profile.uuid,
        kind: "msa".to_string(),
        online: true,
    })
}

#[tauri::command]
async fn auth_login_browser(
    app: AppHandle,
    state: State<'_, Backend>,
) -> Result<AccountSummary, String> {
    let (profile, refresh) = auth::login_minecraft_browser(&state.client, app.clone())
        .await
        .map_err(to_str)?;

    let mut store = load_auth(&app);
    store.add_account(StoredAccount {
        id: format!("msa:{}", profile.uuid),
        name: profile.name.clone(),
        uuid: profile.uuid.clone(),
        kind: "msa".to_string(),
        flow: "live".to_string(),
        refresh_token: refresh,
    });
    save_auth(&app, &store).map_err(to_str)?;

    Ok(AccountSummary {
        id: format!("msa:{}", profile.uuid),
        name: profile.name,
        uuid: profile.uuid,
        kind: "msa".to_string(),
        online: true,
    })
}

#[tauri::command]
fn auth_logout(app: AppHandle) -> Result<(), String> {
    save_auth(&app, &AuthStore::default()).map_err(to_str)
}

#[tauri::command]
async fn account_textures(
    state: State<'_, Backend>,
    uuid: String,
) -> Result<auth::Textures, String> {
    auth::fetch_textures(&state.client, &uuid)
        .await
        .map_err(to_str)
}

/// Looks an account up in the saved store by its Minecraft UUID.
async fn account_by_uuid(app: &AppHandle, uuid: &str) -> Result<StoredAccount, String> {
    let store = load_auth(app);
    store
        .accounts
        .into_iter()
        .find(|candidate| candidate.uuid == uuid)
        .ok_or_else(|| "Account not found.".to_string())
}

/// Resolves the Minecraft session for an account, reusing a cached access token
/// where possible and persisting a rotated Microsoft refresh token when one
/// comes back. This is what stopped capes from failing after a few clicks: the
/// old code re-ran the whole Xbox chain (and burned the single-use refresh
/// token) on every single command.
async fn mc_session(
    app: &AppHandle,
    client: &reqwest::Client,
    account: &StoredAccount,
) -> Result<auth::McProfile, String> {
    let settings = load_settings(app);
    let (profile, rotated) =
        auth::resolve_account_cached(client, &settings.azure_client_id, account)
            .await
            .map_err(to_str)?;
    if let Some(refresh) = rotated {
        let mut store = load_auth(app);
        if let Some(existing) = store.accounts.iter_mut().find(|a| a.id == account.id) {
            existing.refresh_token = Some(refresh);
            save_auth(app, &store).map_err(to_str)?;
        }
    }
    Ok(profile)
}

/// If Mojang rejects the token we still hold, throw the cached copy away,
/// sign in once more and retry the operation a single time.
async fn fetch_capes_with_retry(
    app: &AppHandle,
    client: &reqwest::Client,
    account: &StoredAccount,
) -> Result<Vec<auth::Cape>, String> {
    let profile = mc_session(app, client, account).await?;
    match auth::fetch_capes(client, &profile.access_token).await {
        Ok(capes) => Ok(capes),
        Err(_) => {
            auth::invalidate_token(&account.id);
            let profile = mc_session(app, client, account).await?;
            auth::fetch_capes(client, &profile.access_token)
                .await
                .map_err(to_str)
        }
    }
}

async fn set_cape_with_retry(
    app: &AppHandle,
    client: &reqwest::Client,
    account: &StoredAccount,
    cape_id: Option<&str>,
) -> Result<Vec<auth::Cape>, String> {
    let profile = mc_session(app, client, account).await?;
    match auth::set_active_cape(client, &profile.access_token, cape_id).await {
        Ok(capes) => Ok(capes),
        Err(_) => {
            auth::invalidate_token(&account.id);
            let profile = mc_session(app, client, account).await?;
            auth::set_active_cape(client, &profile.access_token, cape_id)
                .await
                .map_err(to_str)
        }
    }
}

async fn set_skin_with_retry(
    app: &AppHandle,
    client: &reqwest::Client,
    account: &StoredAccount,
    bytes: Vec<u8>,
    model: &str,
) -> Result<(), String> {
    let profile = mc_session(app, client, account).await?;
    match auth::set_skin(client, &profile.access_token, bytes.clone(), model).await {
        Ok(()) => Ok(()),
        Err(_) => {
            auth::invalidate_token(&account.id);
            let profile = mc_session(app, client, account).await?;
            auth::set_skin(client, &profile.access_token, bytes, model)
                .await
                .map_err(to_str)
        }
    }
}

#[tauri::command]
async fn account_capes(
    app: AppHandle,
    state: State<'_, Backend>,
    uuid: String,
) -> Result<Vec<auth::Cape>, String> {
    let account = account_by_uuid(&app, &uuid).await?;
    if account.kind != "msa" {
        return Ok(Vec::new());
    }
    fetch_capes_with_retry(&app, &state.client, &account).await
}

#[tauri::command]
async fn set_cape(
    app: AppHandle,
    state: State<'_, Backend>,
    uuid: String,
    cape_id: Option<String>,
) -> Result<Vec<auth::Cape>, String> {
    let account = account_by_uuid(&app, &uuid).await?;
    if account.kind != "msa" {
        return Err("Capes require a Microsoft account.".to_string());
    }

    // The PUT already returns the updated profile, so one request is enough.
    let capes = set_cape_with_retry(&app, &state.client, &account, cape_id.as_deref()).await?;

    let _ = app.emit(
        "log",
        match &cape_id {
            Some(id) => format!("Equipped cape {id}"),
            None => "Removed cape".to_string(),
        },
    );
    Ok(capes)
}

#[tauri::command]
async fn set_account_skin(
    app: AppHandle,
    state: State<'_, Backend>,
    uuid: String,
    skin_id: String,
    model: String,
) -> Result<(), String> {
    if skin_id.len() != 32 || !skin_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid saved skin selection.".to_string());
    }
    if !matches!(model.as_str(), "classic" | "slim") {
        return Err("Choose either the Classic or Slim skin model.".to_string());
    }

    let account = account_by_uuid(&app, &uuid).await?;
    if account.kind != "msa" {
        return Err("Changing your Minecraft skin requires a Microsoft account.".to_string());
    }

    let settings = active_profile_settings(&app)?;
    let (skins_dir, skins_file) = skin_library_paths(&settings);
    let skins = read_skin_library(&skins_file)?;
    if !skins.iter().any(|skin| skin.id == skin_id) {
        return Err("Saved skin was not found.".to_string());
    }
    let bytes = std::fs::read(skins_dir.join(format!("{skin_id}.png"))).map_err(to_str)?;
    validate_skin_png(&bytes)?;

    set_skin_with_retry(&app, &state.client, &account, bytes, &model).await?;
    let _ = app.emit(
        "log",
        format!("Changed Minecraft skin for {}", account.name),
    );
    Ok(())
}

#[tauri::command]
async fn minecraft_versions(state: State<'_, Backend>) -> Result<Vec<Value>, String> {
    let manifest: Value = state
        .client
        .get(game::VERSION_MANIFEST)
        .send()
        .await
        .map_err(to_str)?
        .error_for_status()
        .map_err(to_str)?
        .json()
        .await
        .map_err(to_str)?;
    Ok(manifest["versions"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|entry| {
            serde_json::json!({
                "id": entry["id"],
                "type": entry["type"],
            })
        })
        .collect())
}

#[tauri::command]
async fn loader_versions(
    state: State<'_, Backend>,
    mc: String,
    mod_loader: String,
) -> Result<Vec<String>, String> {
    game::loader_versions(&state.client, &mc, &mod_loader)
        .await
        .map_err(to_str)
}

#[tauri::command]
async fn launch(app: AppHandle, state: State<'_, Backend>) -> Result<u32, String> {
    if state
        .launch_in_progress
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err("Minecraft is already launching or running.".to_string());
    }
    let _launch_reset = LaunchReset(state.launch_in_progress.clone());
    let settings = active_profile_settings(&app)?;
    let profiles = load_profiles(&app, &load_settings(&app))?;
    let active_profile = profiles
        .profiles
        .iter()
        .find(|profile| profile.id == profiles.active_profile)
        .ok_or_else(|| "The active launcher profile was not found.".to_string())?;
    let mut store = load_auth(&app);

    let account = store
        .active_account()
        .cloned()
        .ok_or_else(|| "You are not signed in. Add an account first.".to_string())?;

    let (profile, rotated) =
        auth::resolve_account_cached(&state.client, &settings.azure_client_id, &account)
            .await
            .map_err(to_str)?;
    if let Some(refresh) = rotated {
        if let Some(existing) = store.accounts.iter_mut().find(|a| a.id == account.id) {
            existing.refresh_token = Some(refresh);
        }
    }
    if let Some(existing) = store.accounts.iter_mut().find(|a| a.id == account.id) {
        existing.name = profile.name.clone();
        existing.uuid = profile.uuid.clone();
    }
    save_auth(&app, &store).map_err(to_str)?;

    let resolved = game::resolve_version(&state.client, &settings)
        .await
        .map_err(to_str)?;
    let paths = game::install(&state.client, &app, &settings, &resolved)
        .await
        .map_err(to_str)?;
    let _ = app.emit("progress-complete", ());

    let _ = app.emit(
        "log",
        format!(
            "Launching Minecraft {} ({}) as {}",
            resolved.mc, resolved.loader, profile.name
        ),
    );
    game::launch(
        &state.client,
        &app,
        &settings,
        &resolved,
        &profile,
        &paths,
        state.game_processes.clone(),
        state.stopped_processes.clone(),
        active_profile.id.clone(),
        active_profile.name.clone(),
    )
    .await
    .map_err(to_str)
}

#[tauri::command]
fn list_running_games(app: AppHandle) -> Result<Vec<game::RunningGame>, String> {
    game::list_running_games(&app).map_err(to_str)
}

#[tauri::command]
async fn stop_game(app: AppHandle, state: State<'_, Backend>, pid: u32) -> Result<(), String> {
    {
        let mut processes = state.game_processes.lock().await;
        if let Some(child) = processes.get_mut(&pid) {
            child
                .start_kill()
                .map_err(|error| format!("Could not stop Minecraft: {error}"))?;
            state.stopped_processes.lock().await.insert(pid);
            return Ok(());
        }
    }

    let game = game::stop_recovered_game(&app, pid).map_err(to_str)?;
    let _ = app.emit(
        "game-exit",
        serde_json::json!({
            "code": 0,
            "success": true,
            "stopped": true,
            "pid": game.pid,
            "profile_id": game.profile_id,
            "profile_name": game.profile_name,
        }),
    );
    Ok(())
}

/// Opens the native file picker so the user can point the launcher at a
/// background picture or video. Returns the chosen path, or nothing on cancel.
#[tauri::command]
async fn pick_background_file(app: AppHandle) -> Result<Option<String>, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter(
            "Media",
            &[
                "png", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "bmp", "ico", "avif", "tiff",
                "mp4", "m4v", "webm", "mkv", "mov", "avi", "wmv", "flv", "ts", "mpg", "mpeg",
            ],
        )
        .pick_file(move |path| {
            let _ = sender.send(path);
        });
    match receiver.await {
        Ok(path) => Ok(path.map(|value| value.to_string())),
        Err(_) => Ok(None),
    }
}

#[tauri::command]
async fn pick_skin_file(app: AppHandle) -> Result<Option<String>, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Minecraft skin PNG", &["png"])
        .pick_file(move |path| {
            let _ = sender.send(path);
        });
    match receiver.await {
        Ok(path) => Ok(path.map(|value| value.to_string())),
        Err(_) => Ok(None),
    }
}

#[tauri::command]
async fn pick_profile_icon_file(app: AppHandle) -> Result<Option<String>, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("PNG profile icon", &["png"])
        .pick_file(move |path| {
            let _ = sender.send(path);
        });
    match receiver.await {
        Ok(path) => Ok(path.map(|value| value.to_string())),
        Err(_) => Err("Could not open the profile icon picker.".to_string()),
    }
}

#[tauri::command]
async fn pick_modpack_file(app: AppHandle) -> Result<Option<String>, String> {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .add_filter("Modrinth modpack", &["mrpack"])
        .pick_file(move |path| {
            let _ = sender.send(path);
        });
    match receiver.await {
        Ok(path) => Ok(path.map(|value| value.to_string())),
        Err(_) => Ok(None),
    }
}

/// Installs a `.mrpack` file that was picked or dropped into the active profile.
#[tauri::command]
async fn import_modpack(
    app: AppHandle,
    state: State<'_, Backend>,
    source: String,
) -> Result<String, String> {
    let settings = active_profile_settings(&app)?;
    modrinth::import_local_modpack(&state.client, &settings.minecraft_dir, &source)
        .await
        .map_err(to_str)
}

#[tauri::command]
fn read_profile_icon(source: String) -> Result<String, String> {
    use base64::Engine;
    let source = Path::new(source.trim().trim_matches('"'));
    let metadata = std::fs::metadata(source).map_err(to_str)?;
    if metadata.len() > 1024 * 1024 {
        return Err("Profile icon must be smaller than 1 MB.".to_string());
    }
    let bytes = std::fs::read(source).map_err(to_str)?;
    let data = format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    );
    validate_profile_icon_data(&data)?;
    Ok(data)
}

#[tauri::command]
fn import_local_skin(
    app: AppHandle,
    source: String,
    name: String,
) -> Result<LocalSkinEntry, String> {
    let settings = active_profile_settings(&app)?;
    let source = std::path::PathBuf::from(source.trim().trim_matches('"'));
    let bytes = std::fs::read(&source).map_err(to_str)?;
    validate_skin_png(&bytes)?;

    let name = name.trim();
    let name = if name.is_empty() {
        source
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("My skin")
    } else {
        name
    };
    let name: String = name
        .chars()
        .filter(|ch| !ch.is_control())
        .take(48)
        .collect();
    if name.is_empty() {
        return Err("Skin name must contain at least one visible character.".to_string());
    }
    save_skin_to_library(&settings, &name, &bytes, "classic")
}

fn save_skin_to_library(
    settings: &Settings,
    name: &str,
    bytes: &[u8],
    model: &str,
) -> Result<LocalSkinEntry, String> {
    validate_skin_png(bytes)?;
    let (config_dir, skins_file) = skin_library_paths(&settings);
    std::fs::create_dir_all(&config_dir).map_err(to_str)?;
    let mut skins = read_skin_library(&skins_file)?;
    let id = format!("{:032x}", rand::random::<u128>());
    let path = config_dir.join(format!("{id}.png"));
    std::fs::write(&path, bytes).map_err(to_str)?;
    let entry = LocalSkinEntry {
        id,
        name: name.to_string(),
        path: path.to_string_lossy().to_string(),
        model: model.to_string(),
    };
    skins.push(entry.clone());
    if let Err(error) = write_skin_library(&skins_file, &skins) {
        let _ = std::fs::remove_file(path);
        return Err(error);
    }
    Ok(entry)
}

#[tauri::command]
async fn download_skin_by_username(
    app: AppHandle,
    state: State<'_, Backend>,
    username: String,
) -> Result<LocalSkinEntry, String> {
    let username = username.trim();
    if username.is_empty() {
        return Err("Enter a Minecraft username first.".to_string());
    }

    let profile_url = minecraft_profile_url(username)?;
    let response = state.client.get(profile_url).send().await.map_err(to_str)?;
    if !response.status().is_success() {
        return Err(format!(
            "Could not find a Minecraft profile for '{username}'. The name may not exist or may have changed."
        ));
    }
    let profile: Value = response.json().await.map_err(to_str)?;
    let uuid = profile["id"]
        .as_str()
        .filter(|id| id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| "Minecraft returned an invalid profile.".to_string())?;
    let profile_name = profile["name"].as_str().unwrap_or(username);

    let signed_profile: Value = state
        .client
        .get(format!(
            "https://sessionserver.mojang.com/session/minecraft/profile/{uuid}"
        ))
        .query(&[("unsigned", "false")])
        .send()
        .await
        .map_err(to_str)?
        .error_for_status()
        .map_err(|error| format!("Could not load {profile_name}'s skin profile: {error}"))?
        .json()
        .await
        .map_err(to_str)?;
    let encoded_textures = signed_profile["properties"]
        .as_array()
        .and_then(|properties| {
            properties.iter().find_map(|property| {
                (property["name"].as_str() == Some("textures"))
                    .then(|| property["value"].as_str())
                    .flatten()
            })
        })
        .ok_or_else(|| format!("{profile_name} does not have a public skin."))?;
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded_textures)
        .map_err(to_str)?;
    let texture_profile: Value = serde_json::from_slice(&decoded).map_err(to_str)?;
    let skin_url = texture_profile["textures"]["SKIN"]["url"]
        .as_str()
        .ok_or_else(|| format!("{profile_name} does not have a public skin."))?;
    let model = skin_model(&texture_profile);
    let parsed_url = secure_mojang_skin_url(skin_url)?;
    let response = state
        .client
        .get(parsed_url)
        .send()
        .await
        .map_err(to_str)?
        .error_for_status()
        .map_err(to_str)?;
    if response
        .content_length()
        .is_some_and(|length| length > 10 * 1024 * 1024)
    {
        return Err("Skin download exceeds the 10 MB limit.".to_string());
    }
    let bytes = response.bytes().await.map_err(to_str)?;
    validate_skin_png(&bytes)?;

    let settings = active_profile_settings(&app)?;
    save_skin_to_library(&settings, &format!("{profile_name}"), &bytes, model)
}

#[tauri::command]
fn local_skins(app: AppHandle) -> Result<Vec<LocalSkinEntry>, String> {
    let settings = active_profile_settings(&app)?;
    let (skins_dir, skins_file) = skin_library_paths(&settings);
    let skins = read_skin_library(&skins_file)?;
    Ok(skins
        .into_iter()
        .filter(|skin| skins_dir.join(format!("{}.png", skin.id)).is_file())
        .map(|mut skin| {
            skin.path = skins_dir
                .join(format!("{}.png", skin.id))
                .to_string_lossy()
                .to_string();
            skin
        })
        .collect())
}

#[derive(Serialize, serde::Deserialize, Clone)]
struct LocalSkinEntry {
    id: String,
    name: String,
    #[serde(default)]
    path: String,
    #[serde(default = "default_skin_model")]
    model: String,
}

fn default_skin_model() -> String {
    "classic".to_string()
}

fn skin_library_paths(settings: &Settings) -> (PathBuf, PathBuf) {
    let config_dir = Path::new(&settings.minecraft_dir)
        .join("config")
        .join("melonclient")
        .join("skins");
    let metadata_path = config_dir.join("skins.json");
    (config_dir, metadata_path)
}

fn read_skin_library(path: &Path) -> Result<Vec<LocalSkinEntry>, String> {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).map_err(to_str),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(to_str(error)),
    }
}

fn write_skin_library(path: &Path, skins: &[LocalSkinEntry]) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(skins).map_err(to_str)?;
    std::fs::write(path, bytes).map_err(to_str)
}

fn validate_skin_png(bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > 10 * 1024 * 1024 {
        return Err("Skin PNG must be no larger than 10 MB.".to_string());
    }
    if bytes.get(..8) != Some(b"\x89PNG\r\n\x1a\n") || bytes.len() < 24 {
        return Err("Choose a valid PNG skin image.".to_string());
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().map_err(to_str)?);
    let height = u32::from_be_bytes(bytes[20..24].try_into().map_err(to_str)?);
    if width != 64 || (height != 64 && height != 32) {
        return Err("Skin PNG must be 64x64 or 64x32 pixels.".to_string());
    }
    Ok(())
}

#[tauri::command]
fn remove_local_skin(app: AppHandle, id: String) -> Result<(), String> {
    if id.len() != 32 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Invalid saved skin selection.".to_string());
    }

    let settings = active_profile_settings(&app)?;
    let (skins_dir, skins_file) = skin_library_paths(&settings);
    let mut skins = read_skin_library(&skins_file)?;
    if !skins.iter().any(|skin| skin.id == id) {
        return Err("Saved skin was not found.".to_string());
    }
    skins.retain(|skin| skin.id != id);
    write_skin_library(&skins_file, &skins)?;

    let skin_path = skins_dir.join(format!("{id}.png"));
    match std::fs::remove_file(skin_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(to_str(error)),
    }

    Ok(())
}

#[tauri::command]
async fn modrinth_search(
    state: State<'_, Backend>,
    query: String,
    version: String,
    loader: String,
    project_type: String,
    category: String,
    sort: String,
) -> Result<Vec<modrinth::SearchHit>, String> {
    modrinth::search(
        &state.client,
        &query,
        &version,
        &loader,
        &project_type,
        &category,
        &sort,
    )
    .await
    .map_err(to_str)
}

#[tauri::command]
async fn modrinth_install(
    app: AppHandle,
    state: State<'_, Backend>,
    project_id: String,
    version: String,
    loader: String,
    project_type: String,
) -> Result<String, String> {
    let settings = active_profile_settings(&app)?;
    modrinth::install(
        &state.client,
        &settings.minecraft_dir,
        &project_id,
        &version,
        &loader,
        &project_type,
    )
    .await
    .map_err(to_str)
}

#[derive(Serialize)]
struct InstalledMod {
    file_name: String,
    name: String,
    version: Option<String>,
    size_bytes: u64,
}

fn jar_mod_metadata(path: &Path, fallback_name: &str) -> (String, Option<String>) {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return (fallback_name.to_string(), None),
    };
    let mut archive = match zip::ZipArchive::new(file) {
        Ok(archive) => archive,
        Err(_) => return (fallback_name.to_string(), None),
    };
    for metadata_file in ["fabric.mod.json", "quilt.mod.json"] {
        let entry = match archive.by_name(metadata_file) {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let mut bytes = Vec::new();
        if entry.take(128 * 1024).read_to_end(&mut bytes).is_err() {
            continue;
        }
        let metadata: Value = match serde_json::from_slice(&bytes) {
            Ok(metadata) => metadata,
            Err(_) => continue,
        };
        let metadata = if metadata_file == "quilt.mod.json" {
            &metadata["quilt_loader"]["metadata"]
        } else {
            &metadata
        };
        let name = metadata["name"]
            .as_str()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(fallback_name)
            .to_string();
        let version = metadata["version"].as_str().map(str::to_string);
        return (name, version);
    }
    for metadata_file in ["META-INF/mods.toml", "META-INF/neoforge.mods.toml"] {
        let entry = match archive.by_name(metadata_file) {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let mut bytes = Vec::new();
        if entry.take(128 * 1024).read_to_end(&mut bytes).is_err() {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        let value = |key: &str| {
            text.lines().find_map(|line| {
                let (name, value) = line.trim().split_once('=')?;
                if name.trim() != key {
                    return None;
                }
                let value = value.trim().trim_matches('"');
                (!value.is_empty() && !value.starts_with("${")).then(|| value.to_string())
            })
        };
        let name = value("displayName")
            .or_else(|| value("modId"))
            .unwrap_or_else(|| fallback_name.to_string());
        return (name, value("version"));
    }
    (fallback_name.to_string(), None)
}

#[tauri::command]
fn list_installed_content(
    app: AppHandle,
    project_type: String,
) -> Result<Vec<InstalledMod>, String> {
    let settings = active_profile_settings(&app)?;
    let directory = modrinth::content_directory(&project_type).map_err(to_str)?;
    let extension = if project_type == "mod" { "jar" } else { "zip" };
    let content_dir = Path::new(&settings.minecraft_dir).join(directory);
    let entries = match std::fs::read_dir(content_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(to_str(error)),
    };
    let mut mods = Vec::new();
    for entry in entries {
        let entry = entry.map_err(to_str)?;
        let path = entry.path();
        if !path.is_file()
            || !path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case(extension))
        {
            continue;
        }
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string();
        let size_bytes = entry.metadata().map_err(to_str)?.len();
        let fallback_name = file_name
            .rsplit_once('.')
            .map(|(name, _)| name)
            .unwrap_or(&file_name);
        let (name, version) = if project_type == "mod" {
            jar_mod_metadata(&path, fallback_name)
        } else {
            (fallback_name.to_string(), None)
        };
        mods.push(InstalledMod {
            file_name,
            name,
            version,
            size_bytes,
        });
    }
    mods.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    Ok(mods)
}

#[tauri::command]
fn open_folder(path: String) -> Result<(), String> {
    if path.trim().is_empty() {
        return Err("No folder configured.".to_string());
    }
    opener::open(path).map_err(to_str)
}

// ---------------------------------------------------------------------------
// Custom window controls
//
// These are plain commands rather than `window.__TAURI__.window` calls, so the
// titlebar buttons work regardless of what the injected JS API exposes or which
// `core:window:allow-*` capabilities are enabled.
// ---------------------------------------------------------------------------

fn main_window(app: &AppHandle) -> Result<tauri::WebviewWindow, String> {
    app.get_webview_window("main")
        .ok_or_else(|| "Main window not found.".to_string())
}

#[tauri::command]
fn win_minimize(app: AppHandle) -> Result<(), String> {
    main_window(&app)?.minimize().map_err(to_str)
}

#[tauri::command]
fn win_toggle_maximize(app: AppHandle) -> Result<(), String> {
    let window = main_window(&app)?;
    if window.is_maximized().map_err(to_str)? {
        window.unmaximize().map_err(to_str)
    } else {
        window.maximize().map_err(to_str)
    }
}

#[tauri::command]
fn win_close(app: AppHandle) -> Result<(), String> {
    main_window(&app)?.close().map_err(to_str)
}

#[tauri::command]
fn hide_window(app: AppHandle) -> Result<(), String> {
    main_window(&app)?.hide().map_err(to_str)
}

#[tauri::command]
fn win_start_dragging(app: AppHandle) -> Result<(), String> {
    main_window(&app)?.start_dragging().map_err(to_str)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let client = reqwest::Client::builder()
        .user_agent("melonclient-launcher/0.1.0")
        .build()
        .expect("failed to build http client");

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(Backend {
            client,
            game_processes: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            stopped_processes: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            launch_in_progress: Arc::new(AtomicBool::new(false)),
        })
        .setup(|app| {
            let show_item = MenuItem::with_id(app, "show", "Show launcher", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit launcher", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &quit_item])?;
            let mut tray = TrayIconBuilder::new()
                .menu(&menu)
                .tooltip("Melon Client Launcher")
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                });
            if let Some(icon) = app.default_window_icon() {
                tray = tray.icon(icon.clone());
            }
            tray.build(app)?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let tracked_games_running = match game::list_running_games(app) {
                    Ok(games) => !games.is_empty(),
                    Err(error) => {
                        eprintln!("Could not check for running Minecraft instances: {error}");
                        false
                    }
                };
                let launcher_tracked_games_running =
                    match app.state::<Backend>().game_processes.try_lock() {
                        Ok(processes) => !processes.is_empty(),
                        Err(_) => true,
                    };
                if tracked_games_running || launcher_tracked_games_running {
                    api.prevent_close();
                    if let Err(error) = window.hide() {
                        eprintln!("Could not hide launcher while Minecraft is running: {error}");
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_settings,
            reset_settings,
            list_profiles,
            list_running_games,
            select_profile,
            create_profile,
            set_profile_avatar,
            set_profile_custom_icon,
            delete_profile,
            fetch_launcher_news,
            auth_status,
            auth_accounts,
            auth_select_account,
            auth_remove_account,
            auth_login_microsoft,
            auth_login_browser,
            auth_logout,
            account_textures,
            account_capes,
            set_cape,
            set_account_skin,
            minecraft_versions,
            loader_versions,
            launch,
            stop_game,
            open_folder,
            pick_background_file,
            pick_skin_file,
            pick_profile_icon_file,
            pick_modpack_file,
            import_modpack,
            read_profile_icon,
            import_local_skin,
            download_skin_by_username,
            local_skins,
            remove_local_skin,
            modrinth_search,
            modrinth_install,
            list_installed_content,
            win_minimize,
            win_toggle_maximize,
            win_close,
            win_start_dragging,
            hide_window,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the launcher");
}
