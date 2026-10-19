use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
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
    if project_type == "mod" {
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
    if project_type == "mod" {
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
    if project_type == "mod" {
        validate_loader(loader)?;
    }
    let minecraft_version = resolve_minecraft_version(client, version).await?;
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

    let response = client.get(parsed_url).send().await?.error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_CONTENT_SIZE as u64)
    {
        bail!("Downloaded file exceeds the 100 MB limit.");
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if bytes.len().saturating_add(chunk.len()) > MAX_CONTENT_SIZE {
            bail!("Downloaded file exceeds the 100 MB limit.");
        }
        bytes.extend_from_slice(&chunk);
    }
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

pub fn content_directory(project_type: &str) -> Result<&'static str> {
    match project_type {
        "mod" => Ok("mods"),
        "resourcepack" => Ok("resourcepacks"),
        "shader" => Ok("shaderpacks"),
        _ => bail!("Choose Mods, Resource Packs, or Shaders."),
    }
}

fn validate_project_type(project_type: &str) -> Result<()> {
    content_directory(project_type).map(|_| ())
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
    use super::{content_directory, safe_filename};

    #[test]
    fn content_types_use_separate_instance_folders() {
        assert_eq!(content_directory("mod").unwrap(), "mods");
        assert_eq!(content_directory("resourcepack").unwrap(), "resourcepacks");
        assert_eq!(content_directory("shader").unwrap(), "shaderpacks");
        assert!(content_directory("unknown").is_err());
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
