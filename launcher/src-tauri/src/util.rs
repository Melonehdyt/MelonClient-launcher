use anyhow::{bail, Result};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// A hash of the launcher-relevant feature flags used when evaluating version
/// argument / library rules. Everything defaults to "off", which matches a
/// normal vanilla launch.
#[derive(Default, Clone)]
pub struct Features {
    pub is_demo_user: bool,
    pub has_custom_resolution: bool,
    pub is_quick_play_singleplayer: bool,
    pub is_quick_play_multiplayer: bool,
    pub is_quick_play_realms: bool,
}

pub fn os_name() -> &'static str {
    if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

pub fn os_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        "x86"
    }
}

/// Evaluates a Mojang-style rules array. An empty array deactivates the entry.
pub fn rules_pass(rules: &[Value], feats: &Features) -> bool {
    if rules.is_empty() {
        return true;
    }
    let mut allowed = false;
    for rule in rules {
        let action = rule
            .get("action")
            .and_then(Value::as_str)
            .unwrap_or("allow");
        let mut applies = true;

        if let Some(os) = rule.get("os") {
            if let Some(name) = os.get("name").and_then(Value::as_str) {
                if name != os_name() {
                    applies = false;
                }
            }
            if let Some(arch) = os.get("arch").and_then(Value::as_str) {
                if arch != os_arch() {
                    applies = false;
                }
            }
        }

        if let Some(features) = rule.get("features").and_then(Value::as_object) {
            for (key, value) in features {
                let want = value.as_bool().unwrap_or(false);
                let have = match key.as_str() {
                    "is_demo_user" => feats.is_demo_user,
                    "has_custom_resolution" => feats.has_custom_resolution,
                    "is_quick_play_singleplayer" => feats.is_quick_play_singleplayer,
                    "is_quick_play_multiplayer" => feats.is_quick_play_multiplayer,
                    "is_quick_play_realms" => feats.is_quick_play_realms,
                    _ => false,
                };
                if have != want {
                    applies = false;
                }
            }
        }

        if applies {
            allowed = action == "allow";
        }
    }
    allowed
}

pub fn sha1_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha1::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize().as_slice())
}

pub fn sha1_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha1::new();
    let mut buf = [0u8; 65536];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(hex::encode(hasher.finalize().as_slice()))
}

/// Performs `GET url` and writes the body to `dest`, verifying the SHA-1 when
/// provided. Existing files with a matching checksum are left untouched.
pub async fn ensure_file(
    client: &reqwest::Client,
    url: &str,
    dest: &Path,
    sha1: Option<&str>,
) -> Result<()> {
    if dest.exists() {
        match sha1 {
            Some(expected) => {
                if let Ok(actual) = sha1_file(dest) {
                    if actual.eq_ignore_ascii_case(expected) {
                        return Ok(());
                    }
                }
            }
            None => return Ok(()),
        }
    }

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let bytes = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;

    if let Some(expected) = sha1 {
        let actual = sha1_hex(&bytes);
        if !actual.eq_ignore_ascii_case(expected) {
            bail!("Checksum mismatch for {url}\n  expected: {expected}\n  actual:   {actual}");
        }
    }

    if dest.exists() {
        let _ = tokio::fs::remove_file(dest).await;
    }
    tokio::fs::write(dest, &bytes).await?;
    Ok(())
}

/// Converts a Maven coordinate (`group:artifact:version[:classifier][@ext]`)
/// into its repository-relative path.
pub fn maven_rel_path(name: &str) -> String {
    let (coords, ext) = match name.split_once('@') {
        Some((coords, ext)) => (coords, ext.to_string()),
        None => (name, "jar".to_string()),
    };
    let parts: Vec<&str> = coords.split(':').collect();
    let group = parts.first().copied().unwrap_or("").replace('.', "/");
    let artifact = parts.get(1).copied().unwrap_or("");
    let version = parts.get(2).copied().unwrap_or("");
    let classifier = parts.get(3).map(|c| format!("-{c}")).unwrap_or_default();
    format!("{group}/{artifact}/{version}/{artifact}-{version}{classifier}.{ext}")
}

/// Extracts every entry of `jar` into `dest`, skipping entries that start with
/// one of the `excludes` prefixes (e.g. `META-INF/`).
pub fn extract_jar(jar: &Path, dest: &Path, excludes: &[&str]) -> Result<()> {
    let file = std::fs::File::open(jar)?;
    let mut archive = zip::ZipArchive::new(file)?;
    std::fs::create_dir_all(dest)?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_string();
        if entry.is_dir() {
            continue;
        }
        if excludes.iter().any(|exclude| name.starts_with(exclude)) {
            continue;
        }
        let out = dest.join(&name);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut file)?;
    }
    Ok(())
}

pub fn java_exe_name() -> &'static str {
    if cfg!(windows) {
        "java.exe"
    } else {
        "java"
    }
}

/// Parses the major version out of `java -version` output, handling both the
/// legacy `1.8.0_xxx` scheme and the modern `21.0.1` scheme.
pub fn parse_java_major(text: &str) -> Option<u32> {
    let marker = "version \"";
    let start = text.find(marker)? + marker.len();
    let rest = &text[start..];
    let end = rest.find('"')?;
    let version = &rest[..end];
    let major = if let Some(stripped) = version.strip_prefix("1.") {
        stripped.split(['.', '_', '-']).next()?
    } else {
        version.split(['.', '_', '-']).next()?
    };
    major.parse().ok()
}

/// Runs `<path> -version` and returns the detected major Java version.
pub fn java_major_of(path: &Path) -> Option<u32> {
    let mut command = std::process::Command::new(path);
    command.arg("-version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command.output().ok()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_java_major(&stderr).or_else(|| parse_java_major(&stdout))
}

/// Recursively searches `dir` (up to `max_depth` levels) for a `bin/java(.exe)`.
pub fn find_java_exe(dir: &Path, max_depth: u32) -> Option<PathBuf> {
    if max_depth == 0 {
        return None;
    }
    let exe = dir.join("bin").join(java_exe_name());
    if exe.is_file() {
        return Some(exe);
    }
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_java_exe(&path, max_depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize().as_slice())
}

/// Extracts a generic zip archive into `dest`.
pub fn extract_zip(zip: &Path, dest: &Path) -> Result<()> {
    extract_jar(zip, dest, &[])
}

/// Replaces `${key}` placeholders in `input` using the supplied variable map.
pub fn substitute(input: &str, vars: &HashMap<&str, String>) -> String {
    let mut out = input.to_string();
    for (key, value) in vars {
        let needle = format!("${{{key}}}");
        if out.contains(&needle) {
            out = out.replace(&needle, value);
        }
    }
    out
}
