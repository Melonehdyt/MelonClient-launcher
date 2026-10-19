use crate::settings::StoredAccount;
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const MSA_DEVICE: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/devicecode";
const MSA_TOKEN: &str = "https://login.microsoftonline.com/consumers/oauth2/v2.0/token";
const XBL_AUTH: &str = "https://user.auth.xboxlive.com/user/authenticate";
const XSTS_AUTH: &str = "https://xsts.auth.xboxlive.com/xsts/authorize";
const MC_AUTH: &str = "https://api.minecraftservices.com/authentication/login_with_xbox";
const MC_PROFILE: &str = "https://api.minecraftservices.com/minecraft/profile";
const MSA_SCOPE: &str = "XboxLive.signin offline_access";

// Official Minecraft (Windows/Xbox) client ID used by vanilla launcher
pub const OFFICIAL_CLIENT_ID: &str = "00000000402b5328";
const MS_AUTH_TOKEN_URL: &str = "https://login.live.com/oauth20_token.srf";

#[derive(Serialize, Deserialize, Clone)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub message: String,
    pub interval: u64,
}

#[derive(Clone)]
pub struct MsaTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

#[derive(Serialize, Clone)]
pub struct Textures {
    pub skin: Option<String>,
    pub cape: Option<String>,
    pub slim: bool,
}

#[derive(Serialize, Clone)]
pub struct Cape {
    pub id: String,
    pub alias: String,
    pub active: bool,
    pub url: Option<String>,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct McProfile {
    pub name: String,
    pub uuid: String,
    pub access_token: String,
    /// "msa" or "legacy"
    pub user_type: String,
}

fn err_json(value: &Value) -> String {
    value.to_string()
}

pub async fn start_device_code(client: &reqwest::Client, client_id: &str) -> Result<DeviceCode> {
    if client_id.trim().is_empty() {
        bail!("No Azure client id configured. Add one in Settings.");
    }
    let response = client
        .post(MSA_DEVICE)
        .form(&[("client_id", client_id), ("scope", MSA_SCOPE)])
        .send()
        .await?;
    let status = response.status();
    let value: Value = response.json().await?;
    if !status.is_success() {
        bail!("Microsoft device-code request failed: {}", err_json(&value));
    }
    Ok(DeviceCode {
        device_code: value["device_code"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        user_code: value["user_code"].as_str().unwrap_or_default().to_string(),
        verification_uri: value["verification_uri"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        message: value["message"].as_str().unwrap_or_default().to_string(),
        interval: value["interval"].as_u64().unwrap_or(5),
    })
}

pub async fn poll_for_token(
    client: &reqwest::Client,
    client_id: &str,
    device_code: &str,
    interval: u64,
) -> Result<MsaTokens> {
    let mut delay = interval.max(1);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        let response = client
            .post(MSA_TOKEN)
            .form(&[
                ("client_id", client_id),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code),
            ])
            .send()
            .await?;
        let status = response.status();
        let value: Value = response.json().await?;
        if status.is_success() {
            return Ok(MsaTokens {
                access_token: value["access_token"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                refresh_token: value["refresh_token"].as_str().map(str::to_string),
            });
        }
        match value["error"].as_str().unwrap_or("") {
            "authorization_pending" => continue,
            "slow_down" => {
                delay += 5;
                continue;
            }
            "expired_token" => bail!("The sign-in code expired. Please try again."),
            "authorization_declined" => bail!("Sign-in was declined."),
            "bad_verification_code" => bail!("Invalid sign-in code."),
            other => bail!(
                "Microsoft sign-in failed ({}): {}",
                other,
                value["error_description"].as_str().unwrap_or("")
            ),
        }
    }
}

pub async fn refresh_msa(
    client: &reqwest::Client,
    client_id: &str,
    refresh_token: &str,
) -> Result<MsaTokens> {
    if client_id.trim().is_empty() {
        bail!("No Azure client id configured. Add one in Settings.");
    }
    let response = client
        .post(MSA_TOKEN)
        .form(&[
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("scope", MSA_SCOPE),
        ])
        .send()
        .await?;
    let status = response.status();
    let value: Value = response.json().await?;
    if !status.is_success() {
        bail!(
            "Microsoft token refresh failed ({}): {}",
            value["error"].as_str().unwrap_or("error"),
            value["error_description"].as_str().unwrap_or("")
        );
    }
    Ok(MsaTokens {
        access_token: value["access_token"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        refresh_token: value["refresh_token"]
            .as_str()
            .map(str::to_string)
            .or_else(|| Some(refresh_token.to_string())),
    })
}

pub async fn authenticate_minecraft(
    client: &reqwest::Client,
    msa_access_token: &str,
) -> Result<McProfile> {
    let xbl_body = serde_json::json!({
        "Properties": {
            "AuthMethod": "RPS",
            "SiteName": "user.auth.xboxlive.com",
            "RpsTicket": format!("d={msa_access_token}")
        },
        "RelyingParty": "http://auth.xboxlive.com",
        "TokenType": "JWT"
    });
    let value: Value = client
        .post(XBL_AUTH)
        .json(&xbl_body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let xbl_token = value["Token"]
        .as_str()
        .ok_or_else(|| anyhow!("Xbox Live authentication did not return a token."))?
        .to_string();

    let xsts_body = serde_json::json!({
        "Properties": {
            "SandboxId": "RETAIL",
            "UserTokens": [xbl_token]
        },
        "RelyingParty": "rp://api.minecraftservices.com/",
        "TokenType": "JWT"
    });
    let response = client.post(XSTS_AUTH).json(&xsts_body).send().await?;
    let status = response.status();
    let value: Value = response.json().await?;
    if !status.is_success() {
        let xerr = value["XErr"].as_u64().unwrap_or(0);
        let message = match xerr {
            2148916227 => "This account has been banned from Xbox Live.",
            2148916233 => "This Microsoft account has no Xbox profile. Sign in once at xbox.com to create one.",
            2148916235 => "Xbox Live is not available in this country or region.",
            2148916236 | 2148916237 => "This account needs to complete age verification.",
            2148916238 => "This is a child account. Add it to a Microsoft Family before signing in.",
            _ => "Xbox Live authorization was rejected.",
        };
        bail!("{message}");
    }
    let xsts_token = value["Token"]
        .as_str()
        .ok_or_else(|| anyhow!("XSTS authorization did not return a token."))?
        .to_string();
    let uhs = value["DisplayClaims"]["xui"][0]["uhs"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let mc_body = serde_json::json!({ "identityToken": format!("XBL3.0 x={uhs};{xsts_token}") });
    let response = client.post(MC_AUTH).json(&mc_body).send().await?;
    let status = response.status();
    let value: Value = response.json().await?;
    if !status.is_success() {
        bail!(
            "Minecraft login failed. Does this account own Minecraft: Java Edition? ({})",
            err_json(&value)
        );
    }
    let mc_token = value["access_token"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let response = client.get(MC_PROFILE).bearer_auth(&mc_token).send().await?;
    let status = response.status();
    let value: Value = response.json().await?;
    if !status.is_success() {
        bail!("Could not load the Minecraft profile: {}", err_json(&value));
    }

    Ok(McProfile {
        name: value["name"].as_str().unwrap_or_default().to_string(),
        uuid: value["id"].as_str().unwrap_or_default().to_string(),
        access_token: mc_token,
        user_type: "msa".to_string(),
    })
}

/// Downloads a PNG and returns it as a base64 `data:` URL.
async fn to_data_url(client: &reqwest::Client, url: &str) -> Result<String> {
    use base64::Engine;
    let bytes = client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:image/png;base64,{encoded}"))
}

/// Fetches the decoded skin and cape textures for a premium account from the
/// Mojang session server (bypasses any webview CORS restrictions).
pub async fn fetch_textures(client: &reqwest::Client, uuid: &str) -> Result<Textures> {
    use base64::Engine;
    let empty = Textures {
        skin: None,
        cape: None,
        slim: false,
    };

    let id = uuid.replace('-', "");
    if id.is_empty() {
        return Ok(empty);
    }

    let response = client
        .get(format!(
            "https://sessionserver.mojang.com/session/minecraft/profile/{id}"
        ))
        .send()
        .await?;
    if !response.status().is_success() {
        return Ok(empty);
    }
    let profile: Value = response.json().await?;

    let properties = profile["properties"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let encoded = properties
        .iter()
        .find(|prop| prop["name"].as_str() == Some("textures"))
        .and_then(|prop| prop["value"].as_str())
        .unwrap_or_default();
    if encoded.is_empty() {
        return Ok(empty);
    }

    let decoded = base64::engine::general_purpose::STANDARD.decode(encoded)?;
    let json: Value = serde_json::from_slice(&decoded)?;
    let textures = &json["textures"];

    let skin_url = textures["SKIN"]["url"].as_str().map(str::to_string);
    let cape_url = textures["CAPE"]["url"].as_str().map(str::to_string);
    let slim = textures["SKIN"]["metadata"]["model"].as_str() == Some("slim");

    let skin = match skin_url {
        Some(url) => to_data_url(client, &url).await.ok(),
        None => None,
    };
    let cape = match cape_url {
        Some(url) => to_data_url(client, &url).await.ok(),
        None => None,
    };

    Ok(Textures { skin, cape, slim })
}

/// Turns the `capes` array of a Minecraft profile response into `Cape` entries,
/// downloading each texture to a data URL. Shared by the list and equip paths.
async fn parse_capes(client: &reqwest::Client, profile: &Value) -> Vec<Cape> {
    let capes = profile["capes"].as_array().cloned().unwrap_or_default();
    let mut out = Vec::with_capacity(capes.len());
    for cape in capes {
        let id = cape["id"].as_str().unwrap_or_default().to_string();
        let alias = cape["alias"]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| id.clone());
        let active = cape["state"].as_str() == Some("ACTIVE");
        let url = match cape["url"].as_str() {
            Some(url) => to_data_url(client, url).await.ok(),
            None => None,
        };
        out.push(Cape {
            id,
            alias,
            active,
            url,
        });
    }
    out
}

/// Lists every cape the account owns (via the authenticated Minecraft profile
/// endpoint). Only the authenticated account can see the full list, so an
/// access token is required.
pub async fn fetch_capes(client: &reqwest::Client, access_token: &str) -> Result<Vec<Cape>> {
    let response = client
        .get(MC_PROFILE)
        .bearer_auth(access_token)
        .send()
        .await?;
    if !response.status().is_success() {
        bail!("Could not load the Minecraft profile for capes.");
    }
    let profile: Value = response.json().await?;
    Ok(parse_capes(client, &profile).await)
}

/// Equips (`Some(cape_id)`) or removes (`None`) the account's active cape on
/// Mojang's side. Returns the refreshed cape list parsed straight out of the
/// PUT response, so a change that Mojang accepted is never reported as failed
/// just because a follow-up fetch flaked.
pub async fn set_active_cape(
    client: &reqwest::Client,
    access_token: &str,
    cape_id: Option<&str>,
) -> Result<Vec<Cape>> {
    let endpoint = format!("{MC_PROFILE}/capes/active");
    let request = match cape_id {
        Some(id) => client
            .put(&endpoint)
            .bearer_auth(access_token)
            .json(&serde_json::json!({ "capeId": id })),
        None => client.delete(&endpoint).bearer_auth(access_token),
    };
    let response = request.send().await?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Cape change was rejected ({status}): {body}");
    }
    match serde_json::from_str::<Value>(&body) {
        Ok(profile) if profile.is_object() => Ok(parse_capes(client, &profile).await),
        // Some proxies answer without a JSON body; fall back to a normal fetch.
        _ => fetch_capes(client, access_token).await,
    }
}

/// Uploads a new skin to the authenticated Minecraft profile.
pub async fn set_skin(
    client: &reqwest::Client,
    access_token: &str,
    bytes: Vec<u8>,
    model: &str,
) -> Result<()> {
    if !matches!(model, "classic" | "slim") {
        bail!("Choose either the Classic or Slim skin model.");
    }

    let file = reqwest::multipart::Part::bytes(bytes)
        .file_name("skin.png")
        .mime_str("image/png")?;
    let form = reqwest::multipart::Form::new()
        .text("variant", model.to_string())
        .part("file", file);
    let response = client
        .post(format!("{MC_PROFILE}/skins"))
        .bearer_auth(access_token)
        .multipart(form)
        .send()
        .await?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("Minecraft skin upload was rejected ({status}): {body}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Minecraft access-token cache
//
// resolve_account runs the whole Microsoft -> XBL -> XSTS -> Minecraft chain,
// and the Microsoft refresh token is rotated (single use) on every run. Doing
// that once per command meant that flipping through capes a few times burned
// the stored refresh token mid-session and failed with
// `Minecraft login failed ... /authentication/login_with_xbox`. These helpers
// keep the resulting access token around so repeated commands reuse it, and
// serialise the refresh itself so two overlapping requests cannot race.
// ---------------------------------------------------------------------------

struct CachedMc {
    profile: McProfile,
    expires_at: Instant,
}

fn mc_cache() -> &'static Mutex<HashMap<String, CachedMc>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedMc>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Minecraft access tokens last about 24 hours; refresh well before that.
const MC_TOKEN_TTL: Duration = Duration::from_secs(20 * 60 * 60);

fn cache_get(account_id: &str) -> Option<McProfile> {
    let cache = mc_cache().lock().ok()?;
    let entry = cache.get(account_id)?;
    if entry.expires_at > Instant::now() {
        Some(entry.profile.clone())
    } else {
        None
    }
}

fn cache_put(account_id: &str, profile: &McProfile) {
    if let Ok(mut cache) = mc_cache().lock() {
        cache.insert(
            account_id.to_string(),
            CachedMc {
                profile: profile.clone(),
                expires_at: Instant::now() + MC_TOKEN_TTL,
            },
        );
    }
}

/// Drops a cached token so the next call re-authenticates (use when Mojang
/// rejects the token we still hold a copy of).
pub fn invalidate_token(account_id: &str) {
    if let Ok(mut cache) = mc_cache().lock() {
        cache.remove(account_id);
    }
}

/// Serialises the refresh so only one caller at a time burns a Microsoft
/// refresh token.
fn refresh_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Like [`resolve_account`], but reuses a cached Minecraft access token and only
/// runs the Xbox chain when there is no valid one left. Safe to call from every
/// command and from launch.
pub async fn resolve_account_cached(
    client: &reqwest::Client,
    client_id: &str,
    account: &StoredAccount,
) -> Result<(McProfile, Option<String>)> {
    if let Some(profile) = cache_get(&account.id) {
        return Ok((profile, None));
    }
    let _guard = refresh_lock().lock().await;
    // Another caller may have filled the cache while we waited for the lock.
    if let Some(profile) = cache_get(&account.id) {
        return Ok((profile, None));
    }
    let (profile, rotated) = resolve_account(client, client_id, account).await?;
    cache_put(&account.id, &profile);
    Ok((profile, rotated))
}

/// Refreshes a browser (login.live.com) Microsoft token. Unlike the device-code
/// flow this uses the official Minecraft client id and the live.com endpoint.
pub async fn refresh_msa_live(client: &reqwest::Client, refresh_token: &str) -> Result<MsaTokens> {
    let response = client
        .post(MS_AUTH_TOKEN_URL)
        .form(&[
            ("client_id", OFFICIAL_CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("scope", "XboxLive.signin offline_access"),
        ])
        .send()
        .await?;
    let status = response.status();
    let value: Value = response.json().await?;
    if !status.is_success() {
        bail!(
            "Microsoft token refresh failed ({}): {}",
            value["error"].as_str().unwrap_or("error"),
            value["error_description"].as_str().unwrap_or("")
        );
    }
    Ok(MsaTokens {
        access_token: value["access_token"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        refresh_token: value["refresh_token"]
            .as_str()
            .map(str::to_string)
            .or_else(|| Some(refresh_token.to_string())),
    })
}

/// Builds a fresh `McProfile` for a stored account. Returns the profile and an
/// updated Microsoft refresh token (when the token was rotated).
pub async fn resolve_account(
    client: &reqwest::Client,
    client_id: &str,
    account: &StoredAccount,
) -> Result<(McProfile, Option<String>)> {
    match account.kind.as_str() {
        "msa" => {
            let refresh = account
                .refresh_token
                .as_ref()
                .ok_or_else(|| anyhow!("No Microsoft refresh token stored. Sign in again."))?;
            let tokens = if account.flow == "consumers" {
                refresh_msa(client, client_id, refresh).await?
            } else {
                refresh_msa_live(client, refresh).await?
            };
            let profile = authenticate_minecraft(client, &tokens.access_token).await?;
            Ok((profile, tokens.refresh_token))
        }
        _ => bail!("This account type is not supported."),
    }
}

pub async fn login_minecraft_browser(
    client: &reqwest::Client,
    app: tauri::AppHandle,
) -> Result<(McProfile, Option<String>)> {
    use base64::Engine;
    use rand::Rng;
    use ring::digest;
    use ring::rand::{SecureRandom, SystemRandom};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    let mut verifier_bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut verifier_bytes)
        .map_err(|e| anyhow!(format!("{e:?}")))?;
    let code_verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(verifier_bytes);

    let hash = digest::digest(&digest::SHA256, code_verifier.as_bytes());
    let code_challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash.as_ref());

    let redirect_uri = "https://login.live.com/oauth20_desktop.srf";
    let state = format!("{}", rand::thread_rng().gen::<u64>());

    let auth_url = url::Url::parse_with_params(
        "https://login.live.com/oauth20_authorize.srf",
        &[
            ("client_id", OFFICIAL_CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", redirect_uri),
            ("scope", "XboxLive.signin offline_access"),
            ("state", &state),
            ("code_challenge", &code_challenge),
            ("code_challenge_method", "S256"),
            ("prompt", "select_account"),
        ],
    )?;
    let auth_url_str = auth_url.to_string();

    let code_result: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let code_result_clone = code_result.clone();
    let label = format!("ms-auth-{}", rand::thread_rng().gen::<u32>());
    let label_clone = label.clone();

    let _window = WebviewWindowBuilder::new(&app, &label, WebviewUrl::External(auth_url_str.parse()?))
        .title("Sign in with Microsoft")
        .inner_size(510.0, 600.0)
        .resizable(false)
        .center()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36")
        .build()?;

    let app_handle = app.clone();
    thread::spawn(move || loop {
        if let Some(w) = app_handle.get_webview_window(&label_clone) {
            if let Ok(url) = w.url() {
                let s = url.to_string();
                if s.starts_with(redirect_uri) {
                    if let Ok(u) = url::Url::parse(&s) {
                        for (k, v) in u.query_pairs() {
                            if k == "code" {
                                *code_result_clone.lock().unwrap() = Some(v.to_string());
                                break;
                            }
                        }
                    }
                    let _ = w.close();
                    break;
                }
            }
        } else {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    });

    let mut got: Option<String> = None;
    for _ in 0..300 {
        if let Some(c) = code_result.lock().unwrap().take() {
            got = Some(c);
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.close();
    }

    let auth_code = got.ok_or_else(|| anyhow!("Microsoft login timed out or was cancelled"))?;

    let scope_variants: [Option<&str>; 3] = [
        Some("XboxLive.signin offline_access"),
        Some("XboxLive.signin"),
        None,
    ];
    let mut token: Option<Value> = None;
    let mut last_err = String::new();
    for scope in scope_variants {
        let mut params: Vec<(&str, String)> = vec![
            ("client_id", OFFICIAL_CLIENT_ID.to_string()),
            ("code", auth_code.clone()),
            ("grant_type", "authorization_code".to_string()),
            ("code_verifier", code_verifier.clone()),
            ("redirect_uri", redirect_uri.to_string()),
        ];
        if let Some(s) = scope {
            params.push(("scope", s.to_string()));
        }
        let resp = client.post(MS_AUTH_TOKEN_URL).form(&params).send().await?;
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(Value::Null);
        if status.is_success() && body["access_token"].as_str().is_some() {
            let has_refresh = body["refresh_token"].as_str().is_some();
            // Keep the first success as a fallback, but prefer one that also
            // returned a refresh token so the session survives a restart.
            if token.is_none() {
                token = Some(body.clone());
            }
            if has_refresh {
                token = Some(body);
                break;
            }
        } else {
            last_err = format!(
                "{status}: {}",
                body.to_string().chars().take(300).collect::<String>()
            );
        }
    }
    let token = token.ok_or_else(|| anyhow!("Token exchange failed ({last_err})"))?;
    let ms_access = token["access_token"]
        .as_str()
        .ok_or_else(|| anyhow!("No access token"))?
        .to_string();

    let ms_refresh = token["refresh_token"].as_str().map(|s| s.to_string());

    let xbl_body = serde_json::json!({
        "Properties": { "AuthMethod": "RPS", "SiteName": "user.auth.xboxlive.com", "RpsTicket": format!("d={ms_access}") },
        "RelyingParty": "http://auth.xboxlive.com",
        "TokenType": "JWT"
    });
    let xbl: Value = client
        .post(XBL_AUTH)
        .json(&xbl_body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let xbl_token = xbl["Token"]
        .as_str()
        .ok_or_else(|| anyhow!("XBL missing"))?
        .to_string();

    let xsts_body = serde_json::json!({ "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl_token] }, "RelyingParty": "rp://api.minecraftservices.com/", "TokenType": "JWT" });
    let xsts_resp = client.post(XSTS_AUTH).json(&xsts_body).send().await?;
    let status = xsts_resp.status();
    let xsts_val: Value = xsts_resp.json().await?;
    if !status.is_success() {
        let xerr = xsts_val["XErr"].as_u64().unwrap_or(0);
        let msg = match xerr {
            2148916227 => "Account banned",
            2148916233 => "No Xbox profile",
            2148916235 => "Xbox unavailable",
            2148916236 | 2148916237 => "Age verification",
            2148916238 => "Child account",
            _ => "XSTS rejected",
        };
        bail!("{msg}");
    }
    let xsts_token = xsts_val["Token"]
        .as_str()
        .ok_or_else(|| anyhow!("XSTS missing"))?
        .to_string();
    let uhs = xsts_val["DisplayClaims"]["xui"][0]["uhs"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let mc_body2 = serde_json::json!({ "identityToken": format!("XBL3.0 x={uhs};{xsts_token}") });
    let mc: Value = client
        .post(MC_AUTH)
        .json(&mc_body2)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let mc_token = mc["access_token"]
        .as_str()
        .ok_or_else(|| anyhow!("MC missing"))?
        .to_string();

    let prof: Value = client
        .get(MC_PROFILE)
        .bearer_auth(&mc_token)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok((
        McProfile {
            name: prof["name"].as_str().unwrap_or_default().to_string(),
            uuid: prof["id"].as_str().unwrap_or_default().to_string(),
            access_token: mc_token,
            user_type: "msa".to_string(),
        },
        ms_refresh,
    ))
}
