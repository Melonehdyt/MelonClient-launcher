use crate::auth::McProfile;
use crate::settings::Settings;
use crate::util::{self, Features};
use anyhow::{anyhow, bail, Context, Result};
use futures_util::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use sysinfo::{Pid, System};
use tauri::{AppHandle, Emitter, Manager};

pub const VERSION_MANIFEST: &str =
    "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
pub const FABRIC_META: &str = "https://meta.fabricmc.net/v2/versions/loader";
const QUILT_META: &str = "https://meta.quiltmc.org/v3/versions/loader";
const FORGE_PROMOTIONS: &str =
    "https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json";
const NEOFORGE_METADATA: &str =
    "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml";
const RESOURCES: &str = "https://resources.download.minecraft.net";

#[derive(Serialize, Clone)]
struct Progress {
    title: String,
    message: String,
    current: u64,
    total: u64,
}

fn progress(app: &AppHandle, title: &str, message: &str, current: u64, total: u64) {
    let _ = app.emit(
        "progress",
        Progress {
            title: title.to_string(),
            message: message.to_string(),
            current,
            total,
        },
    );
}

pub struct Resolved {
    pub mc: String,
    pub loader: String,
    pub loader_type: String,
    pub vanilla: Value,
    pub profile: Value,
}

pub struct InstallPaths {
    pub classpath: Vec<PathBuf>,
    pub natives_dir: PathBuf,
    pub profile: Value,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct RunningGame {
    pub pid: u32,
    pub profile_id: String,
    pub profile_name: String,
    pub version: String,
    pub loader: String,
    #[serde(default)]
    marker: String,
}

fn running_games_path(app: &AppHandle) -> Result<PathBuf> {
    let directory = app.path().app_config_dir()?;
    std::fs::create_dir_all(&directory)?;
    Ok(directory.join("running-games.json"))
}

fn read_tracked_games(app: &AppHandle) -> Result<Vec<RunningGame>> {
    let path = running_games_path(app)?;
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(serde_json::from_str(&contents)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

fn write_tracked_games(app: &AppHandle, games: &[RunningGame]) -> Result<()> {
    let path = running_games_path(app)?;
    std::fs::write(path, serde_json::to_vec_pretty(games)?).map_err(Into::into)
}

fn process_arguments(process: &sysinfo::Process) -> Vec<String> {
    process.cmd().iter().cloned().collect()
}

fn minecraft_loader(arguments: &[String]) -> Option<&'static str> {
    let arguments = arguments.join(" ");
    if arguments.contains("net.fabricmc.loader.") {
        Some("fabric")
    } else if arguments.contains("org.quiltmc.loader.") {
        Some("quilt")
    } else if arguments.contains("net.neoforged.") {
        Some("neoforge")
    } else if arguments.contains("net.minecraftforge.")
        || arguments.contains("cpw.mods.bootstraplauncher.BootstrapLauncher")
    {
        Some("forge")
    } else {
        None
    }
}

fn is_minecraft_process(arguments: &[String]) -> bool {
    let arguments = arguments.join(" ");
    arguments.contains("net.minecraft.client.main.Main")
        || arguments.contains("net.fabricmc.loader.impl.launch.knot.KnotClient")
        || arguments.contains("org.quiltmc.loader.impl.launch.knot.KnotClient")
        || arguments.contains("cpw.mods.bootstraplauncher.BootstrapLauncher")
        || arguments.contains("net.minecraftforge.")
        || arguments.contains("net.neoforged.")
}

fn minecraft_version(arguments: &[String]) -> String {
    arguments
        .windows(2)
        .find(|pair| pair[0] == "--version")
        .map(|pair| pair[1].clone())
        .unwrap_or_else(|| "unknown version".to_string())
}

pub fn list_running_games(app: &AppHandle) -> Result<Vec<RunningGame>> {
    let mut system = System::new_all();
    system.refresh_all();
    let mut tracked = read_tracked_games(app)?;
    let tracked_count = tracked.len();
    let mut active = Vec::new();

    for (pid, process) in system.processes() {
        let pid = pid.as_u32();
        let arguments = process_arguments(process);
        if let Some(record) = tracked.iter().find(|record| {
            record.pid == pid
                && !record.marker.is_empty()
                && arguments.iter().any(|argument| {
                    argument == &format!("-Dmelonclient.instance={}", record.marker)
                })
        }) {
            active.push(record.clone());
        } else if is_minecraft_process(&arguments) {
            active.push(RunningGame {
                pid,
                profile_id: format!("external-{pid}"),
                profile_name: "Minecraft (detected)".to_string(),
                version: minecraft_version(&arguments),
                loader: minecraft_loader(&arguments)
                    .unwrap_or("minecraft")
                    .to_string(),
                marker: String::new(),
            });
        }
    }

    tracked.retain(|record| {
        active
            .iter()
            .any(|game| game.pid == record.pid && game.marker == record.marker)
    });
    if tracked.len() != tracked_count {
        write_tracked_games(app, &tracked)?;
    }
    Ok(active)
}

pub fn stop_recovered_game(app: &AppHandle, pid: u32) -> Result<RunningGame> {
    let game = list_running_games(app)?
        .into_iter()
        .find(|game| game.pid == pid)
        .ok_or_else(|| anyhow!("That Minecraft instance is no longer running."))?;
    let mut system = System::new_all();
    system.refresh_all();
    let process = system
        .process(Pid::from_u32(pid))
        .ok_or_else(|| anyhow!("That Minecraft instance is no longer running."))?;
    let arguments = process_arguments(process);
    let verified = if game.marker.is_empty() {
        is_minecraft_process(&arguments)
    } else {
        arguments
            .iter()
            .any(|argument| argument == &format!("-Dmelonclient.instance={}", game.marker))
    };
    if !verified {
        bail!("The selected process no longer matches this Minecraft instance.");
    }
    if !process.kill() {
        bail!("Could not stop the selected Minecraft instance.");
    }
    Ok(game)
}

fn forget_tracked_game(app: &AppHandle, pid: u32) -> Result<()> {
    let mut games = read_tracked_games(app)?;
    games.retain(|game| game.pid != pid);
    write_tracked_games(app, &games)
}

pub async fn loader_versions(
    client: &reqwest::Client,
    minecraft_version: &str,
    loader_type: &str,
) -> Result<Vec<String>> {
    validate_loader_type(loader_type)?;
    match loader_type {
        "fabric" | "quilt" => {
            let url = if loader_type == "fabric" {
                format!("{FABRIC_META}/{minecraft_version}")
            } else {
                format!("{QUILT_META}/{minecraft_version}")
            };
            let versions: Value = client
                .get(url)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            Ok(versions
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| entry["loader"]["version"].as_str().map(str::to_string))
                .collect())
        }
        "forge" => {
            let promotions: Value = client
                .get(FORGE_PROMOTIONS)
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            let promos = promotions
                .get("promos")
                .and_then(Value::as_object)
                .ok_or_else(|| anyhow!("Forge returned an invalid version list"))?;
            let recommended = promos
                .get(&format!("{minecraft_version}-recommended"))
                .and_then(Value::as_str);
            let latest = promos
                .get(&format!("{minecraft_version}-latest"))
                .and_then(Value::as_str);
            let mut versions = Vec::new();
            for version in [recommended, latest].into_iter().flatten() {
                if !versions.iter().any(|current| current == version) {
                    versions.push(version.to_string());
                }
            }
            Ok(versions)
        }
        "neoforge" => {
            let response = client
                .get(NEOFORGE_METADATA)
                .send()
                .await?
                .error_for_status()?
                .text()
                .await?;
            let prefix = neoforge_prefix(minecraft_version)?;
            let mut versions = response
                .split("<version>")
                .skip(1)
                .filter_map(|part| part.split("</version>").next())
                .filter(|version| version.starts_with(&prefix))
                .map(str::to_string)
                .collect::<Vec<_>>();
            versions.sort_by(|left, right| compare_versions(right, left));
            versions.dedup();
            versions.truncate(40);
            Ok(versions)
        }
        _ => unreachable!(),
    }
}

fn validate_loader_type(loader_type: &str) -> Result<()> {
    if matches!(loader_type, "fabric" | "quilt" | "forge" | "neoforge") {
        Ok(())
    } else {
        bail!("Choose Fabric, Quilt, Forge or NeoForge.")
    }
}

fn neoforge_prefix(minecraft_version: &str) -> Result<String> {
    let mut parts = minecraft_version.split('.');
    let first = parts.next().unwrap_or_default();
    if first != "1" {
        let second = parts
            .next()
            .ok_or_else(|| anyhow!("Invalid Minecraft version for NeoForge."))?;
        return Ok(format!("{first}.{second}."));
    }
    let minor = parts
        .next()
        .ok_or_else(|| anyhow!("Invalid Minecraft version for NeoForge."))?;
    let patch = parts.next().unwrap_or("0");
    Ok(format!("{minor}.{patch}."))
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let left_parts = left.split(['.', '-', '+']).collect::<Vec<_>>();
    let right_parts = right.split(['.', '-', '+']).collect::<Vec<_>>();
    for (left, right) in left_parts.iter().zip(right_parts.iter()) {
        let ordering = match (left.parse::<u64>(), right.parse::<u64>()) {
            (Ok(left), Ok(right)) => left.cmp(&right),
            _ => left.cmp(right),
        };
        if ordering != std::cmp::Ordering::Equal {
            return ordering;
        }
    }
    left_parts.len().cmp(&right_parts.len())
}

#[cfg(test)]
mod tests {
    use super::{
        compare_versions, is_minecraft_process, java_download_target, minecraft_loader,
        minecraft_version, neoforge_prefix,
    };

    #[test]
    fn java_download_target_matches_supported_host_platform() {
        let (os, architecture, archive) = java_download_target().unwrap();
        assert!(matches!(os, "windows" | "mac" | "linux"));
        assert!(matches!(architecture, "x64" | "aarch64"));
        assert_eq!(archive, if os == "windows" { "zip" } else { "tar.gz" });
    }

    #[test]
    fn neoforge_version_prefix_matches_minecraft_release() {
        assert_eq!(neoforge_prefix("1.20.1").unwrap(), "20.1.");
        assert_eq!(neoforge_prefix("1.21").unwrap(), "21.0.");
        assert_eq!(neoforge_prefix("26.1.3").unwrap(), "26.1.");
    }

    #[test]
    fn loader_versions_sort_numerically() {
        assert!(compare_versions("21.1.10", "21.1.9").is_gt());
    }

    #[test]
    fn detects_minecraft_process_and_reads_loader_and_version() {
        let arguments = vec![
            "java".to_string(),
            "net.fabricmc.loader.impl.launch.knot.KnotClient".to_string(),
            "--version".to_string(),
            "1.21.4-fabric".to_string(),
        ];
        assert!(is_minecraft_process(&arguments));
        assert_eq!(minecraft_loader(&arguments), Some("fabric"));
        assert_eq!(minecraft_version(&arguments), "1.21.4-fabric");
        assert!(!is_minecraft_process(&[
            "java".to_string(),
            "SomeApp".to_string()
        ]));
    }
}

/// Resolves the selected Minecraft version and its mod-loader profile/version.
pub async fn resolve_version(client: &reqwest::Client, settings: &Settings) -> Result<Resolved> {
    let manifest: Value = client
        .get(VERSION_MANIFEST)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let id = match settings.version.as_str() {
        "" | "latest-release" => manifest["latest"]["release"]
            .as_str()
            .ok_or_else(|| anyhow!("manifest has no latest release"))?
            .to_string(),
        "latest-snapshot" => manifest["latest"]["snapshot"]
            .as_str()
            .ok_or_else(|| anyhow!("manifest has no latest snapshot"))?
            .to_string(),
        other => other.to_string(),
    };

    let versions = manifest["versions"]
        .as_array()
        .ok_or_else(|| anyhow!("malformed version manifest"))?;
    let entry = versions
        .iter()
        .find(|candidate| candidate["id"].as_str() == Some(id.as_str()))
        .ok_or_else(|| anyhow!("Minecraft version '{id}' was not found"))?;
    let url = entry["url"]
        .as_str()
        .ok_or_else(|| anyhow!("version '{id}' has no metadata url"))?;
    let vanilla: Value = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let loader_type = if settings.mod_loader.trim().is_empty() {
        "fabric"
    } else {
        settings.mod_loader.trim()
    };
    validate_loader_type(loader_type)?;
    let versions = loader_versions(client, &id, loader_type).await?;
    let loader = match settings.loader.trim() {
        "" => versions
            .first()
            .cloned()
            .ok_or_else(|| anyhow!("No {loader_type} version is available for Minecraft {id}"))?,
        selected if versions.iter().any(|version| version == selected) => selected.to_string(),
        _ => bail!(
            "The selected {loader_type} version is not available for Minecraft {id}. Refresh its version list and choose a supported version."
        ),
    };

    let profile = match loader_type {
        "fabric" => {
            client
                .get(format!("{FABRIC_META}/{id}/{loader}/profile/json"))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?
        }
        "quilt" => {
            client
                .get(format!("{QUILT_META}/{id}/{loader}/profile/json"))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?
        }
        "forge" | "neoforge" => Value::Null,
        _ => unreachable!(),
    };
    if matches!(loader_type, "fabric" | "quilt")
        && (profile["libraries"].as_array().is_none() || profile["mainClass"].as_str().is_none())
    {
        bail!("{loader_type} returned an invalid launch profile for Minecraft {id}");
    }

    Ok(Resolved {
        mc: id,
        loader,
        loader_type: loader_type.to_string(),
        vanilla,
        profile,
    })
}

pub async fn install(
    client: &reqwest::Client,
    app: &AppHandle,
    settings: &Settings,
    resolved: &Resolved,
) -> Result<InstallPaths> {
    let root = PathBuf::from(&settings.minecraft_dir);
    let libraries_dir = root.join("libraries");
    let assets_dir = root.join("assets");
    let versions_dir = root.join("versions").join(&resolved.mc);
    let natives_dir = versions_dir.join("natives");

    tokio::fs::create_dir_all(&libraries_dir).await?;
    tokio::fs::create_dir_all(&natives_dir).await?;
    tokio::fs::create_dir_all(assets_dir.join("indexes")).await?;

    let profile = match resolved.loader_type.as_str() {
        "forge" | "neoforge" => install_external_loader(client, app, settings, resolved).await?,
        _ => resolved.profile.clone(),
    };

    // 1. Client jar.
    let client_jar = versions_dir.join(format!("{}.jar", resolved.mc));
    let client_download = &resolved.vanilla["downloads"]["client"];
    let client_url = client_download["url"]
        .as_str()
        .ok_or_else(|| anyhow!("client download url missing"))?;
    progress(app, "Installing", "Downloading Minecraft client", 0, 1);
    util::ensure_file(
        client,
        client_url,
        &client_jar,
        client_download["sha1"].as_str(),
    )
    .await?;

    // 2. Libraries (+ natives extraction).
    let mut classpath = collect_classpath(
        client,
        app,
        &profile,
        &resolved.vanilla,
        &libraries_dir,
        &natives_dir,
    )
    .await?;
    classpath.push(client_jar);

    // 3. Assets.
    install_assets(client, app, &resolved.vanilla, &assets_dir).await?;

    progress(app, "Installing", "Ready", 1, 1);
    Ok(InstallPaths {
        classpath,
        natives_dir,
        profile,
    })
}

async fn install_external_loader(
    client: &reqwest::Client,
    app: &AppHandle,
    settings: &Settings,
    resolved: &Resolved,
) -> Result<Value> {
    let root = PathBuf::from(&settings.minecraft_dir);
    let profile_id = format!(
        "{}-{}-{}",
        resolved.mc, resolved.loader_type, resolved.loader
    );
    let versions_dir = root.join("versions");
    let expected_profile = versions_dir
        .join(&profile_id)
        .join(format!("{profile_id}.json"));
    if expected_profile.is_file() {
        return Ok(serde_json::from_slice(
            &tokio::fs::read(expected_profile).await?,
        )?);
    }

    let (url, label) = if resolved.loader_type == "forge" {
        (
            format!(
                "https://maven.minecraftforge.net/net/minecraftforge/forge/{}-{}/forge-{}-{}-installer.jar",
                resolved.mc, resolved.loader, resolved.mc, resolved.loader
            ),
            "Forge",
        )
    } else {
        (
            format!(
                "https://maven.neoforged.net/releases/net/neoforged/neoforge/{}/neoforge-{}-installer.jar",
                resolved.loader, resolved.loader
            ),
            "NeoForge",
        )
    };
    let installer_path = root.join("launcher-cache").join(format!(
        "{}-{}-installer.jar",
        resolved.loader_type, resolved.loader
    ));
    let _ = app.emit(
        "log",
        format!("Downloading {label} {} installer", resolved.loader),
    );
    util::ensure_file(client, &url, &installer_path, None).await?;

    let required_java = required_java_major(&resolved.vanilla);
    let java = resolve_java(client, app, settings, required_java).await?;
    let output = tokio::process::Command::new(&java)
        .arg("-jar")
        .arg(&installer_path)
        .arg("--installClient")
        .arg(&root)
        .output()
        .await
        .with_context(|| format!("Could not start the {label} installer"))?;
    if !output.status.success() {
        let details = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        bail!(
            "{label} installer exited with {}.\n{}{}",
            output.status,
            stdout.trim(),
            details.trim()
        );
    }

    if expected_profile.is_file() {
        return Ok(serde_json::from_slice(
            &tokio::fs::read(expected_profile).await?,
        )?);
    }

    let mut entries = tokio::fs::read_dir(&versions_dir).await?;
    while let Some(entry) = entries.next_entry().await? {
        if !entry.file_type().await?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(&resolved.mc)
            || !name.contains(&resolved.loader_type)
            || !name.contains(&resolved.loader)
        {
            continue;
        }
        let candidate = entry.path().join(format!("{name}.json"));
        if candidate.is_file() {
            let profile: Value = serde_json::from_slice(&tokio::fs::read(candidate).await?)?;
            if profile["mainClass"].as_str().is_some() && profile["libraries"].as_array().is_some()
            {
                return Ok(profile);
            }
        }
    }
    bail!("{label} installer completed, but its game launch profile was not found")
}

struct DownloadJob {
    url: String,
    dest: PathBuf,
    sha1: Option<String>,
    on_classpath: bool,
    extract: bool,
}

async fn collect_classpath(
    client: &reqwest::Client,
    app: &AppHandle,
    fabric: &Value,
    vanilla: &Value,
    libraries_dir: &Path,
    natives_dir: &Path,
) -> Result<Vec<PathBuf>> {
    let feats = Features::default();
    let mut jobs: Vec<DownloadJob> = Vec::new();

    let vanilla_libs = vanilla["libraries"].as_array().cloned().unwrap_or_default();
    let fabric_libs = fabric["libraries"].as_array().cloned().unwrap_or_default();

    for library in vanilla_libs.iter().chain(fabric_libs.iter()) {
        if let Some(rules) = library.get("rules").and_then(Value::as_array) {
            if !util::rules_pass(rules, &feats) {
                continue;
            }
        }
        let has_extract = library.get("extract").is_some();

        if let Some(downloads) = library.get("downloads") {
            if let Some(artifact) = downloads.get("artifact") {
                if let (Some(url), Some(relative)) =
                    (artifact["url"].as_str(), artifact["path"].as_str())
                {
                    jobs.push(DownloadJob {
                        url: url.to_string(),
                        dest: libraries_dir.join(relative),
                        sha1: artifact["sha1"].as_str().map(str::to_string),
                        on_classpath: !has_extract,
                        extract: has_extract,
                    });
                }
            }
            if let (Some(natives), Some(classifiers)) = (
                library.get("natives").and_then(Value::as_object),
                downloads.get("classifiers").and_then(Value::as_object),
            ) {
                if let Some(key) = natives.get(util::os_name()).and_then(Value::as_str) {
                    let key = key.replace(
                        "${arch}",
                        if util::os_arch() == "x86_64" {
                            "64"
                        } else {
                            "32"
                        },
                    );
                    if let Some(classifier) = classifiers.get(&key) {
                        if let Some(url) = classifier["url"].as_str() {
                            jobs.push(DownloadJob {
                                url: url.to_string(),
                                dest: libraries_dir
                                    .join(classifier["path"].as_str().unwrap_or_default()),
                                sha1: classifier["sha1"].as_str().map(str::to_string),
                                on_classpath: false,
                                extract: true,
                            });
                        }
                    }
                }
            }
        } else if let (Some(name), Some(base)) = (
            library.get("name").and_then(Value::as_str),
            library.get("url").and_then(Value::as_str),
        ) {
            let relative = util::maven_rel_path(name);
            jobs.push(DownloadJob {
                url: format!("{base}{relative}"),
                dest: libraries_dir.join(&relative),
                sha1: library
                    .get("sha1")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                on_classpath: !has_extract,
                extract: has_extract,
            });
        }
    }

    let total = jobs.len() as u64;
    let mut done = 0u64;
    progress(app, "Installing", "Downloading libraries", 0, total);
    for chunk in jobs.chunks(16) {
        let futures = chunk
            .iter()
            .map(|job| util::ensure_file(client, &job.url, &job.dest, job.sha1.as_deref()));
        for result in join_all(futures).await {
            result?;
        }
        done += chunk.len() as u64;
        progress(app, "Installing", "Downloading libraries", done, total);
    }

    let mut classpath = Vec::new();
    for job in &jobs {
        if job.on_classpath {
            classpath.push(job.dest.clone());
        }
        if job.extract {
            util::extract_jar(&job.dest, natives_dir, &["META-INF/"])?;
        }
    }
    Ok(classpath)
}

async fn install_assets(
    client: &reqwest::Client,
    app: &AppHandle,
    vanilla: &Value,
    assets_dir: &Path,
) -> Result<()> {
    let index = &vanilla["assetIndex"];
    let id = index["id"]
        .as_str()
        .ok_or_else(|| anyhow!("asset index id missing"))?;
    let url = index["url"]
        .as_str()
        .ok_or_else(|| anyhow!("asset index url missing"))?;

    let index_path = assets_dir.join("indexes").join(format!("{id}.json"));
    util::ensure_file(client, url, &index_path, index["sha1"].as_str()).await?;

    let raw = tokio::fs::read_to_string(&index_path).await?;
    let parsed: Value = serde_json::from_str(&raw)?;
    let objects = parsed["objects"]
        .as_object()
        .ok_or_else(|| anyhow!("asset index has no objects"))?;

    let objects_dir = assets_dir.join("objects");
    let mut jobs: Vec<(String, PathBuf)> = Vec::new();
    for object in objects.values() {
        let hash = object["hash"].as_str().unwrap_or_default();
        if hash.len() < 2 {
            continue;
        }
        let sub = &hash[0..2];
        let dest = objects_dir.join(sub).join(hash);
        if dest.exists() {
            continue;
        }
        jobs.push((format!("{RESOURCES}/{sub}/{hash}"), dest));
    }

    let total = jobs.len() as u64;
    let mut done = 0u64;
    progress(app, "Installing", "Downloading assets", 0, total);
    for chunk in jobs.chunks(32) {
        let futures = chunk
            .iter()
            .map(|(url, dest)| util::ensure_file(client, url, dest, None));
        for result in join_all(futures).await {
            result?;
        }
        done += chunk.len() as u64;
        progress(app, "Installing", "Downloading assets", done, total);
    }
    Ok(())
}

fn collect_args(args: &Value, feats: &Features, vars: &HashMap<&str, String>) -> Vec<String> {
    let mut out = Vec::new();
    let array = match args.as_array() {
        Some(array) => array,
        None => return out,
    };
    for item in array {
        if let Some(text) = item.as_str() {
            out.push(util::substitute(text, vars));
            continue;
        }
        let object = match item.as_object() {
            Some(object) => object,
            None => continue,
        };
        if let Some(rules) = object.get("rules").and_then(Value::as_array) {
            if !util::rules_pass(rules, feats) {
                continue;
            }
        }
        match object.get("value") {
            Some(Value::String(text)) => out.push(util::substitute(text, vars)),
            Some(Value::Array(list)) => {
                for entry in list {
                    if let Some(text) = entry.as_str() {
                        out.push(util::substitute(text, vars));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// Reads the Java major version required by a vanilla version manifest entry.
fn required_java_major(vanilla: &Value) -> u32 {
    vanilla
        .get("javaVersion")
        .and_then(|value| value.get("majorVersion"))
        .and_then(Value::as_u64)
        .map(|value| value as u32)
        .unwrap_or(8)
}

/// Gathers every plausible `java` executable on the system, most-specific first.
fn java_candidates(settings: &Settings) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut push = |path: PathBuf| {
        if path.is_file() && !out.iter().any(|existing| existing == &path) {
            out.push(path);
        }
    };

    let configured = settings.java_path.trim();
    if !configured.is_empty() {
        let path = PathBuf::from(configured);
        if path.is_dir() {
            push(path.join("bin").join(util::java_exe_name()));
        } else {
            push(path);
        }
    }

    if let Ok(home) = std::env::var("JAVA_HOME") {
        if !home.trim().is_empty() {
            push(PathBuf::from(home).join("bin").join(util::java_exe_name()));
        }
    }

    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            push(directory.join(util::java_exe_name()));
        }
    }

    let mut bases: Vec<PathBuf> = Vec::new();
    #[cfg(windows)]
    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Ok(base) = std::env::var(var) {
            let base = PathBuf::from(base);
            for name in [
                "Eclipse Adoptium",
                "Java",
                "Microsoft",
                "Zulu",
                "Amazon Corretto",
                "BellSoft",
                "Semeru",
                "RedHat",
            ] {
                bases.push(base.join(name));
            }
        }
    }
    #[cfg(windows)]
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let programs = PathBuf::from(&local).join("Programs");
        for name in [
            "Eclipse Adoptium",
            "Microsoft",
            "Java",
            "Zulu",
            "Amazon Corretto",
            "AdoptOpenJDK",
        ] {
            bases.push(programs.join(name));
        }
    }
    #[cfg(windows)]
    if let Ok(appdata) = std::env::var("APPDATA") {
        bases.push(PathBuf::from(appdata).join(".minecraft").join("runtime"));
    }
    #[cfg(target_os = "linux")]
    bases.push(PathBuf::from("/usr/lib/jvm"));
    #[cfg(target_os = "macos")]
    bases.push(PathBuf::from("/Library/Java/JavaVirtualMachines"));
    if !settings.minecraft_dir.trim().is_empty() {
        bases.push(PathBuf::from(&settings.minecraft_dir).join("runtime"));
    }

    for base in &bases {
        push(base.join("bin").join(util::java_exe_name()));
        #[cfg(target_os = "macos")]
        push(
            base.join("Contents")
                .join("Home")
                .join("bin")
                .join(util::java_exe_name()),
        );
        if let Ok(entries) = std::fs::read_dir(base) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if let Some(exe) = util::find_java_exe(&path, 2) {
                        push(exe);
                    }
                    #[cfg(target_os = "macos")]
                    push(
                        path.join("Contents")
                            .join("Home")
                            .join("bin")
                            .join(util::java_exe_name()),
                    );
                }
            }
        }
    }

    // Microsoft Store Minecraft launcher ships its own bundled runtimes.
    #[cfg(windows)]
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let packages = PathBuf::from(local).join("Packages");
        if let Ok(entries) = std::fs::read_dir(&packages) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("Microsoft.4297127D64EC6") {
                    let runtime = entry
                        .path()
                        .join("LocalCache")
                        .join("Local")
                        .join("runtime");
                    if let Ok(dirs) = std::fs::read_dir(&runtime) {
                        for dir in dirs.flatten() {
                            if let Some(exe) = util::find_java_exe(&dir.path(), 3) {
                                push(exe);
                            }
                        }
                    }
                }
            }
        }
    }

    out
}

fn runtime_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_local_data_dir()?.join("runtimes"))
}

fn java_download_target() -> Result<(&'static str, &'static str, &'static str)> {
    let os = match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "mac",
        "linux" => "linux",
        _ => bail!("Automatic Java installation is not supported on this operating system."),
    };
    let architecture = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "aarch64",
        _ => bail!("Automatic Java installation is not supported on this processor architecture."),
    };
    let archive = if os == "windows" { "zip" } else { "tar.gz" };
    Ok((os, architecture, archive))
}

async fn download_java(client: &reqwest::Client, app: &AppHandle, major: u32) -> Result<PathBuf> {
    let (os, architecture, archive_extension) = java_download_target()?;
    let url = format!(
        "https://api.adoptium.net/v3/assets/latest/{major}/hotspot?architecture={architecture}&image_type=jre&os={os}&vendor=eclipse"
    );
    let assets: Value = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let entry = assets
        .as_array()
        .and_then(|list| list.first())
        .ok_or_else(|| anyhow!("No Java {major} runtime is available for download"))?;
    let package = &entry["binary"]["package"];
    let link = package["link"]
        .as_str()
        .ok_or_else(|| anyhow!("Java runtime download link missing"))?;
    let checksum = package["checksum"].as_str();

    let root = runtime_dir(app)?.join(format!("temurin-{major}"));
    if let Some(exe) = util::find_java_exe(&root, 3) {
        return Ok(exe);
    }
    tokio::fs::create_dir_all(&root).await?;

    progress(
        app,
        "Java",
        &format!("Downloading Java {major} runtime"),
        0,
        1,
    );
    let bytes = client
        .get(link)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    if let Some(expected) = checksum {
        let actual = util::sha256_hex(&bytes);
        if !actual.eq_ignore_ascii_case(expected) {
            bail!("Downloaded Java runtime failed its checksum");
        }
    }

    let archive = root.join(format!("runtime.{archive_extension}"));
    tokio::fs::write(&archive, &bytes).await?;
    progress(app, "Java", "Extracting Java runtime", 0, 1);
    let extract_dir = root.join("jre");
    let archive_clone = archive.clone();
    let extract_clone = extract_dir.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        if archive_extension == "zip" {
            util::extract_zip(&archive_clone, &extract_clone)?;
        } else {
            let file = std::fs::File::open(&archive_clone)?;
            let decoder = flate2::read::GzDecoder::new(file);
            let mut tar = tar::Archive::new(decoder);
            tar.unpack(&extract_clone)?;
        }
        Ok(())
    })
    .await??;
    let _ = tokio::fs::remove_file(&archive).await;

    util::find_java_exe(&extract_dir, 3)
        .ok_or_else(|| anyhow!("Downloaded Java runtime did not contain java"))
}

/// Picks a Java executable matching `required`, auto-downloading one when none
/// of the installed runtimes are new enough.
async fn resolve_java(
    client: &reqwest::Client,
    app: &AppHandle,
    settings: &Settings,
    required: u32,
) -> Result<PathBuf> {
    let mut best: Option<(u32, PathBuf)> = None;
    for path in java_candidates(settings) {
        let major = match util::java_major_of(&path) {
            Some(major) => major,
            None => continue,
        };
        if major == required {
            return Ok(path);
        }
        if major >= required {
            let replace = match &best {
                Some((best_major, _)) => *best_major > major,
                None => true,
            };
            if replace {
                best = Some((major, path));
            }
        }
    }

    if let Some((major, path)) = best {
        let _ = app.emit(
            "log",
            format!("Using Java {major} (this version needs Java {required})."),
        );
        return Ok(path);
    }

    let _ = app.emit(
        "log",
        format!("Java {required} is not installed. Downloading a runtime..."),
    );
    download_java(client, app, required).await
}

pub async fn launch(
    client: &reqwest::Client,
    app: &AppHandle,
    settings: &Settings,
    resolved: &Resolved,
    profile: &McProfile,
    paths: &InstallPaths,
    game_processes: std::sync::Arc<
        tokio::sync::Mutex<std::collections::HashMap<u32, tokio::process::Child>>,
    >,
    stopped_processes: std::sync::Arc<tokio::sync::Mutex<std::collections::HashSet<u32>>>,
    profile_id: String,
    profile_name: String,
) -> Result<u32> {
    let root = PathBuf::from(&settings.minecraft_dir);
    let separator = if cfg!(windows) { ";" } else { ":" };
    let classpath = paths
        .classpath
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(separator);

    let feats = Features::default();
    let mut vars: HashMap<&str, String> = HashMap::new();
    vars.insert("auth_player_name", profile.name.clone());
    vars.insert(
        "version_name",
        format!(
            "{}-{}-{}",
            resolved.mc, resolved.loader_type, resolved.loader
        ),
    );
    vars.insert("game_directory", root.to_string_lossy().to_string());
    vars.insert(
        "assets_root",
        root.join("assets").to_string_lossy().to_string(),
    );
    vars.insert(
        "assets_index_name",
        resolved.vanilla["assetIndex"]["id"]
            .as_str()
            .unwrap_or("")
            .to_string(),
    );
    vars.insert("auth_uuid", profile.uuid.replace('-', ""));
    vars.insert("auth_access_token", profile.access_token.clone());
    vars.insert("auth_session", format!("token:{}", profile.access_token));
    vars.insert("clientid", String::new());
    vars.insert("auth_xuid", String::new());
    vars.insert("user_type", profile.user_type.clone());
    vars.insert("version_type", "release".to_string());
    vars.insert(
        "natives_directory",
        paths.natives_dir.to_string_lossy().to_string(),
    );
    vars.insert("launcher_name", "melonclient-launcher".to_string());
    vars.insert("launcher_version", env!("CARGO_PKG_VERSION").to_string());
    vars.insert("classpath", classpath.clone());
    vars.insert("classpath_separator", separator.to_string());
    vars.insert(
        "library_directory",
        root.join("libraries").to_string_lossy().to_string(),
    );
    vars.insert("user_properties", "{}".to_string());
    vars.insert("resolution_width", "854".to_string());
    vars.insert("resolution_height", "480".to_string());

    let mut jvm_args: Vec<String> = vec![
        format!("-Xmx{}M", settings.memory_mb.max(1024)),
        // jemalloc (LWJGL's default allocator) crashes with
        // EXCEPTION_ACCESS_VIOLATION during shader compilation on some
        // Windows/NVIDIA setups. Forcing the system allocator avoids it.
        "-Dorg.lwjgl.system.allocator=system".to_string(),
    ];
    let marker = format!("{:032x}", rand::random::<u128>());
    jvm_args.push(format!("-Dmelonclient.instance={marker}"));
    for extra in settings.extra_jvm_args.split_whitespace() {
        if !extra.is_empty() {
            jvm_args.push(extra.to_string());
        }
    }

    let mut game_args: Vec<String> = Vec::new();
    if let Some(arguments) = resolved.vanilla.get("arguments") {
        jvm_args.extend(collect_args(&arguments["jvm"], &feats, &vars));
        game_args.extend(collect_args(&arguments["game"], &feats, &vars));
    } else if let Some(legacy) = resolved.vanilla["minecraftArguments"].as_str() {
        for token in legacy.split_whitespace() {
            game_args.push(util::substitute(token, &vars));
        }
    }
    if let Some(arguments) = paths.profile.get("arguments") {
        jvm_args.extend(collect_args(&arguments["jvm"], &feats, &vars));
        game_args.extend(collect_args(&arguments["game"], &feats, &vars));
    } else if let Some(legacy) = paths.profile["minecraftArguments"].as_str() {
        for token in legacy.split_whitespace() {
            game_args.push(util::substitute(token, &vars));
        }
    }

    if !jvm_args
        .iter()
        .any(|arg| arg == "-cp" || arg == "-classpath")
    {
        jvm_args.push("-cp".to_string());
        jvm_args.push(classpath.clone());
    }

    let main_class = paths.profile["mainClass"]
        .as_str()
        .ok_or_else(|| anyhow!("{} launch class is missing", resolved.loader_type))?
        .to_string();

    let required = required_java_major(&resolved.vanilla);
    let java = resolve_java(client, app, settings, required).await?;
    let java_label = java.to_string_lossy().to_string();
    let _ = app.emit("log", format!("Launching with {java_label}"));

    let mut command = tokio::process::Command::new(&java);
    command.args(&jvm_args);
    command.arg(&main_class);
    command.args(&game_args);
    command.current_dir(&root);
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    #[cfg(windows)]
    command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

    let mut child = command
        .spawn()
        .with_context(|| format!("Failed to start Java ({java_label})."))?;

    let pid = child
        .id()
        .ok_or_else(|| anyhow!("Could not determine the Minecraft process id."))?;
    let record = RunningGame {
        pid,
        profile_id: profile_id.clone(),
        profile_name: profile_name.clone(),
        version: resolved.mc.clone(),
        loader: resolved.loader_type.clone(),
        marker,
    };
    match read_tracked_games(app) {
        Ok(mut games) => {
            games.retain(|game| game.pid != pid);
            games.push(record);
            if let Err(error) = write_tracked_games(app, &games) {
                let _ = app.emit(
                    "log",
                    format!("Could not persist Minecraft process tracking: {error}"),
                );
            }
        }
        Err(error) => {
            let _ = app.emit(
                "log",
                format!("Could not read Minecraft process tracking: {error}"),
            );
        }
    }
    if let Some(stdout) = child.stdout.take() {
        let handle = app.clone();
        tokio::spawn(async move { read_lines(stdout, handle, pid).await });
    }
    if let Some(stderr) = child.stderr.take() {
        let handle = app.clone();
        tokio::spawn(async move { read_lines(stderr, handle, pid).await });
    }

    game_processes.lock().await.insert(pid, child);
    let _ = app.emit(
        "game-start",
        serde_json::json!({
            "pid": pid,
            "profile_id": profile_id,
            "profile_name": profile_name,
            "version": resolved.mc,
            "loader": resolved.loader_type,
        }),
    );

    let app = app.clone();
    tokio::spawn(async move {
        loop {
            let result = {
                let mut processes = game_processes.lock().await;
                match processes.get_mut(&pid) {
                    Some(child) => match child.try_wait() {
                        Ok(Some(status)) => {
                            processes.remove(&pid);
                            Some(Ok(status))
                        }
                        Ok(None) => None,
                        Err(error) => {
                            processes.remove(&pid);
                            Some(Err(error))
                        }
                    },
                    None => Some(Err(std::io::Error::other(
                        "Minecraft process tracking ended unexpectedly.",
                    ))),
                }
            };
            if let Some(result) = result {
                match result {
                    Ok(status) => {
                        let code = status.code().unwrap_or(-1);
                        let stopped = stopped_processes.lock().await.remove(&pid);
                        if let Err(error) = forget_tracked_game(&app, pid) {
                            let _ = app.emit(
                                "log",
                                format!("Could not clear Minecraft process tracking: {error}"),
                            );
                        }
                        let _ = app.emit(
                            "game-exit",
                            serde_json::json!({
                                "code": code,
                                "success": status.success(),
                                "stopped": stopped,
                                "pid": pid,
                                "profile_id": profile_id,
                                "profile_name": profile_name,
                            }),
                        );
                    }
                    Err(error) => {
                        let _ = stopped_processes.lock().await.remove(&pid);
                        if let Err(clear_error) = forget_tracked_game(&app, pid) {
                            let _ = app.emit(
                                "log",
                                format!(
                                    "Could not clear Minecraft process tracking: {clear_error}"
                                ),
                            );
                        }
                        let _ = app.emit(
                            "log",
                            format!("Could not monitor Minecraft process {pid}: {error}"),
                        );
                        let _ = app.emit(
                            "game-exit",
                            serde_json::json!({
                                "code": -1,
                                "success": false,
                                "stopped": false,
                                "pid": pid,
                                "profile_id": profile_id,
                                "profile_name": profile_name,
                            }),
                        );
                    }
                }
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    });
    Ok(pid)
}

async fn read_lines<R>(reader: R, app: AppHandle, pid: u32)
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncBufReadExt;
    let mut lines = tokio::io::BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let _ = app.emit("game-log", serde_json::json!({ "pid": pid, "line": line }));
    }
}
