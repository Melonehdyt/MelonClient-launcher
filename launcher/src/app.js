const invoke = window.__TAURI__.core.invoke;
const listen = window.__TAURI__.event.listen;
const convertFileSrc = window.__TAURI__.core.convertFileSrc;

const $ = (id) => document.getElementById(id);

let settings = null;
let busy = false;
let accounts = [];
let profiles = [];
let activeProfileId = "";
const runningGames = new Map();
let newProfileAvatarData = null;
let modrinthCache = new Map();
let modrinthSearchToken = 0;
let installedModsLoadToken = 0;
let recentGameLogs = [];
const consoleLogs = new Map([["launcher", []]]);
const consoleInstances = new Map();
let selectedConsoleId = "launcher";
const PROFILE_DEFAULT_ICON = "img/logo.png";
const CONTENT_TYPES = {
  mod: { label: "mods", singular: "mod", directory: "mods", extension: ".jar" },
  resourcepack: {
    label: "resource packs",
    singular: "resource pack",
    directory: "resourcepacks",
    extension: ".zip",
  },
  shader: {
    label: "shader packs",
    singular: "shader pack",
    directory: "shaderpacks",
    extension: ".zip",
  },
};
let localSkins = [];
let selectedSkinId = "";
let selectedSkinModel = "classic";
let skinModelOverridden = false;
let currentSkinSource = "";
let currentSkinName = "No skin selected";
let currentCapeSource = null;

function setStatus(text) {
  $("status").textContent = text;
}

function setBusy(value) {
  busy = value;
  $("btn-launch").disabled = value;
}

function renderRunningGames() {
  const select = $("running-game-select");
  const previous = select.value;
  select.replaceChildren();
  for (const [pid, game] of runningGames) {
    const option = document.createElement("option");
    option.value = pid;
    option.textContent = `${game.profile_name} · ${game.version} · ${loaderLabel(game.loader)}`;
    select.appendChild(option);
  }
  select.value = runningGames.has(previous) ? previous : [...runningGames.keys()].at(-1) || "";
  $("running-game-controls").hidden = runningGames.size === 0;
  $("hero-dot").classList.toggle("running", runningGames.size > 0);
  renderConsoleSources();
}

const CONSOLE_MAX_LINES = 3000;

// Classic Minecraft colour palette (§0-§9, §a-§f).
const MC_COLORS = {
  "0": "#000000",
  "1": "#0000AA",
  "2": "#00AA00",
  "3": "#00AAAA",
  "4": "#AA0000",
  "5": "#AA00AA",
  "6": "#FFAA00",
  "7": "#AAAAAA",
  "8": "#555555",
  "9": "#5555FF",
  a: "#55FF55",
  b: "#55FFFF",
  c: "#FF5555",
  d: "#FF55FF",
  e: "#FFFF55",
  f: "#FFFFFF",
};

function escapeHtml(text) {
  return String(text)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

const OBFUSCATED_POOL =
  "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!\"#$%&'()*+,-./:;<=>?@[]^_{|}~";

function scramble(text) {
  let out = "";
  for (const ch of text) {
    out += ch === " " ? " " : OBFUSCATED_POOL[(Math.random() * OBFUSCATED_POOL.length) | 0];
  }
  return out;
}

/**
 * Renders a line that may contain Minecraft formatting codes (§) as styled
 * markup, mirroring the in-game chat/log renderer: colour codes (including
 * §x hex colours) plus bold/italic/underline/strike/obfuscate and §r reset.
 */
function mcFormat(raw) {
  const chars = [...String(raw)];
  let out = "";
  let buf = "";
  let color = null;
  let bold = false;
  let italic = false;
  let underline = false;
  let strike = false;
  let obf = false;
  const sec = "\u00A7";

  const flush = () => {
    if (!buf) return;
    const body = escapeHtml(obf ? scramble(buf) : buf);
    const styles = [];
    if (color) styles.push(`color:${color}`);
    if (bold) styles.push("font-weight:700");
    if (italic) styles.push("font-style:italic");
    if (underline && strike) styles.push("text-decoration:underline line-through");
    else if (underline) styles.push("text-decoration:underline");
    else if (strike) styles.push("text-decoration:line-through");
    out += `<span${obf ? ' class="c-obf"' : ""}${
      styles.length ? ` style="${styles.join(";")}"` : ""
    }>${body}</span>`;
    buf = "";
  };

  for (let i = 0; i < chars.length; i++) {
    const ch = chars[i];
    if (ch !== sec || i + 1 >= chars.length) {
      buf += ch;
      continue;
    }
    const code = chars[++i].toLowerCase();

    // §x§R§R§G§G§B§B - the modern hex colour format used by 1.16+.
    if (code === "x") {
      flush();
      let hex = "";
      while (hex.length < 6 && i + 2 < chars.length && chars[i + 1] === sec) {
        hex += chars[i + 2];
        i += 2;
      }
      if (hex.length === 6) color = "#" + hex;
      continue;
    }

    flush();
    if (code === "k") obf = true;
    else if (code === "l") bold = true;
    else if (code === "m") strike = true;
    else if (code === "n") underline = true;
    else if (code === "o") italic = true;
    else if (code === "r") {
      color = null;
      bold = false;
      italic = false;
      underline = false;
      strike = false;
      obf = false;
    } else if (MC_COLORS[code]) {
      // Like the game, a colour code clears every previous formatting flag.
      bold = false;
      italic = false;
      underline = false;
      strike = false;
      obf = false;
      color = MC_COLORS[code];
    }
  }
  flush();
  return out;
}

// Gives plain vanilla log lines the colouring of the real Minecraft log, but
// only when the line does not already carry its own § codes.
function tintLevel(line) {
  if (line.includes("\u00A7")) return line;

  const match = line.match(
    /^\[[^\]]*\]\s*\[([^\]/]+)\/(INFO|WARN(?:ING)?|ERROR|FATAL|DEBUG|TRACE)\]/
  );
  if (match) {
    const head = line.slice(0, match[0].length);
    const rest = line.slice(match[0].length);
    const level = match[2];
    const tint =
      level === "ERROR" || level === "FATAL"
        ? "\u00A7c"
        : level === "WARN" || level === "WARNING"
          ? "\u00A7e"
          : level === "DEBUG" || level === "TRACE"
            ? "\u00A78"
            : "\u00A7a";
    const start = head.lastIndexOf(level);
    return `\u00A78${head.slice(0, start)}\u00A7r${tint}${level}\u00A7r${rest}`;
  }

  if (/^\[error\]/i.test(line)) return `\u00A7c${line}`;
  if (/^\[launcher\]/i.test(line)) return `\u00A77${line}`;
  return line;
}

function renderConsoleSources() {
  const select = $("console-instance-select");
  if (!select) return;
  select.replaceChildren();
  const launcherOption = document.createElement("option");
  launcherOption.value = "launcher";
  launcherOption.textContent = "Launcher";
  select.appendChild(launcherOption);
  for (const [pid, game] of consoleInstances) {
    const option = document.createElement("option");
    option.value = pid;
    const state = runningGames.has(pid) ? "running" : "exited";
    option.textContent =
      `${game.profile_name || "Minecraft"} · ${game.version || "unknown version"} · ` +
      `${loaderLabel(game.loader || "")} · ${state}`;
    select.appendChild(option);
  }
  if (!consoleLogs.has(selectedConsoleId)) selectedConsoleId = "launcher";
  select.value = selectedConsoleId;
  renderConsole();
}

function renderConsole() {
  const pre = $("console");
  if (!pre) return;
  pre.replaceChildren();
  for (const line of consoleLogs.get(selectedConsoleId) || []) {
    const div = document.createElement("div");
    div.className = "c-line";
    div.innerHTML = mcFormat(tintLevel(line));
    pre.appendChild(div);
  }
  pre.scrollTop = pre.scrollHeight;
}

function appendConsole(line, source = "launcher") {
  const plainLine = redactPrivatePaths(String(line));
  recentGameLogs.push(plainLine);
  if (recentGameLogs.length > 1200) recentGameLogs.shift();
  let lines = consoleLogs.get(source);
  if (!lines) {
    lines = [];
    consoleLogs.set(source, lines);
  }
  lines.push(plainLine);
  if (lines.length > CONSOLE_MAX_LINES) lines.shift();
  if (source !== selectedConsoleId) return;
  const pre = $("console");
  const div = document.createElement("div");
  div.className = "c-line";
  div.innerHTML = mcFormat(tintLevel(plainLine));
  pre.appendChild(div);
  while (pre.children.length > CONSOLE_MAX_LINES) {
    pre.removeChild(pre.firstElementChild);
  }
  pre.scrollTop = pre.scrollHeight;
}

function redactPrivatePaths(text) {
  return text
    .replace(/\b([A-Z]:\\Users\\)[^\\/"']+/gi, "$1[hidden]")
    .replace(/\b([A-Z]:\/Users\/)[^\\/"']+/gi, "$1[hidden]")
    .replace(/(^|[\s"'(])\/Users\/[^/"']+/g, "$1/Users/[hidden]")
    .replace(/(^|[\s"'(])\/home\/[^/"']+/g, "$1/home/[hidden]");
}

function blurPathForDisplay(path) {
  return redactPrivatePaths(path).replace(
    /^([A-Z]:\\Users\\)\[hidden\]/i,
    "$1•••"
  ).replace(
    /^([A-Z]:\/Users\/)\[hidden\]/i,
    "$1•••"
  ).replace(
    /^(\/Users\/)\[hidden\]/,
    "$1•••"
  ).replace(
    /^(\/home\/)\[hidden\]/,
    "$1•••"
  );
}

function avatarUrl(uuid) {
  return uuid ? `https://mc-heads.net/avatar/${uuid}/40` : "";
}

function applyAvatar(el, account) {
  if (!el) return;
  if (account) {
    const url = avatarUrl(account.uuid);
    el.dataset.state = account.kind === "msa" ? "online" : "offline";
    el.style.backgroundImage = url ? `url("${url}")` : "";
  } else {
    el.dataset.state = "offline";
    el.style.backgroundImage = "";
  }
}

/* ---- Settings ---- */
function fillSettingsForm(s) {
  $("set-mcdir").value = s.minecraft_dir || "";
  $("set-java").value = s.java_path || "";
  $("set-memory").value = s.memory_mb || 4096;
  $("mem-label").textContent = s.memory_mb || 4096;
  $("set-jvm").value = s.extra_jvm_args || "";
  $("set-azid").value = s.azure_client_id || "";
  $("set-news-enabled").checked = s.news_enabled !== false;
  $("set-hide-after-launch").checked = s.hide_after_launch === true;
  $("minecraft-dir-label").textContent = blurPathForDisplay(s.minecraft_dir || "");
  fillBackgroundForm(s);
  applyBackground(s);
}

function renderProfiles(state) {
  profiles = state.profiles || [];
  activeProfileId = state.active_profile || "";
  const select = $("play-profile-select");
  select.replaceChildren();
  for (const profile of profiles) {
    const option = document.createElement("option");
    option.value = profile.id;
    option.textContent = profile.name;
    select.appendChild(option);
  }
  select.value = activeProfileId;
  const activeProfile = profiles.find((profile) => profile.id === activeProfileId);
  renderProfileIcon($("play-profile-avatar"), activeProfile && activeProfile.avatar_data);
  renderProfileCards();
  renderProfileIcon($("new-profile-avatar-preview"), newProfileAvatarData);
  $("mods-active-profile").textContent = activeProfile
    ? `${activeProfile.name} · ${activeProfile.version} · ${loaderLabel(activeProfile.mod_loader)}`
    : "No active game profile";
}

function renderProfileIcon(container, customIcon) {
  container.replaceChildren();
  const image = document.createElement("img");
  image.src = customIcon || PROFILE_DEFAULT_ICON;
  image.className = customIcon ? "profile-icon-custom" : "profile-icon-default";
  image.alt = "";
  container.appendChild(image);
}

function renderProfileCards() {
  const container = $("game-profile-cards");
  container.replaceChildren();
  for (const profile of profiles) {
    const card = document.createElement("article");
    card.className = "game-profile-card";
    card.classList.toggle("active", profile.id === activeProfileId);
    const select = document.createElement("button");
    select.type = "button";
    select.className = "game-profile-select";
    select.setAttribute("aria-pressed", String(profile.id === activeProfileId));
    select.addEventListener("click", () => openProfileMods(profile.id));
    const avatar = document.createElement("span");
    avatar.className = "game-profile-avatar";
    avatar.setAttribute("aria-hidden", "true");
    renderProfileIcon(avatar, profile.avatar_data);
    select.appendChild(avatar);
    const details = document.createElement("span");
    details.className = "game-profile-details";
    const name = document.createElement("strong");
    name.textContent = profile.name;
    const version = document.createElement("small");
    version.textContent = `${profile.version} · ${loaderLabel(profile.mod_loader)}`;
    details.append(name, version);
    const hint = document.createElement("small");
    hint.className = "profile-mods-hint";
    hint.textContent = "View installed mods";
    details.appendChild(hint);
    select.appendChild(details);
    card.appendChild(select);
    const actions = document.createElement("div");
    actions.className = "game-profile-actions";
    const imageButton = document.createElement("button");
    imageButton.type = "button";
    imageButton.className = "small";
    imageButton.textContent = "Change icon";
    imageButton.addEventListener("click", () => uploadProfileIcon(profile.id));
    actions.appendChild(imageButton);
    if (profile.id !== "default") {
      const deleteButton = document.createElement("button");
      deleteButton.type = "button";
      deleteButton.className = "small danger";
      deleteButton.textContent = "Delete";
      deleteButton.addEventListener("click", () => deleteGameProfile(profile));
      actions.appendChild(deleteButton);
    }
    card.appendChild(actions);
    container.appendChild(card);
  }
}

async function openProfileMods(id) {
  await selectActiveProfile(id);
  if (activeProfileId === id) activateTab("mods");
}

async function uploadProfileIcon(profileId = null) {
  try {
    const path = await invoke("pick_profile_icon_file");
    if (!path) return;
    const avatarData = await invoke("read_profile_icon", { source: path });
    if (!profileId) {
      newProfileAvatarData = avatarData;
      $("new-profile-icon-name").textContent = "Custom PNG selected";
      renderProfileIcon($("new-profile-avatar-preview"), newProfileAvatarData);
      return;
    }
    const state = await invoke("set_profile_custom_icon", {
      id: profileId,
      avatarData,
    });
    renderProfiles(state);
  } catch (error) {
    appendConsole("[error] Could not set profile icon: " + error);
  }
}

async function selectActiveProfile(id) {
  if (!id || id === activeProfileId || !profiles.some((profile) => profile.id === id)) return;
  try {
    await saveSettings();
    const state = await invoke("select_profile", { id });
    renderProfiles(state);
    await reloadActiveProfile();
    setStatus("Profile switched");
  } catch (error) {
    $("play-profile-select").value = activeProfileId;
    appendConsole("[error] Could not switch profile: " + error);
    setStatus("Profile switch failed");
  }
}

async function deleteGameProfile(profile) {
  if (!profile || profile.id === "default") return;
  const confirmed = window.confirm(
    `Delete "${profile.name}" and permanently remove its game files and mods?\n\nThis cannot be undone.`
  );
  if (!confirmed) return;
  const deletingActiveProfile = profile.id === activeProfileId;
  try {
    const state = await invoke("delete_profile", { id: profile.id });
    renderProfiles(state);
    if (deletingActiveProfile) await reloadActiveProfile();
    setStatus("Profile deleted");
  } catch (error) {
    appendConsole("[error] Could not delete profile: " + error);
    setStatus("Profile deletion failed");
  }
}

async function loadLauncherNews() {
  const status = $("news-status");
  const list = $("launcher-news-list");
  const newsCard = $("launcher-news");
  if (!settings || settings.news_enabled === false) {
    newsCard.hidden = true;
    status.textContent = "News is disabled in Settings.";
    list.replaceChildren();
    return;
  }
  newsCard.hidden = false;
  status.textContent = "Loading launcher news…";
  list.replaceChildren();
  try {
    const items = await invoke("fetch_launcher_news");
    if (items.length === 0) {
      status.textContent = "There are no news posts right now.";
      return;
    }
    status.textContent = "";
    for (const item of items) {
      const article = document.createElement("article");
      article.className = "launcher-news-item";
      const title = document.createElement("h4");
      title.textContent = item.title;
      article.appendChild(title);
      if (item.date) {
        const date = document.createElement("time");
        date.textContent = item.date;
        article.appendChild(date);
      }
      const body = document.createElement("p");
      body.textContent = item.body;
      article.appendChild(body);
      list.appendChild(article);
    }
  } catch (error) {
    status.textContent = "Could not load launcher news: " + error;
    appendConsole("[launcher] " + status.textContent);
  }
}

async function reloadActiveProfile() {
  settings = await invoke("get_settings");
  fillSettingsForm(settings);
  $("mod-loader-select").value = settings.mod_loader || "fabric";
  await loadVersions();
  await loadLocalSkins();
  await refreshInstalledMods();
  updateSummary();
}

function gatherSettings() {
  return {
    ...settings,
    minecraft_dir: $("set-mcdir").value.trim(),
    java_path: $("set-java").value.trim(),
    memory_mb: Number($("set-memory").value),
    extra_jvm_args: $("set-jvm").value.trim(),
    version: $("version-select").value || (settings && settings.version) || "latest-release",
    mod_loader: $("mod-loader-select").value || "fabric",
    loader: $("loader-select").value || "",
    azure_client_id: $("set-azid").value.trim(),
    news_enabled: $("set-news-enabled").checked,
    hide_after_launch: $("set-hide-after-launch").checked,
    ...backgroundFromForm(),
  };
}

async function saveSettings() {
  settings = gatherSettings();
  await invoke("set_settings", { settings });
  const activeProfile = profiles.find((profile) => profile.id === activeProfileId);
  if (activeProfile) {
    activeProfile.version = settings.version;
    activeProfile.mod_loader = settings.mod_loader;
    activeProfile.loader = settings.loader;
    renderProfiles({ active_profile: activeProfileId, profiles });
  }
  $("minecraft-dir-label").textContent = blurPathForDisplay(settings.minecraft_dir);
  $("mem-label").textContent = settings.memory_mb;
  applyBackground(settings);
  updateSummary();
  setStatus("Settings saved");
}

function updateSummary() {
  const type = (settings && settings.mod_loader) || "fabric";
  const loader = $("loader-select").value || (type === "forge" ? "recommended" : "latest");
  const name = type === "neoforge" ? "NeoForge" : type[0].toUpperCase() + type.slice(1);
  $("play-summary").textContent = `${name} ${loader} + your mods`;
  $("hero-version").textContent = (settings && settings.version) || "Latest release";
}

function loaderLabel(value) {
  return value === "neoforge" ? "NeoForge" : value[0].toUpperCase() + value.slice(1);
}

/* ---- Background ---- */
// Turns a picked file path into something the webview can load. Remote URLs and
// data URLs are passed straight through; local files go through Tauri's asset
// protocol (`assetProtocol` has to be enabled in tauri.conf.json).
function toMediaUrl(path) {
  if (!path) return "";
  if (/^(https?:|data:|blob:)/i.test(path)) return path;
  try {
    return convertFileSrc(path);
  } catch (_) {
    return path;
  }
}

// Accepts a video or playlist id and the common YouTube URL formats.
function youtubeEmbedSource(source) {
  if (!source) return "";
  const trimmed = source.trim();
  if (/^[\w-]{11}$/.test(trimmed)) return { kind: "video", id: trimmed };
  if (/^(PL|UU|LL|FL|RD|OLAK5uy_)[\w-]{8,}$/i.test(trimmed)) {
    return { kind: "playlist", id: trimmed };
  }
  try {
    const url = new URL(trimmed);
    const host = url.hostname.toLowerCase();
    if (!["youtube.com", "www.youtube.com", "m.youtube.com", "youtu.be",
      "www.youtu.be", "youtube-nocookie.com", "www.youtube-nocookie.com"].includes(host)) {
      return null;
    }
    const playlist = url.searchParams.get("list");
    if (playlist && /^[\w-]{10,128}$/.test(playlist)) {
      return { kind: "playlist", id: playlist };
    }
    if (host === "youtu.be" || host === "www.youtu.be") {
      const video = url.pathname.slice(1, 12);
      return /^[\w-]{11}$/.test(video) ? { kind: "video", id: video } : null;
    }
    const v = url.searchParams.get("v");
    if (v && /^[\w-]{11}$/.test(v)) return { kind: "video", id: v };
    const parts = url.pathname.split("/").filter(Boolean);
    const index = parts.findIndex((part) => part === "embed" || part === "shorts" || part === "live");
    if (index >= 0 && parts[index + 1] && /^[\w-]{11}$/.test(parts[index + 1])) {
      return { kind: "video", id: parts[index + 1] };
    }
  } catch (_) {
    /* not a URL, fall through */
  }
  return null;
}

const BG_FIT_OBJECT = { cover: "cover", contain: "contain", fill: "fill" };
const BG_FIT_SIZE = { cover: "cover", contain: "contain", fill: "100% 100%" };

function resizeBackground(fit) {
  const frame = $("bg-youtube-frame");
  const layer = $("bg-youtube");
  if (!frame || !layer || !layer.classList.contains("active")) return;
  const w = window.innerWidth;
  const h = window.innerHeight;
  const ar = 16 / 9;
  let width;
  let height;
  if (fit === "contain") {
    if (w / ar <= h) {
      width = w;
      height = w / ar;
    } else {
      height = h;
      width = h * ar;
    }
  } else {
    width = w;
    height = w / ar;
    if (height < h) {
      height = h;
      width = h * ar;
    }
  }
  frame.style.width = `${Math.round(width)}px`;
  frame.style.height = `${Math.round(height)}px`;
}

function applyYoutubeVolume(frame, volume, mute) {
  frame.contentWindow?.postMessage(
    JSON.stringify({
      event: "command",
      func: mute || volume === 0 ? "mute" : "unMute",
      args: [],
    }),
    "https://www.youtube-nocookie.com"
  );
  if (!mute && volume > 0) {
    frame.contentWindow?.postMessage(
      JSON.stringify({ event: "command", func: "setVolume", args: [volume] }),
      "https://www.youtube-nocookie.com"
    );
  }
}

// Applies the saved background settings to the DOM. Also used as a live preview
// while the user drags the sliders or picks a new source.
function applyBackground(s) {
  const background = $("background");
  if (!background) return;
  const type = (s && s.background_type) || "default";
  const source = (s && s.background_source) || "";
  const fit = (s && s.background_fit) || "cover";
  const dim = typeof s.background_dim === "number" ? s.background_dim : 35;
  const blur = typeof s.background_blur === "number" ? s.background_blur : 0;
  const panelTransparency =
    typeof s.background_panel_transparency === "number"
      ? Math.min(Math.max(s.background_panel_transparency, 0), 80)
      : 0;
  const volume =
    typeof s.background_volume === "number"
      ? Math.min(Math.max(s.background_volume, 0), 100)
      : 30;
  const mute = !s || s.background_mute !== false;
  const loop = !s || s.background_loop !== false;
  document.documentElement.style.setProperty(
    "--panel-transparency",
    `${panelTransparency}%`
  );

  let active = "bg-default";
  if (type === "picture" && source) active = "bg-picture";
  else if (type === "video" && source) active = "bg-video";
  const youtubeSource = type === "youtube" && source ? youtubeEmbedSource(source) : null;
  if (youtubeSource) active = "bg-youtube";
  const youtubeStatus = $("bg-youtube-status");
  youtubeStatus.hidden = type !== "youtube";
  youtubeStatus.textContent =
    type === "youtube" && source && !youtubeSource
      ? "Enter a valid YouTube video or playlist URL / ID."
      : type === "youtube" && youtubeSource?.kind === "playlist"
        ? "YouTube playlist selected."
        : type === "youtube" && youtubeSource
          ? "YouTube video selected."
          : "";

  background.querySelectorAll(".bg-layer").forEach((el) => {
    el.classList.toggle("active", el.id === active);
  });

  $("bg-dim").style.opacity = String(Math.min(Math.max(dim, 0), 100) / 100);
  background.classList.toggle("blurred", blur > 0);
  background.style.setProperty("--bg-blur", `${blur}px`);

  const picture = $("bg-picture");
  if (active === "bg-picture") {
    picture.style.backgroundImage = `url("${toMediaUrl(source)}")`;
    picture.style.backgroundSize = BG_FIT_SIZE[fit] || "cover";
  } else {
    picture.style.backgroundImage = "";
  }

  const video = $("bg-video-el");
  if (active === "bg-video") {
    const src = toMediaUrl(source);
    if (video.dataset.src !== src) {
      video.dataset.src = src;
      video.src = src;
    }
    video.style.objectFit = BG_FIT_OBJECT[fit] || "cover";
    video.volume = volume / 100;
    video.muted = mute;
    video.loop = loop;
    video.play().catch(() => {});
  } else if (video.dataset.src) {
    video.pause();
    video.removeAttribute("src");
    video.dataset.src = "";
    video.load();
  }

  const frame = $("bg-youtube-frame");
  if (active === "bg-youtube") {
    const path =
      youtubeSource.kind === "playlist"
        ? "embed/videoseries"
        : `embed/${youtubeSource.id}`;
    const params = new URLSearchParams({
      autoplay: "1",
      mute: mute || volume === 0 ? "1" : "0",
      loop: loop ? "1" : "0",
      controls: "0",
      rel: "0",
      playsinline: "1",
      modestbranding: "1",
      iv_load_policy: "3",
      enablejsapi: "1",
      origin: window.location.origin,
    });
    if (youtubeSource.kind === "playlist") params.set("list", youtubeSource.id);
    else params.set("playlist", youtubeSource.id);
    const embed = `https://www.youtube-nocookie.com/${path}?${params}`;
    if (frame.dataset.src !== embed) {
      frame.dataset.src = embed;
      frame.onload = () => applyYoutubeVolume(frame, volume, mute);
      frame.src = embed;
    }
    applyYoutubeVolume(frame, volume, mute);
  } else if (frame.dataset.src) {
    frame.dataset.src = "";
    frame.removeAttribute("src");
  }

  resizeBackground(fit);
}

// Reads the background controls and folds them into the current settings.
function backgroundFromForm() {
  return {
    background_type: $("set-bg-type").value,
    background_source: $("set-bg-source").value.trim(),
    background_fit: $("set-bg-fit").value,
    background_dim: Number($("set-bg-dim").value),
    background_blur: Number($("set-bg-blur").value),
    background_panel_transparency: Number($("set-bg-panel-transparency").value),
    background_volume: Number($("set-bg-volume").value),
    background_mute: $("set-bg-mute").checked,
    background_loop: $("btn-bg-loop").getAttribute("aria-pressed") === "true",
  };
}

function previewBackground() {
  applyBackground({ ...(settings || {}), ...backgroundFromForm() });
}

function fillBackgroundForm(s) {
  $("set-bg-type").value = (s && s.background_type) || "default";
  $("set-bg-source").value = (s && s.background_source) || "";
  $("set-bg-fit").value = (s && s.background_fit) || "cover";
  $("set-bg-dim").value = s && typeof s.background_dim === "number" ? s.background_dim : 35;
  $("set-bg-blur").value = s && typeof s.background_blur === "number" ? s.background_blur : 0;
  $("set-bg-panel-transparency").value =
    s && typeof s.background_panel_transparency === "number"
      ? Math.min(Math.max(s.background_panel_transparency, 0), 80)
      : 0;
  $("set-bg-volume").value =
    s && typeof s.background_volume === "number"
      ? Math.min(Math.max(s.background_volume, 0), 100)
      : 30;
  $("set-bg-mute").checked = !s || s.background_mute !== false;
  setBackgroundLoopButton(!s || s.background_loop !== false);
  $("bg-dim-label").textContent = $("set-bg-dim").value;
  $("bg-blur-label").textContent = $("set-bg-blur").value;
  $("bg-panel-transparency-label").textContent = $("set-bg-panel-transparency").value;
  $("bg-volume-label").textContent = $("set-bg-volume").value;
}

function setBackgroundLoopButton(enabled) {
  const button = $("btn-bg-loop");
  button.setAttribute("aria-pressed", String(enabled));
  button.textContent = `Loop video: ${enabled ? "On" : "Off"}`;
}

/* ---- Versions ---- */
async function loadVersions() {
  const select = $("version-select");
  select.innerHTML = "";
  const opts = [
    ["latest-release", "Latest release"],
    ["latest-snapshot", "Latest snapshot"],
  ];
  for (const [value, label] of opts) {
    const option = document.createElement("option");
    option.value = value;
    option.textContent = label;
    select.appendChild(option);
  }

  try {
    const versions = await invoke("minecraft_versions");
    for (const version of versions.filter((v) => v.type === "release")) {
      const option = document.createElement("option");
      option.value = version.id;
      option.textContent = version.id;
      select.appendChild(option);
    }
  } catch (err) {
    appendConsole("[launcher] Could not load version list: " + err);
  }

  if (settings && settings.version) {
    select.value = settings.version;
  }
  const profileVersion = $("new-profile-version");
  profileVersion.replaceChildren(...Array.from(select.options, (option) => option.cloneNode(true)));
  if ([...profileVersion.options].some((option) => option.value === select.value)) {
    profileVersion.value = select.value;
  }
  await loadLoaders();
}

async function loadNewProfileLoaderVersions() {
  const version = $("new-profile-version").value || "latest-release";
  const loaderType = $("new-profile-loader").value || "fabric";
  const select = $("new-profile-loader-version");
  $("new-profile-loader-label").textContent = `${loaderLabel(loaderType)} version`;
  select.replaceChildren(new Option(loaderType === "forge" ? "Recommended" : "Latest", ""));
  if (version.startsWith("latest-")) return;
  try {
    const versions = await invoke("loader_versions", { mc: version, modLoader: loaderType });
    for (const loader of versions.slice(0, 40)) {
      select.add(new Option(loader, loader));
    }
  } catch (error) {
    appendConsole(`[launcher] Could not list ${loaderLabel(loaderType)} versions for the new profile: ${error}`);
  }
}

async function loadLoaders() {
  const select = $("loader-select");
  const mc = $("version-select").value;
  const loaderType = $("mod-loader-select").value || "fabric";
  $("loader-label").textContent = `${loaderLabel(loaderType)} version`;
  select.innerHTML = `<option value="">${loaderType === "forge" ? "Recommended" : "Latest"}</option>`;
  if (mc.startsWith("latest-")) {
    updateSummary();
    return;
  }
  try {
    const loaders = await invoke("loader_versions", { mc, modLoader: loaderType });
    for (const loader of loaders.slice(0, 40)) {
      const option = document.createElement("option");
      option.value = loader;
      option.textContent = loader;
      select.appendChild(option);
    }
    if (
      settings &&
      (settings.mod_loader === loaderType || (!settings.mod_loader && loaderType === "fabric")) &&
      settings.loader
    ) {
      select.value = settings.loader;
    }
  } catch (err) {
    appendConsole(`[launcher] Could not list ${loaderLabel(loaderType)} versions: ` + err);
  }
  updateSummary();
}

/* ---- Accounts ---- */
function activeAccount() {
  return accounts.find((a) => a.active) || null;
}

function renderAccounts(state) {
  accounts = (state && state.accounts ? state.accounts : []).map((a) => ({
    ...a,
    active: state.active === a.id,
  }));

  const active = activeAccount();

  // Topbar button
  $("account-name").textContent = active ? active.name : "Not signed in";
  applyAvatar($("account-avatar"), active);
  $("hero-account").textContent = active
    ? active.name + (active.kind === "msa" ? "" : " (offline)")
    : "Not signed in";
  const dot = $("hero-dot");
  dot.classList.toggle("online", !!(active && active.kind === "msa"));

  const badge = $("conn-badge");
  const online = !!(active && active.kind === "msa");
  badge.dataset.state = online ? "online" : "offline";
  $("conn-text").textContent = active
    ? online
      ? "Signed in"
      : "Offline mode"
    : "Not signed in";

  // Dropdown list
  const list = $("account-list");
  list.innerHTML = "";
  if (accounts.length === 0) {
    const note = document.createElement("p");
    note.className = "muted small-text";
    note.style.textAlign = "center";
    note.textContent = "No accounts yet.";
    list.appendChild(note);
  }
  for (const account of accounts) {
    list.appendChild(accountMenuItem(account));
  }

  // Accounts pane cards
  const cards = $("account-cards");
  cards.innerHTML = "";
  if (accounts.length === 0) {
    const note = document.createElement("div");
    note.className = "empty-note minecraft-inset-box";
    note.textContent = "You are not signed in. Add a Microsoft or offline account.";
    cards.appendChild(note);
  }
  for (const account of accounts) {
    cards.appendChild(accountCard(account));
  }

  if (profilePaneActive()) {
    updateProfile();
  }
}

function accountMenuItem(account) {
  const item = document.createElement("div");
  item.className = "account-item" + (account.active ? " active" : "");
  item.innerHTML = `
    <span class="avatar"></span>
    <span class="account-item-info">
      <span class="account-item-name"></span>
      <span class="account-item-kind"></span>
    </span>
    <button class="account-item-remove" title="Remove">&times;</button>`;
  applyAvatar(item.querySelector(".avatar"), account);
  item.querySelector(".account-item-name").textContent = account.name;
  item.querySelector(".account-item-kind").textContent =
    account.kind === "msa" ? "Microsoft" : "Offline";
  item.addEventListener("click", () => selectAccount(account.id));
  item.querySelector(".account-item-remove").addEventListener("click", (e) => {
    e.stopPropagation();
    removeAccount(account.id);
  });
  return item;
}

function accountCard(account) {
  const card = document.createElement("div");
  card.className = "account-card minecraft-box" + (account.active ? " active" : "");
  card.innerHTML = `
    <span class="avatar"></span>
    <span class="account-card-info">
      <span class="account-card-name"></span>
      <span class="account-card-meta"></span>
    </span>
    <button class="small account-card-remove">Remove</button>`;
  applyAvatar(card.querySelector(".avatar"), account);
  card.querySelector(".account-card-name").textContent = account.name;
  card.querySelector(".account-card-meta").textContent =
    (account.kind === "msa" ? "Microsoft account" : "Offline account") +
    (account.active ? " · Active" : "");
  card.addEventListener("click", () => selectAccount(account.id));
  card.querySelector(".account-card-remove").addEventListener("click", (e) => {
    e.stopPropagation();
    removeAccount(account.id);
  });
  return card;
}

async function refreshAccounts() {
  const state = await invoke("auth_accounts");
  renderAccounts(state);
}

async function selectAccount(id) {
  const active = await invoke("auth_select_account", { id });
  renderAccounts({ active: active ? active.id : null, accounts });
  $("account-menu").hidden = true;
  appendConsole(`[launcher] Switched to ${active ? active.name : "no account"}`);
}

async function removeAccount(id) {
  const state = await invoke("auth_remove_account", { id });
  renderAccounts(state);
}

function closeMenuOnOutsideClick(event) {
  const switcher = $("account-switcher");
  if (!switcher.contains(event.target)) {
    $("account-menu").hidden = true;
  }
}

/* ---- Profile (3D skin viewer) ---- */
let skinViewer = null;
let skinDefaultCam = null;
function profilePaneActive() {
  const pane = document.querySelector('.pane[data-pane="profile"]');
  return !!pane && pane.classList.contains("active");
}

function viewer() {
  if (skinViewer) return skinViewer;
  if (typeof window.skinview3d === "undefined") return null;
  const canvas = $("skin-canvas");
  const view = canvas && canvas.parentElement;
  if (!view || view.clientWidth < 10 || view.clientHeight < 10) return null;
  skinViewer = new window.skinview3d.SkinViewer({
    canvas,
    width: view.clientWidth,
    height: view.clientHeight,
  });
  skinViewer.animation = new window.skinview3d.IdleAnimation();
  skinViewer.controls.enablePan = false;
  skinViewer.controls.minDistance = 40;
  skinViewer.controls.maxDistance = 160;
  skinViewer.autoRotate = false;
  skinDefaultCam = skinViewer.camera.position.clone();
  return skinViewer;
}

function resizeViewer() {
  if (!skinViewer) return;
  const view = document.querySelector(".profile-view");
  if (!view) return;
  if (view.clientWidth < 10 || view.clientHeight < 10) return;
  skinViewer.width = view.clientWidth;
  skinViewer.height = view.clientHeight;
}

function selectSkinAnimation(animation) {
  const v = viewer();
  if (!v) return;
  if (animation === "elytra") {
    const enabled = v.playerObject.backEquipment !== "elytra";
    v.playerObject.backEquipment = enabled
      ? "elytra"
      : currentCapeSource
        ? "cape"
        : null;
    v.animation = enabled
      ? new window.skinview3d.FlyingAnimation()
      : new window.skinview3d.IdleAnimation();
    $("btn-animation-elytra").setAttribute("aria-pressed", String(enabled));
    $("btn-animation-swim").setAttribute("aria-pressed", "false");
    $("btn-animation-idle").setAttribute("aria-pressed", String(!enabled));
    return;
  }

  if (animation === "swim") {
    v.animation = new window.skinview3d.SwimAnimation();
    $("btn-animation-elytra").setAttribute(
      "aria-pressed",
      String(v.playerObject.backEquipment === "elytra")
    );
    $("btn-animation-swim").setAttribute("aria-pressed", "true");
    $("btn-animation-idle").setAttribute("aria-pressed", "false");
    return;
  }

  v.animation = new window.skinview3d.IdleAnimation();
  $("btn-animation-elytra").setAttribute(
    "aria-pressed",
    String(v.playerObject.backEquipment === "elytra")
  );
  $("btn-animation-swim").setAttribute("aria-pressed", "false");
  $("btn-animation-idle").setAttribute("aria-pressed", "true");
}

function renderSkinLibrary() {
  const library = $("skin-library");
  library.replaceChildren();
  for (const skin of localSkins) {
    const card = document.createElement("div");
    card.className = "skin-library-card";
    const button = document.createElement("button");
    button.type = "button";
    button.className = "skin-library-item" + (skin.id === selectedSkinId ? " selected" : "");
    button.setAttribute("aria-pressed", String(skin.id === selectedSkinId));
    button.title = skin.name;

    const face = document.createElement("canvas");
    face.className = "skin-library-face";
    face.width = 40;
    face.height = 40;
    button.appendChild(face);

    const name = document.createElement("span");
    name.className = "skin-library-name";
    name.textContent = skin.name;
    button.appendChild(name);
    button.addEventListener("click", () => {
      selectedSkinId = skin.id;
      renderSkinLibrary();
      previewLocalSkin(skin.path, skin.name);
      $("skin-status").textContent = "Previewing saved skin. Your Minecraft skin is unchanged.";
    });
    card.appendChild(button);

    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "small skin-library-remove";
    remove.textContent = "Remove";
    remove.setAttribute("aria-label", `Remove ${skin.name}`);
    remove.addEventListener("click", async () => {
      remove.disabled = true;
      try {
        await invoke("remove_local_skin", { id: skin.id });
        localSkins = localSkins.filter((entry) => entry.id !== skin.id);
        const wasSelected = selectedSkinId === skin.id;
        if (wasSelected) selectedSkinId = "";
        renderSkinLibrary();
        if (wasSelected) {
          await updateProfile();
        } else {
          $("skin-status").textContent = `${skin.name} removed from your saved skins.`;
        }
      } catch (error) {
        remove.disabled = false;
        $("skin-status").textContent = `Could not remove ${skin.name}: ${error}`;
        appendConsole("[error] Could not remove saved skin: " + error);
      }
    });
    card.appendChild(remove);
    library.appendChild(card);

    const image = new Image();
    image.onload = () => {
      const context = face.getContext("2d");
      if (!context) return;
      context.imageSmoothingEnabled = false;
      context.drawImage(image, 8, 8, 8, 8, 0, 0, 40, 40);
      if (image.width >= 48) {
        context.drawImage(image, 40, 8, 8, 8, 0, 0, 40, 40);
      }
    };
    image.onerror = () => {
      const context = face.getContext("2d");
      if (context) {
        context.fillStyle = "#aa5555";
        context.fillRect(0, 0, face.width, face.height);
      }
    };
    image.src = toMediaUrl(skin.path);
  }

  if (localSkins.length === 0) {
    const empty = document.createElement("p");
    empty.className = "skin-library-empty";
    empty.textContent = "No saved skins yet. Add a skin PNG to start your library.";
    library.appendChild(empty);
  }

  $("btn-skin-classic").classList.toggle("selected", selectedSkinModel === "classic");
  $("btn-skin-slim").classList.toggle("selected", selectedSkinModel === "slim");
}

function drawCurrentSkinFace(source) {
  const canvas = $("current-skin-face");
  const context = canvas.getContext("2d");
  if (!context) return;
  context.clearRect(0, 0, canvas.width, canvas.height);
  if (!source) return;

  const image = new Image();
  image.onload = () => {
    if (currentSkinSource !== source) return;
    context.clearRect(0, 0, canvas.width, canvas.height);
    context.imageSmoothingEnabled = false;
    context.drawImage(image, 8, 8, 8, 8, 0, 0, canvas.width, canvas.height);
    if (image.width >= 48) {
      context.drawImage(image, 40, 8, 8, 8, 0, 0, canvas.width, canvas.height);
    }
  };
  image.onerror = () => {
    context.clearRect(0, 0, canvas.width, canvas.height);
  };
  image.src = source;
}

async function previewCurrentSkin(source, name) {
  currentSkinSource = source || "";
  currentSkinName = name || "No skin selected";
  $("current-skin-name").textContent = currentSkinName;
  drawCurrentSkinFace(currentSkinSource);

  const v = viewer();
  if (!v || !currentSkinSource) return;
  try {
    await v.loadSkin(currentSkinSource, {
      model: selectedSkinModel === "slim" ? "slim" : "default",
    });
    $("profile-empty").hidden = true;
  } catch (error) {
    $("skin-status").textContent = "Could not preview this skin: " + error;
    appendConsole("[launcher] Could not preview saved skin: " + error);
  }
}

async function previewLocalSkin(path, name) {
  return previewCurrentSkin(toMediaUrl(path), name || "Saved skin");
}

async function loadLocalSkins() {
  try {
    localSkins = await invoke("local_skins");
  } catch (error) {
    localSkins = [];
    $("skin-status").textContent = "Could not load saved skins: " + error;
    appendConsole("[launcher] Could not load local skin library: " + error);
  }
  renderSkinLibrary();
}

async function updateProfile() {
  const active = activeAccount();
  const empty = $("profile-empty");

  $("profile-name").textContent = active ? active.name : "Not signed in";
  $("profile-uuid").textContent = active ? active.uuid : "";
  $("profile-kind-label").textContent = active
    ? active.kind === "msa"
      ? "Microsoft account"
      : "Offline account"
    : "";

  const v = viewer();
  if (!v) {
    empty.hidden = false;
    if (active) loadCapes(active);
    return;
  }

  renderSkinLibrary();

  if (!active) {
    $("profile-cape").textContent = "Sign in to load your capes";
    $("cape-list").replaceChildren();
    currentCapeSource = null;
    v.loadCape(null);
    empty.hidden = false;
    const selected = localSkins.find((skin) => skin.id === selectedSkinId);
    if (selected) {
      await previewLocalSkin(selected.path, selected.name);
      $("skin-status").textContent = "Previewing saved skin. Your Minecraft skin is unchanged.";
    } else {
      await previewCurrentSkin("", "No skin selected");
      empty.hidden = false;
      $("skin-status").textContent = "Select a saved skin to preview it here.";
    }
    return;
  }

  try {
    const tex = await invoke("account_textures", {
      uuid: active.uuid,
      offline: active.kind !== "msa",
    });
    const selected = localSkins.find((skin) => skin.id === selectedSkinId);
    if (selected) {
      await previewLocalSkin(selected.path, selected.name);
    } else {
      if (!skinModelOverridden) {
        selectedSkinModel = tex.slim ? "slim" : "classic";
        renderSkinLibrary();
      }
      await previewCurrentSkin(tex.skin || "img/steve.png", tex.skin ? "Account skin" : "Default skin");
    }
    currentCapeSource = tex.cape || null;
    v.loadCape(currentCapeSource);
    empty.hidden = true;
    $("skin-status").textContent = selectedSkinId
      ? "Previewing saved skin. Your Minecraft skin is unchanged."
      : "Account skin is active. Select a saved skin to preview it.";
  } catch (err) {
    appendConsole("[launcher] Could not load skin: " + err);
    try {
      const selected = localSkins.find((skin) => skin.id === selectedSkinId);
      if (selected) {
        await previewLocalSkin(selected.path, selected.name);
      } else {
        await previewCurrentSkin("img/steve.png", "Default skin");
      }
      empty.hidden = true;
      $("skin-status").textContent = selected
        ? "Previewing saved skin. Your Minecraft skin is unchanged."
        : "Account skin is unavailable; showing the default skin.";
    } catch (e) {
      empty.hidden = false;
    }
  }

  loadCapes(active);
}

async function loadCapes(account) {
  const list = $("cape-list");
  list.innerHTML = "";
  $("profile-cape").textContent = "Loading capes\u2026";

  if (account.kind !== "msa") {
    $("profile-cape").textContent = "Capes require a Microsoft account";
    return;
  }

  try {
    const capes = await invoke("account_capes", { uuid: account.uuid });
    if (!capes.length) {
      $("profile-cape").textContent = "This account owns no capes";
      return;
    }
    renderCapes(account, capes);
  } catch (err) {
    $("profile-cape").textContent = "Could not load capes";
    appendConsole("[launcher] Could not load capes: " + err);
  }
}

function renderCapes(account, capes) {
  const list = $("cape-list");
  list.innerHTML = "";

  if (!account || !capes.length) {
    if (account && account.kind === "msa") {
      $("profile-cape").textContent = "This account owns no capes";
    }
    return;
  }

  $("profile-cape").textContent = `${capes.length} cape${
    capes.length === 1 ? "" : "s"
  } owned`;
  const activeCape = capes.find((c) => c.active) || null;

  for (const cape of capes) {
    list.appendChild(capeItem(account, cape, cape === activeCape));
  }
}

function capeItem(account, cape, selected) {
  const item = document.createElement("button");
  item.className = "cape-item" + (selected ? " selected" : "");
  item.title = cape.active ? `${cape.alias} (equipped) - click to remove` : cape.alias;

  const thumb = document.createElement("span");
  thumb.className = "cape-thumb";
  applyCapeCrop(thumb, cape.url);
  item.appendChild(thumb);

  const name = document.createElement("span");
  name.className = "cape-name";
  name.textContent = cape.alias || cape.id;
  item.appendChild(name);

  if (cape.active) {
    const tag = document.createElement("span");
    tag.className = "cape-tag";
    tag.textContent = "Equipped";
    item.appendChild(tag);
  }

  item.addEventListener("click", async () => {
    const v = viewer();
    if (v && cape.url) {
      v.loadCape(cape.url);
    }
    markSelectedCape(item);
    await equipCape(account, cape);
  });
  return item;
}

// Asks Mojang to actually equip the cape (or remove it when it already is the
// active one), then refreshes the list so the "Equipped" badge is truthful.
let capeBusy = false;

async function equipCape(account, cape) {
  if (!account || account.kind !== "msa" || capeBusy) return;
  capeBusy = true;
  const status = $("profile-cape");
  status.textContent = cape.active ? "Removing cape\u2026" : "Equipping cape\u2026";
  try {
    const capes = await invoke("set_cape", {
      uuid: account.uuid,
      capeId: cape.active ? null : cape.id,
    });
    renderCapes(account, capes);
    const nowActive = capes.find((c) => c.active);
    const v = viewer();
    if (v) v.loadCape(nowActive && nowActive.url ? nowActive.url : null);
    appendConsole(
      `[launcher] ${cape.active ? "Removed" : "Equipped"} cape ${
        cape.alias || cape.id
      }`
    );
  } catch (err) {
    status.textContent = "Could not change cape";
    appendConsole("[error] Cape change failed: " + err);
  } finally {
    capeBusy = false;
  }
}

function markSelectedCape(item) {
  const list = $("cape-list");
  list.querySelectorAll(".cape-item").forEach((el) => {
    el.classList.toggle("selected", el === item);
  });
}

// Minecraft cape textures are normalised to a 64x32 grid; the visible cape face
// sits at (1, 1) with a size of 10x16. We crop exactly that region so the
// thumbnail shows the cape itself instead of the whole unfolded texture.
function capeScale(w, h) {
  if (w === 2 * h) return w / 64;
  if (17 * w === 22 * h) return w / 22;
  if (11 * w === 23 * h) return w / 46;
  return 0;
}

function applyCapeCrop(el, url) {
  if (!url) return;
  const probe = new Image();
  probe.onload = () => {
    const w = probe.naturalWidth;
    const h = probe.naturalHeight;
    const i = capeScale(w, h);
    el.style.backgroundImage = `url("${url}")`;
    if (!i) {
      el.style.backgroundSize = "contain";
      el.style.backgroundPosition = "center";
      return;
    }
    const cw = 10 * i;
    const ch = 16 * i;
    const cx = 1 * i;
    const cy = 1 * i;
    el.style.backgroundSize = `${(w / cw) * 100}% ${(h / ch) * 100}%`;
    const px = w - cw > 0 ? (cx / (w - cw)) * 100 : 0;
    const py = h - ch > 0 ? (cy / (h - ch)) * 100 : 0;
    el.style.backgroundPosition = `${px}% ${py}%`;
  };
  probe.src = url;
}

/* ---- Window controls (custom titlebar) ---- */
// These go through our own Rust commands instead of window.__TAURI__.window, so
// the titlebar works no matter what the injected JS API exposes or which window
// capabilities are enabled.
const DRAG_IGNORE = "button, input, select, textarea, a, .account-menu, .window-controls";

function renderModrinthResults(results) {
  const container = $("modrinth-results");
  const contentType = $("mod-content-type").value;
  const content = CONTENT_TYPES[contentType];
  container.replaceChildren();
  if (!results.length) {
    $("modrinth-status").textContent = `No ${content.label} found for these filters.`;
    return;
  }
  $("modrinth-status").textContent =
    `${results.length} compatible ${content.label} found.`;

  for (const mod of results) {
    const card = document.createElement("article");
    card.className = "modrinth-card";

    const icon = document.createElement("img");
    icon.className = "modrinth-icon";
    icon.alt = "";
    if (mod.icon_url && /^https:\/\/[^/]+/i.test(mod.icon_url)) {
      icon.src = mod.icon_url;
      icon.addEventListener("error", () => icon.removeAttribute("src"), { once: true });
    }
    card.appendChild(icon);

    const body = document.createElement("div");
    body.className = "modrinth-card-body";
    const title = document.createElement("h4");
    title.textContent = mod.title;
    body.appendChild(title);

    const description = document.createElement("p");
    description.textContent = mod.description || "No description provided.";
    body.appendChild(description);

    const downloads = document.createElement("p");
    downloads.className = "muted";
    downloads.textContent =
      `${Number(mod.downloads || 0).toLocaleString()} downloads` +
      (contentType === "mod" ? ` · ${loaderLabel($("mod-loader-select").value)}` : "");
    body.appendChild(downloads);

    const install = document.createElement("button");
    install.className = "small primary";
    install.textContent = "Install";
    install.addEventListener("click", () =>
      run(`Installing ${mod.title}`, async () => {
        await saveSettings();
        const destination = await invoke("modrinth_install", {
          projectId: mod.id,
          version: $("version-select").value || settings.version || "latest-release",
          loader: $("mod-loader-select").value || "fabric",
          projectType: contentType,
        });
        install.disabled = true;
        install.textContent = "Installed";
        $("modrinth-status").textContent = `Installed ${content.singular} ${mod.title} (${destination}).`;
        appendConsole(`[launcher] Installed ${mod.title}: ${destination}`);
        await refreshInstalledMods();
      })
    );
    body.appendChild(install);
    card.appendChild(body);
    container.appendChild(card);
  }
}

async function searchModrinth({ force = false } = {}) {
  const query = $("modrinth-query").value.trim();
  const version = $("version-select").value || settings.version || "latest-release";
  const loader = $("mod-loader-select").value || "fabric";
  const projectType = $("mod-content-type").value;
  const category = projectType === "mod" ? $("modrinth-category").value || "" : "";
  const sort = $("modrinth-sort").value || "relevance";
  const key = JSON.stringify([query, version, loader, projectType, category, sort]);
  const token = ++modrinthSearchToken;
  if (!force && modrinthCache.has(key)) {
    renderModrinthResults(modrinthCache.get(key));
    return;
  }
  $("modrinth-status").textContent = query ? "Searching Modrinth…" : "Loading popular mods…";
  $("modrinth-results").replaceChildren();
  try {
    const results = await invoke("modrinth_search", {
      query,
      version,
      loader,
      projectType,
      category,
      sort,
    });
    if (token !== modrinthSearchToken) return;
    modrinthCache.set(key, results);
    renderModrinthResults(results);
  } catch (error) {
    if (token !== modrinthSearchToken) return;
    $("modrinth-status").textContent = "Search failed: " + error;
    appendConsole("[error] Modrinth search failed: " + error);
  }
}

async function refreshInstalledMods() {
  const token = ++installedModsLoadToken;
  const contentType = $("mod-content-type").value;
  const content = CONTENT_TYPES[contentType];
  $("installed-content-title").textContent = `Installed ${content.label}`;
  $("installed-mods-status").textContent = `Loading ${content.label}…`;
  $("installed-mods-list").replaceChildren();
  try {
    const mods = await invoke("list_installed_content", { projectType: contentType });
    if (token !== installedModsLoadToken) return;
    const list = $("installed-mods-list");
    for (const mod of mods) {
      const card = document.createElement("article");
      card.className = "installed-mod-card";

      const icon = document.createElement("div");
      icon.className = "installed-mod-icon";
      icon.setAttribute("aria-hidden", "true");
      icon.textContent = (mod.name || mod.file_name || "?").trim().slice(0, 2).toUpperCase();
      card.appendChild(icon);

      const details = document.createElement("div");
      details.className = "installed-mod-details";
      const title = document.createElement("h4");
      title.textContent = mod.name || mod.file_name;
      details.appendChild(title);
      const version = document.createElement("p");
      version.className = "muted";
      version.textContent = [mod.version, mod.file_name].filter(Boolean).join(" · ");
      details.appendChild(version);
      card.appendChild(details);
      list.appendChild(card);
    }
    const profile = profiles.find((item) => item.id === activeProfileId);
    const loader = profile?.mod_loader || $("mod-loader-select").value;
    $("installed-mods-status").textContent = mods.length
      ? `${mods.length} ${content.label} installed in this profile` +
        (contentType === "mod" ? ` (${loaderLabel(loader)}).` : ".")
      : `No ${content.label} installed in this profile yet. Add ${content.label} below or use Open folder.`;
  } catch (error) {
    if (token !== installedModsLoadToken) return;
    $("installed-mods-status").textContent = `Could not read installed ${content.label}: ${error}`;
    appendConsole(`[launcher] Could not load installed ${content.label}: ${error}`);
  }
}

function initWindowControls() {
  const minimize = $("win-min");
  const maximize = $("win-max");
  const close = $("win-close");
  if (!minimize || !maximize || !close) return;

  minimize.addEventListener("click", (event) => {
    event.preventDefault();
    invoke("win_minimize").catch((error) => appendConsole("[error] " + error));
  });
  maximize.addEventListener("click", (event) => {
    event.preventDefault();
    invoke("win_toggle_maximize").catch((error) => appendConsole("[error] " + error));
  });
  close.addEventListener("click", (event) => {
    event.preventDefault();
    invoke("win_close").catch((error) => appendConsole("[error] " + error));
  });

  const topbar = document.querySelector(".topbar");
  if (!topbar) return;

  // The top bar is the window's title bar. Dragging only arms on the press and
  // starts once the pointer really moves, so plain clicks keep working and a
  // double click still maximises.
  let armed = null;

  topbar.addEventListener("mousedown", (event) => {
    if (event.button !== 0) return;
    if (event.target.closest(DRAG_IGNORE)) return;
    armed = { x: event.clientX, y: event.clientY };
  });

  window.addEventListener("mousemove", (event) => {
    if (!armed) return;
    if (Math.abs(event.clientX - armed.x) + Math.abs(event.clientY - armed.y) < 5) {
      return;
    }
    armed = null;
    invoke("win_start_dragging").catch(() => {});
  });

  window.addEventListener("mouseup", () => {
    armed = null;
  });
  window.addEventListener("blur", () => {
    armed = null;
  });

  topbar.addEventListener("dblclick", (event) => {
    if (event.target.closest(DRAG_IGNORE)) return;
    invoke("win_toggle_maximize").catch(() => {});
  });
}

/* ---- Run helper ---- */
async function run(label, fn) {
  if (busy) return;
  setBusy(true);
  setStatus(label + "\u2026");
  try {
    await fn();
    setStatus("Done");
  } catch (error) {
    setStatus("Error");
    appendConsole("[error] " + error);
  } finally {
    setBusy(false);
    $("progress").hidden = true;
  }
}

function showCrashAnalysis(exitCode) {
  const logs = recentGameLogs.join("\n");
  const lowerLogs = logs.toLowerCase();
  let title = `Minecraft closed unexpectedly (exit code ${exitCode}).`;
  let advice =
    "Check the last red/error lines in the Console tab. Confirm that your mods match this profile's Minecraft version and loader, then try launching again.";
  if (/outofmemoryerror|java heap space|not enough memory/i.test(logs)) {
    title = "Minecraft ran out of memory.";
    advice =
      "In Settings, increase the memory allocation if your PC has enough free RAM. Also remove large resource packs or shaders and close other memory-heavy apps.";
  } else if (/unsupportedclassversionerror|class file version/i.test(logs)) {
    title = "Minecraft was started with an incompatible Java version.";
    advice =
      "Install or select a compatible Java runtime. Newer Minecraft releases generally require a newer Java version; let the launcher auto-detect Java or choose the correct executable in Settings.";
  } else if (/modresolutionexception|requires .* but|incompatible mod set|depends on/i.test(logs)) {
    title = "One or more mods are incompatible.";
    advice =
      "Remove or update the mod named in the Console error. Make sure every installed mod matches this profile's Minecraft version and selected loader.";
  } else if (/mixin apply failed|org\\.spongepowered\\.asm\\.mixin|failed to apply mixin/i.test(logs)) {
    title = "A mod failed to load.";
    advice =
      "The Console indicates a mixin/mod compatibility failure. Update or remove the mod mentioned near the error, and verify that it supports this profile's Minecraft version and loader.";
  } else if (/could not find or load main class|classnotfoundexception/i.test(lowerLogs)) {
    title = "Minecraft could not find a required game or loader class.";
    advice =
      "The installation may be incomplete or a loader component may be missing. Retry Launch to let the launcher repair the profile, and remove any manually added mods that could conflict.";
  }

  const recentErrors = recentGameLogs
    .filter((line) => /error|exception|fatal|crash/i.test(line))
    .slice(-12)
    .join("\n");
  $("crash-title").textContent = title;
  $("crash-advice").textContent = advice;
  $("crash-details").textContent =
    recentErrors || recentGameLogs.slice(-20).join("\n") || "No crash details were captured.";
  const dialog = $("crash-dialog");
  if (!dialog.open) dialog.showModal();
}

/* ---- Tabs ---- */
function activateTab(tab) {
  document.querySelectorAll(".side-btn").forEach((b) => {
    b.classList.toggle("active", b.dataset.tab === tab);
  });
  document.querySelectorAll(".pane").forEach((p) => {
    p.classList.toggle("active", p.dataset.pane === tab);
  });
  const btn = document.querySelector(`.side-btn[data-tab="${tab}"]`);
  $("topbar-tab").textContent = btn ? btn.dataset.label : "";

  if (tab === "profile") {
    requestAnimationFrame(() => {
      resizeViewer();
      updateProfile();
    });
  }
  if (tab === "mods") {
    refreshInstalledMods();
    searchModrinth();
  }
  if (tab === "game-profiles") renderProfiles({ active_profile: activeProfileId, profiles });
}

function initTabs() {
  document.querySelectorAll(".side-btn").forEach((btn) => {
    btn.addEventListener("click", () => activateTab(btn.dataset.tab));
  });
}

/* ---- Events from backend ---- */
function initEvents() {
  let current = 0;
  let total = 0;

  listen("progress", (event) => {
    const { title, message, current: done, total: count } = event.payload;
    $("progress").hidden = false;
    $("progress-title").textContent = title;
    $("progress-message").textContent = message;
    current = done;
    total = count;
    const pct = total > 0 ? Math.min(100, Math.round((current / total) * 100)) : 0;
    $("progress-fill").style.width = pct + "%";
    $("progress-count").textContent = total > 0 ? `${current} / ${total}` : "";
  });

  listen("progress-complete", () => {
    $("progress").hidden = true;
  });

  listen("log", (event) => {
    appendConsole(String(event.payload));
  });

  listen("game-log", (event) => {
    const { pid, line } = event.payload;
    const source = String(pid);
    if (!consoleInstances.has(source)) {
      consoleInstances.set(source, {
        profile_name: "Minecraft",
        version: "",
        loader: "",
      });
      renderConsoleSources();
    }
    appendConsole(String(line), source);
  });

  listen("game-start", (event) => {
    const game = event.payload;
    const pid = String(game.pid);
    runningGames.set(pid, game);
    consoleInstances.set(pid, {
      ...consoleInstances.get(pid),
      ...game,
    });
    selectedConsoleId = pid;
    renderRunningGames();
    setStatus(`${runningGames.size} game instance(s) running`);
    appendConsole(
      `[launcher] ${game.profile_name} started (${game.version}, ${loaderLabel(game.loader)}).`
    );
    if (settings?.hide_after_launch) {
      invoke("hide_window").catch((error) => appendConsole("[error] Could not hide launcher: " + error));
    }
  });

  listen("game-exit", (event) => {
    const result =
      typeof event.payload === "number"
        ? { code: event.payload, success: event.payload === 0 }
        : event.payload;
    const code = Number(result.code);
    const pid = String(result.pid);
    runningGames.delete(pid);
    renderRunningGames();
    appendConsole(`[launcher] ${result.profile_name || "Minecraft"} exited (code ${code})`);
    setStatus(runningGames.size ? `${runningGames.size} game instance(s) running` : "Game closed");
    $("progress").hidden = true;
    if (!result.stopped && (result.success === false || code !== 0)) showCrashAnalysis(code);
  });

  listen("auth-code", (event) => {
    const { user_code, verification_uri, message } = event.payload;
    const box = $("device-code");
    if (!user_code) {
      box.hidden = true;
      return;
    }
    box.hidden = false;
    $("user-code").textContent = user_code;
    $("verify-link").textContent = verification_uri;
    $("verify-link").href = verification_uri;
    appendConsole(message);
  });
}

/* ---- Buttons ---- */
function initButtons() {
  $("btn-save").addEventListener("click", () => saveSettings());
  $("btn-reset-settings").addEventListener("click", async () => {
    if (!window.confirm("Reset launcher settings to their defaults? Profiles, accounts, and game files will not be changed.")) return;
    const button = $("btn-reset-settings");
    button.disabled = true;
    try {
      settings = await invoke("reset_settings");
      fillSettingsForm(settings);
      $("mod-loader-select").value = settings.mod_loader || "fabric";
      await loadVersions();
      await loadLauncherNews();
      updateSummary();
      setStatus("Settings reset to defaults");
    } catch (error) {
      appendConsole("[error] Could not reset settings: " + error);
      setStatus("Could not reset settings");
    } finally {
      button.disabled = false;
    }
  });
  $("btn-refresh-news").addEventListener("click", () => loadLauncherNews());
  $("set-news-enabled").addEventListener("change", () => {
    settings = { ...(settings || {}), news_enabled: $("set-news-enabled").checked };
    void loadLauncherNews();
  });

  $("play-profile-select").addEventListener("change", (event) => selectActiveProfile(event.target.value));
  $("btn-manage-profiles").addEventListener("click", () => activateTab("game-profiles"));
  $("btn-upload-new-profile-icon").addEventListener("click", () => uploadProfileIcon());
  $("new-profile-version").addEventListener("change", () => loadNewProfileLoaderVersions());
  $("new-profile-loader").addEventListener("change", () => loadNewProfileLoaderVersions());

  $("profile-create-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const name = $("new-profile-name").value.trim();
    if (!name) return;
    const button = $("btn-create-profile");
    button.disabled = true;
    try {
      await saveSettings();
      const state = await invoke("create_profile", {
        name,
        version: $("new-profile-version").value || "latest-release",
        modLoader: $("new-profile-loader").value || "fabric",
        loader: $("new-profile-loader-version").value || "",
        avatar: "grass",
        avatarData: newProfileAvatarData,
      });
      $("new-profile-name").value = "";
      newProfileAvatarData = null;
      $("new-profile-icon-name").textContent = "Default Melon Client logo";
      renderProfiles(state);
      await reloadActiveProfile();
      setStatus("Profile created");
    } catch (error) {
      appendConsole("[error] Could not create profile: " + error);
      setStatus("Profile creation failed");
    } finally {
      button.disabled = false;
    }
  });

  $("btn-launch").addEventListener("click", () => {
    run("Launching", async () => {
      await saveSettings();
      $("progress").hidden = true;
      appendConsole("[launcher] Launching\u2026");
      await invoke("launch");
    }).catch(() => {});
  });
  $("btn-stop-game").addEventListener("click", async () => {
    const button = $("btn-stop-game");
    const pid = Number($("running-game-select").value);
    if (!Number.isSafeInteger(pid) || pid <= 0) {
      setStatus("Select a running game instance first");
      return;
    }
    button.disabled = true;
    try {
      await invoke("stop_game", { pid });
      const game = runningGames.get(String(pid));
      setStatus(`Stopping ${game?.profile_name || "Minecraft"}…`);
    } catch (error) {
      appendConsole("[error] Could not stop Minecraft: " + error);
      setStatus("Could not stop game");
    } finally {
      button.disabled = false;
    }
  });
  $("btn-crash-close").addEventListener("click", () => $("crash-dialog").close());

  const signInMicrosoft = () =>
    run("Waiting for Microsoft sign-in", async () => {
      const account = await invoke("auth_login_browser");
      await refreshAccounts();
      $("device-code").hidden = true;
      appendConsole(`[launcher] Signed in as ${account.name}`);
    });

  const signInDevice = () =>
    run("Waiting for Microsoft sign-in", async () => {
      const account = await invoke("auth_login_microsoft");
      await refreshAccounts();
      $("device-code").hidden = true;
      appendConsole(`[launcher] Signed in as ${account.name}`);
    });

  const addOffline = (input) =>
    run("Adding offline account", async () => {
      const account = await invoke("auth_login_offline", { username: input.value });
      input.value = "";
      await refreshAccounts();
      appendConsole(`[launcher] Added offline account ${account.name}`);
    });

  $("btn-msa").addEventListener("click", signInMicrosoft);
  $("btn-add-msa").addEventListener("click", signInMicrosoft);
  $("btn-msa-device").addEventListener("click", signInDevice);
  $("btn-offline").addEventListener("click", () => addOffline($("offline-name")));
  $("btn-add-offline").addEventListener("click", () =>
    addOffline($("account-offline-name"))
  );

  $("account-btn").addEventListener("click", (event) => {
    event.stopPropagation();
    const menu = $("account-menu");
    menu.hidden = !menu.hidden;
  });
  document.addEventListener("click", closeMenuOnOutsideClick);

  $("btn-open-mods").addEventListener("click", async () => {
    await saveSettings();
    const base = ($("set-mcdir").value || "").replace(/[\\/]+$/, "");
    const folder = CONTENT_TYPES[$("mod-content-type").value].directory;
    invoke("open_folder", { path: base + `\\${folder}` }).catch((e) =>
      appendConsole("[error] " + e)
    );
  });

  $("btn-select-skin").addEventListener("click", async () => {
    try {
      const path = await invoke("pick_skin_file");
      if (!path) return;
      await saveSettings();
      const skin = await invoke("import_local_skin", { source: path, name: "" });
      localSkins.push(skin);
      selectedSkinId = skin.id;
      renderSkinLibrary();
      await previewLocalSkin(skin.path, skin.name);
      $("skin-status").textContent = "Skin saved. It is a launcher preview only and does not change your Minecraft skin.";
      appendConsole(`[launcher] Added ${skin.name} to the local skin library.`);
    } catch (error) {
      appendConsole("[error] " + error);
      $("skin-status").textContent = "Could not add that skin: " + error;
    }
  });

  $("skin-username-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    const username = $("skin-username").value.trim();
    if (!username) {
      $("skin-username-status").textContent = "Enter a Minecraft username first.";
      return;
    }

    const button = $("btn-download-skin");
    button.disabled = true;
    $("skin-username-status").textContent = `Looking up ${username}…`;
    try {
      await saveSettings();
      const skin = await invoke("download_skin_by_username", { username });
      localSkins.push(skin);
      selectedSkinId = skin.id;
      renderSkinLibrary();
      await previewLocalSkin(skin.path, skin.name);
      $("skin-username-status").textContent = `Saved ${skin.name}'s skin to your local library.`;
      appendConsole(`[launcher] Downloaded the public skin for ${username}.`);
    } catch (error) {
      $("skin-username-status").textContent = "Could not download that skin: " + error;
      appendConsole("[launcher] Could not download username skin: " + error);
    } finally {
      button.disabled = false;
    }
  });

  $("btn-skin-classic").addEventListener("click", () => {
    selectedSkinModel = "classic";
    skinModelOverridden = true;
    renderSkinLibrary();
    if (currentSkinSource) previewCurrentSkin(currentSkinSource, currentSkinName);
  });

  $("btn-skin-slim").addEventListener("click", () => {
    selectedSkinModel = "slim";
    skinModelOverridden = true;
    renderSkinLibrary();
    if (currentSkinSource) previewCurrentSkin(currentSkinSource, currentSkinName);
  });

  $("modrinth-search-form").addEventListener("submit", async (event) => {
    event.preventDefault();
    await saveSettings();
    await searchModrinth({ force: true });
  });
  $("mod-content-type").addEventListener("change", () => {
    const isModSearch = $("mod-content-type").value === "mod";
    $("modrinth-category").closest(".field").hidden = !isModSearch;
    modrinthCache.clear();
    refreshInstalledMods();
    searchModrinth({ force: true });
    const content = CONTENT_TYPES[$("mod-content-type").value];
    $("btn-open-mods").textContent = `Open ${content.label} folder`;
  });
  $("modrinth-category").addEventListener("change", () => searchModrinth({ force: true }));
  $("modrinth-sort").addEventListener("change", () => searchModrinth({ force: true }));

  $("btn-browse-bg").addEventListener("click", async () => {
    try {
      const path = await invoke("pick_background_file");
      if (!path) return;
      $("set-bg-source").value = path;
      const type = $("set-bg-type").value;
      if (type !== "picture" && type !== "video") {
        $("set-bg-type").value = /\.(mp4|webm|mkv|mov|m4v|avi)$/i.test(path) ? "video" : "picture";
      }
      previewBackground();
    } catch (error) {
      appendConsole("[error] " + error);
    }
  });

  $("btn-apply-bg").addEventListener("click", () => previewBackground());
  $("set-bg-type").addEventListener("change", () => previewBackground());
  $("set-bg-fit").addEventListener("change", () => previewBackground());
  $("set-bg-source").addEventListener("change", () => previewBackground());
  $("set-bg-mute").addEventListener("change", () => previewBackground());
  $("btn-bg-loop").addEventListener("click", () => {
    const enabled = $("btn-bg-loop").getAttribute("aria-pressed") !== "true";
    setBackgroundLoopButton(enabled);
    previewBackground();
  });

  $("set-bg-dim").addEventListener("input", (event) => {
    $("bg-dim-label").textContent = event.target.value;
    $("bg-dim").style.opacity = String(Number(event.target.value) / 100);
  });
  $("set-bg-blur").addEventListener("input", (event) => {
    $("bg-blur-label").textContent = event.target.value;
    const value = Number(event.target.value);
    const background = $("background");
    background.classList.toggle("blurred", value > 0);
    background.style.setProperty("--bg-blur", `${value}px`);
  });
  $("set-bg-panel-transparency").addEventListener("input", (event) => {
    $("bg-panel-transparency-label").textContent = event.target.value;
    previewBackground();
  });
  $("set-bg-volume").addEventListener("input", (event) => {
    $("bg-volume-label").textContent = event.target.value;
    previewBackground();
  });

  window.addEventListener("resize", () => {
    resizeBackground($("set-bg-fit").value);
  });

  $("btn-clear").addEventListener("click", () => {
    const lines = consoleLogs.get(selectedConsoleId);
    if (lines) lines.length = 0;
    renderConsole();
  });
  $("console-instance-select").addEventListener("change", (event) => {
    selectedConsoleId = event.target.value;
    renderConsole();
  });

  $("set-memory").addEventListener("input", (event) => {
    $("mem-label").textContent = event.target.value;
  });

  $("version-select").addEventListener("change", () => loadLoaders());
  $("loader-select").addEventListener("change", () => {
    if (settings) settings.loader = $("loader-select").value;
    updateSummary();
    modrinthCache.clear();
    if (document.querySelector('.pane[data-pane="mods"]').classList.contains("active")) {
      searchModrinth({ force: true });
    }
  });
  $("version-select").addEventListener("change", () => {
    modrinthCache.clear();
    if (document.querySelector('.pane[data-pane="mods"]').classList.contains("active")) {
      searchModrinth({ force: true });
    }
  });
  $("mod-loader-select").addEventListener("change", () => {
    if (settings) settings.mod_loader = $("mod-loader-select").value;
    if (settings) settings.loader = "";
    $("loader-select").innerHTML = `<option value="">${$("mod-loader-select").value === "forge" ? "Recommended" : "Latest"}</option>`;
    $("loader-select").value = "";
    loadLoaders();
    updateSummary();
    modrinthCache.clear();
    if (document.querySelector('.pane[data-pane="mods"]').classList.contains("active")) {
      refreshInstalledMods();
      searchModrinth({ force: true });
    }
  });

  $("btn-refresh-mods").addEventListener("click", refreshInstalledMods);

  $("btn-skin-spin").addEventListener("click", () => {
    const v = viewer();
    if (v) v.autoRotate = !v.autoRotate;
  });

  $("btn-skin-reset").addEventListener("click", () => {
    const v = viewer();
    if (!v || !skinDefaultCam) return;
    v.camera.position.copy(skinDefaultCam);
    v.controls.target.set(0, 0, 0);
    v.controls.update();
  });

  $("btn-animation-elytra").addEventListener("click", () => selectSkinAnimation("elytra"));
  $("btn-animation-swim").addEventListener("click", () => selectSkinAnimation("swim"));
  $("btn-animation-idle").addEventListener("click", () => selectSkinAnimation("idle"));

  window.addEventListener("resize", resizeViewer);
}

/* ---- Click sound ---- */
function playClick() {
  try {
    const audio = new Audio("sounds/click.ogg");
    audio.volume = 0.4;
    audio.play().catch(() => {});
  } catch (e) {}
}

document.addEventListener(
  "mousedown",
  (event) => {
    if (event.target.closest("button, .side-btn, .account-item, .account-card")) {
      playClick();
    }
  },
  { passive: true }
);

/* ---- Init ---- */
async function init() {
  initTabs();
  initEvents();
  initButtons();
  initWindowControls();
  try {
    renderProfiles(await invoke("list_profiles"));
    settings = await invoke("get_settings");
  } catch (error) {
    settings = {};
    appendConsole("[error] Could not load settings: " + error);
  }
  try {
    const games = await invoke("list_running_games");
    for (const game of games) {
      const pid = String(game.pid);
      runningGames.set(pid, game);
      consoleInstances.set(pid, game);
      if (!consoleLogs.has(pid)) consoleLogs.set(pid, []);
    }
    if (games.length) {
      selectedConsoleId = String(games[games.length - 1].pid);
      renderRunningGames();
      setStatus(`${games.length} game instance(s) running`);
    }
  } catch (error) {
    appendConsole("[error] Could not restore running Minecraft instances: " + error);
  }
  fillSettingsForm(settings);
  $("mod-loader-select").value = settings.mod_loader || "fabric";
  await loadLocalSkins();
  try {
    await refreshAccounts();
  } catch (error) {
    appendConsole("[error] Could not load accounts: " + error);
  }
  await loadVersions();
  void searchModrinth();
  await loadLauncherNews();
  updateSummary();
  activateTab("play");
}

window.addEventListener("DOMContentLoaded", init);
