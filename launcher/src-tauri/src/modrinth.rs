use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::io::Read;
use std::path::Path;

const API: &str = "https://api.modrinth.com/v2";
const MAX_CONTENT_SIZE: usize = 100 * 1024 * 1024;

#[derive(Serialize)]
pub struct SearchHit {
    id: String,
    title: String,
    description: String,
    icon_url: Option<String>,
    downloads: u64,
}

pub async fn search(
    client: &reqwest::Client,
    query: &str,
    version: &str,
    loader: &str,
    project_type: &str,
    category: &str,
    sort: &str,
) -> Result<Vec<SearchHit>> {
    let query = query.trim();
    if query.len() > 120 {
        bail!("Search text must be 120 characters or fewer.");
    }
    validate_project_type(project_type)?;
    if matches!(project_type, "mod" | "modpack") {
        validate_loader(loader)?;
    }
    if !matches!(
        category,
        "" | "adventure"
            | "decoration"
            | "economy"
            | "food"
            | "game-mechanics"
            | "library"
            | "magic"
            | "management"
            | "minigame"
            | "mobs"
            | "optimization"
            | "social"
            | "storage"
            | "technology"
            | "transportation"
            | "utility"
            | "worldgen"
    ) {
        bail!("Choose a valid Modrinth category.");
    }
    if !matches!(sort, "relevance" | "downloads" | "follows" | "updated") {
        bail!("Choose a valid Modrinth sort order.");
    }
    let minecraft_version = resolve_minecraft_version(client, version).await?;
    let mut facets = vec![
        vec![format!("project_type:{project_type}")],
        vec![format!("versions:{minecraft_version}")],
    ];
    if matches!(project_type, "mod" | "modpack") {
        facets.push(vec![format!("categories:{loader}")]);
    }
    if project_type == "mod" && !category.is_empty() {
        facets.push(vec![format!("categories:{category}")]);
    }
    let facets = serde_json::to_string(&facets)?;
    let response: Value = client
        .get(format!("{API}/search"))
        .query(&[
            ("query", query),
            ("limit", "24"),
            ("facets", &facets),
            ("index", sort),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let hits = response["hits"]
        .as_array()
        .ok_or_else(|| anyhow!("Modrinth returned an invalid search response."))?;

    Ok(hits
        .iter()
        .filter_map(|hit| {
            Some(SearchHit {
                id: hit["project_id"].as_str()?.to_string(),
                title: hit["title"].as_str()?.to_string(),
                description: hit["description"].as_str().unwrap_or_default().to_string(),
                icon_url: hit["icon_url"].as_str().map(str::to_string),
                downloads: hit["downloads"].as_u64().unwrap_or_default(),
            })
        })
        .collect())
}

pub async fn install(
    client: &reqwest::Client,
    minecraft_dir: &str,
    project_id: &str,
    version: &str,
    loader: &str,
    project_type: &str,
) -> Result<String> {
    if project_id.is_empty()
        || project_id.len() > 64
        || !project_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        bail!("Invalid Modrinth project id.");
    }
    validate_project_type(project_type)?;
    if matches!(project_type, "mod" | "modpack") {
        validate_loader(loader)?;
    }
    let minecraft_version = resolve_minecraft_version(client, version).await?;
    if project_type == "modpack" {
        return install_modpack(client, minecraft_dir, project_id, &minecraft_version, loader).await;
    }
    let game_versions = serde_json::to_string(&[&minecraft_version])?;
    let loaders = serde_json::to_string(&[loader])?;
    let mut query = vec![("game_versions", game_versions)];
    if project_type == "mod" {
        query.push(("loaders", loaders));
    }
    let versions: Value = client
        .get(format!("{API}/project/{project_id}/version"))
        .query(&query)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let extension = if project_type == "mod" {
        ".jar"
    } else {
        ".zip"
    };
    let version = versions
        .as_array()
        .and_then(|versions| versions.first())
        .ok_or_else(|| {
            anyhow!("No compatible {project_type} release is available for Minecraft {minecraft_version}.")
        })?;
    let files = version["files"]
        .as_array()
        .ok_or_else(|| anyhow!("Modrinth returned a release without files."))?;
    let file = files
        .iter()
        .filter(|file| {
            file["filename"]
                .as_str()
                .is_some_and(|name| name.to_ascii_lowercase().ends_with(extension))
        })
        .find(|file| file["primary"].as_bool() == Some(true))
        .or_else(|| {
            files.iter().find(|file| {
                file["filename"]
                    .as_str()
                    .is_some_and(|name| name.to_ascii_lowercase().ends_with(extension))
            })
        })
        .ok_or_else(|| anyhow!("The selected Modrinth release has no downloadable files."))?;
    let url = file["url"]
        .as_str()
        .ok_or_else(|| anyhow!("Modrinth release is missing its download URL."))?;
    let parsed_url = reqwest::Url::parse(url)?;
    if parsed_url.scheme() != "https" || parsed_url.host_str() != Some("cdn.modrinth.com") {
        bail!("Modrinth returned an unexpected download host.");
    }
    let filename = safe_filename(
        file["filename"]
            .as_str()
            .ok_or_else(|| anyhow!("Modrinth release is missing its filename."))?,
        extension,
    )?;

    let bytes = download_capped(client, parsed_url, "file").await?;
    let archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.as_slice()))
        .context("The file downloaded from Modrinth is not a valid zip archive.")?;
    if archive.is_empty() {
        bail!("The file downloaded from Modrinth is an empty archive.");
    }
    drop(archive);

    let content_dir = Path::new(minecraft_dir).join(content_directory(project_type)?);
    std::fs::create_dir_all(&content_dir)?;
    let destination = content_dir.join(filename);
    if destination.exists() {
        bail!(
            "{} is already present in this profile's {} folder.",
            destination
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
            content_directory(project_type)?
        );
    }
    let temporary =
        destination.with_extension(format!("{}.download", extension.trim_start_matches('.')));
    std::fs::write(&temporary, bytes)?;
    if let Err(error) = std::fs::rename(&temporary, &destination) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error).context("Could not place the downloaded file in this profile.");
    }
    Ok(filename.to_string())
}

/// Installs a Modrinth `.mrpack` modpack: downloads the pack archive, reads its
/// `modrinth.index.json`, downloads every file it lists, and unpacks the
/// `overrides/` (and `client-overrides/`) payloads into the profile.
async fn install_modpack(
    client: &reqwest::Client,
    minecraft_dir: &str,
    project_id: &str,
    minecraft_version: &str,
    loader: &str,
) -> Result<String> {
    let game_versions = serde_json::to_string(&[minecraft_version])?;
    let loaders = serde_json::to_string(&[loader])?;
    let versions: Value = client
        .get(format!("{API}/project/{project_id}/version"))
        .query(&[("game_versions", game_versions), ("loaders", loaders)])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let version = versions.as_array().and_then(|versions| versions.first()).ok_or_else(|| {
        anyhow!("No compatible modpack release is available for Minecraft {minecraft_version}.")
    })?;
    let files = version["files"]
        .as_array()
        .ok_or_else(|| anyhow!("Modrinth returned a release without files."))?;
    let file = files
        .iter()
        .filter(|file| {
            file["filename"]
                .as_str()
                .is_some_and(|name| name.to_ascii_lowercase().ends_with(".mrpack"))
        })
        .find(|file| file["primary"].as_bool() == Some(true))
        .or_else(|| {
            files.iter().find(|file| {
                file["filename"]
                    .as_str()
                    .is_some_and(|name| name.to_ascii_lowercase().ends_with(".mrpack"))
            })
        })
        .ok_or_else(|| anyhow!("The selected Modrinth modpack release has no .mrpack file."))?;
    let url = file["url"]
        .as_str()
        .ok_or_else(|| anyhow!("Modrinth release is missing its download URL."))?;
    let parsed_url = reqwest::Url::parse(url)?;
    if parsed_url.scheme() != "https" || parsed_url.host_str() != Some("cdn.modrinth.com") {
        bail!("Modrinth returned an unexpected download host.");
    }

    let pack_bytes = download_capped(client, parsed_url, "modpack").await?;
    install_modpack_archive(client, minecraft_dir, &pack_bytes).await
}

/// Installs a `.mrpack` modpack from raw bytes: reads its
/// `modrinth.index.json`, downloads every file it lists, and unpacks the
/// `overrides/` (and `client-overrides/`) payloads into the profile.
async fn install_modpack_archive(
    client: &reqwest::Client,
    minecraft_dir: &str,
    pack_bytes: &[u8],
) -> Result<String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(pack_bytes))
        .context("The downloaded modpack is not a valid zip archive.")?;

    let index_bytes = {
        let mut entry = match archive.by_name("modrinth.index.json") {
            Ok(entry) => entry,
            Err(_) => bail!("This modpack is missing its modrinth.index.json."),
        };
        let mut bytes = Vec::new();
        entry
            .by_ref()
            .take(8 * 1024 * 1024)
            .read_to_end(&mut bytes)?;
        bytes
    };
    let index: Value =
        serde_json::from_slice(&index_bytes).context("modrinth.index.json is not valid JSON.")?;
    let format_version = index["formatVersion"]
        .as_u64()
        .or_else(|| index["formatVersion"].as_f64().map(|value| value as u64))
        .unwrap_or(0);
    if format_version != 1 {
        bail!("This modpack uses an unsupported format version.");
    }
    if index["game"].as_str() != Some("minecraft") {
        bail!("This modpack is not for Minecraft.");
    }
    let pack_name = index["name"]
        .as_str()
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("Modpack");

    let root = Path::new(minecraft_dir);
    let mut downloaded = 0usize;
    if let Some(entries) = index["files"].as_array() {
        for entry in entries {
            let client_env = entry["env"]["client"].as_str().unwrap_or("required");
            if client_env == "unsupported" {
                continue;
            }
            let rel = entry["path"]
                .as_str()
                .ok_or_else(|| anyhow!("A modpack file is missing its path."))?;
            let rel_path = safe_relative_path(rel)?;
            let url = entry["downloads"]
                .as_array()
                .and_then(|downloads| {
                    downloads
                        .iter()
                        .filter_map(Value::as_str)
                        .find(|url| safe_modrinth_url(url))
                })
                .ok_or_else(|| anyhow!("A modpack file has no acceptable download URL."))?;
            let bytes = download_capped(client, reqwest::Url::parse(url)?, "file").await?;
            verify_hashes(&entry["hashes"], &bytes)?;
            write_file(&root.join(&rel_path), &bytes)?;
            downloaded += 1;
        }
    }

    let mut overrides = 0usize;
    for prefix in ["overrides/", "client-overrides/"] {
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index)?;
            let name = entry.name().to_string();
            if entry.is_dir() || !name.starts_with(prefix) {
                continue;
            }
            let rel = match safe_relative_path(&name[prefix.len()..]) {
                Ok(rel) => rel,
                Err(_) => continue,
            };
            let dest = root.join(rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = std::fs::File::create(&dest)?;
            std::io::copy(&mut entry, &mut file)?;
            overrides += 1;
        }
    }

    Ok(format!(
        "{pack_name} ({downloaded} downloaded, {overrides} overrides)"
    ))
}

/// Installs a locally picked or dropped `.mrpack` into the profile. The pack's
/// listed files are still downloaded from Modrinth's CDN, but the archive
/// itself comes from disk instead of the network.
pub async fn import_local_modpack(
    client: &reqwest::Client,
    minecraft_dir: &str,
    source: &str,
) -> Result<String> {
    let path = std::path::Path::new(source.trim().trim_matches('"'));
    let metadata = std::fs::metadata(path).map_err(|_| anyhow!("Could not open that modpack file."))?;
    if metadata.len() > MAX_CONTENT_SIZE as u64 {
        bail!("The modpack file exceeds the 100 MB limit.");
    }
    let pack_bytes = std::fs::read(path).map_err(|_| anyhow!("Could not read that modpack file."))?;
    install_modpack_archive(client, minecraft_dir, &pack_bytes)
        .await
        .context("That file is not a valid Modrinth modpack.")
}

fn safe_modrinth_url(url: &str) -> bool {
    reqwest::Url::parse(url)
        .map(|parsed| parsed.scheme() == "https" && parsed.host_str() == Some("cdn.modrinth.com"))
        .unwrap_or(false)
}

/// Validates a modpack-relative path: no absolute paths, no drive letters, no
/// `..`, no backslashes, so it can never escape the profile directory.
fn safe_relative_path(input: &str) -> Result<std::path::PathBuf> {
    let input = input.trim().trim_start_matches('/');
    if input.is_empty() || input.len() > 260 {
        bail!("A modpack contained an empty or oversized file path.");
    }
    if input.contains('\\')
        || input.contains(':')
        || input
            .chars()
            .any(|character| character.is_control() || character == '\0')
    {
        bail!("A modpack contained an unsafe file path.");
    }
    let mut result = std::path::PathBuf::new();
    for component in input.split('/') {
        if component.is_empty() || component == "." || component == ".." {
            bail!("A modpack contained an unsafe file path.");
        }
        result.push(component);
    }
    Ok(result)
}

fn write_file(dest: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = dest.with_file_name(format!(
        "{}.part",
        dest.file_name().unwrap_or_default().to_string_lossy()
    ));
    std::fs::write(&temporary, bytes)?;
    if let Err(error) = std::fs::rename(&temporary, dest) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error).context("Could not place the downloaded file in this profile.");
    }
    Ok(())
}

fn verify_hashes(hashes: &Value, bytes: &[u8]) -> Result<()> {
    if let Some(expected) = hashes["sha512"].as_str() {
        use sha2::{Digest, Sha512};
        let mut hasher = Sha512::new();
        hasher.update(bytes);
        let actual = hex::encode(hasher.finalize().as_slice());
        if !actual.eq_ignore_ascii_case(expected) {
            bail!("A modpack file failed its checksum check.");
        }
        return Ok(());
    }
    if let Some(expected) = hashes["sha1"].as_str() {
        let actual = crate::util::sha1_hex(bytes);
        if !actual.eq_ignore_ascii_case(expected) {
            bail!("A modpack file failed its checksum check.");
        }
    }
    Ok(())
}

async fn download_capped(
    client: &reqwest::Client,
    url: reqwest::Url,
    what: &str,
) -> Result<Vec<u8>> {
    let response = client.get(url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_CONTENT_SIZE as u64)
    {
        bail!("Downloaded {what} exceeds the 100 MB limit.");
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_CONTENT_SIZE {
            bail!("Downloaded {what} exceeds the 100 MB limit.");
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub fn content_directory(project_type: &str) -> Result<&'static str> {
    match project_type {
        "mod" => Ok("mods"),
        "resourcepack" => Ok("resourcepacks"),
        "shader" => Ok("shaderpacks"),
        _ => bail!("Choose Mods, Resource Packs, or Shaders."),
    }
}

fn validate_project_type(project_type: &str) -> Result<()> {
    if matches!(project_type, "mod" | "modpack" | "resourcepack" | "shader") {
        Ok(())
    } else {
        bail!("Choose Mods, Resource Packs, Shaders, or Modpacks.")
    }
}

fn validate_loader(loader: &str) -> Result<()> {
    if !matches!(loader, "fabric" | "quilt" | "forge" | "neoforge") {
        bail!("Choose Fabric, Quilt, Forge or NeoForge.");
    }
    Ok(())
}

fn safe_filename<'a>(filename: &'a str, extension: &str) -> Result<&'a str> {
    let stem = filename
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    let reserved_device_name = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|number| {
                matches!(number, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
            })
        });
    if filename.is_empty()
        || filename.len() > 180
        || !filename.to_ascii_lowercase().ends_with(extension)
        || filename.ends_with([' ', '.'])
        || filename.chars().any(|character| {
            character.is_control()
                || matches!(
                    character,
                    '/' | '\\' | '<' | '>' | ':' | '"' | '|' | '?' | '*'
                )
        })
        || reserved_device_name
    {
        bail!("Modrinth returned an unsafe or invalid filename.");
    }
    Ok(filename)
}

async fn resolve_minecraft_version(client: &reqwest::Client, version: &str) -> Result<String> {
    match version {
        "latest-release" | "latest-snapshot" | "" => {
            let manifest: Value = client
                .get("https://launchermeta.mojang.com/mc/game/version_manifest_v2.json")
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            manifest["latest"][if version == "latest-snapshot" {
                "snapshot"
            } else {
                "release"
            }]
            .as_str()
            .map(str::to_string)
            .ok_or_else(|| anyhow!("Could not determine the selected Minecraft version."))
        }
        other
            if other.len() <= 32
                && other
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b".-_".contains(&byte)) =>
        {
            Ok(other.to_string())
        }
        _ => bail!("Invalid Minecraft version."),
    }
}

#[cfg(test)]
mod tests {
    use super::{content_directory, safe_filename, safe_relative_path};

    #[test]
    fn content_types_use_separate_instance_folders() {
        assert_eq!(content_directory("mod").unwrap(), "mods");
        assert_eq!(content_directory("resourcepack").unwrap(), "resourcepacks");
        assert_eq!(content_directory("shader").unwrap(), "shaderpacks");
        assert!(content_directory("unknown").is_err());
        assert!(content_directory("modpack").is_err());
    }

    #[test]
    fn modpack_paths_cannot_escape_the_profile_directory() {
        assert_eq!(
            safe_relative_path("mods/sodium.jar").unwrap(),
            std::path::PathBuf::from("mods/sodium.jar")
        );
        assert_eq!(
            safe_relative_path("config/pack/config.toml").unwrap(),
            std::path::PathBuf::from("config/pack/config.toml")
        );
        assert_eq!(
            safe_relative_path("/mods/lead.jar").unwrap(),
            std::path::PathBuf::from("mods/lead.jar")
        );
        assert!(safe_relative_path("").is_err());
        assert!(safe_relative_path("../evil.jar").is_err());
        assert!(safe_relative_path("mods/../../evil.jar").is_err());
        assert!(safe_relative_path("mods/./x.jar").is_err());
        assert!(safe_relative_path("C:\\Windows\\evil.jar").is_err());
        assert!(safe_relative_path("mods\\evil.jar").is_err());
        assert!(safe_relative_path("a:b.jar").is_err());
    }

    #[test]
    fn downloaded_content_filenames_are_extension_checked_and_safe() {
        assert_eq!(
            safe_filename("Awesome-Pack.ZIP", ".zip").unwrap(),
            "Awesome-Pack.ZIP"
        );
        assert_eq!(
            safe_filename("My Resource Pack (1.20).zip", ".zip").unwrap(),
            "My Resource Pack (1.20).zip"
        );
        assert_eq!(
            safe_filename("日本語リソースパック.zip", ".zip").unwrap(),
            "日本語リソースパック.zip"
        );
        assert_eq!(safe_filename("CoolMod.jar", ".jar").unwrap(), "CoolMod.jar");
        assert!(safe_filename("Bad/Pack.zip", ".zip").is_err());
        assert!(safe_filename("Bad:Pack.zip", ".zip").is_err());
        assert!(safe_filename("Pack.zip.", ".zip").is_err());
        assert!(safe_filename("CON.zip", ".zip").is_err());
        assert!(safe_filename("../evil.zip", ".zip").is_err());
        assert!(safe_filename("readme.txt", ".zip").is_err());
    }
}
