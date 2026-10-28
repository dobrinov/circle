//! Chrome extensions: which are installed, what their manifests say, and
//! installing and updating them from the Chrome Web Store.
//!
//! Circle keeps its own list (`extensions/extensions.json` in the profile)
//! and loads each enabled extension into Chromium at startup through the
//! engine pipe (see `engine.rs`). Store downloads are CRX3 files: Circle
//! checks both the developer's signature (which fixes the extension's ID)
//! and the Chrome Web Store's own signature before unpacking one.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex},
};

/// Circle's built-in helper extension (see `resources/ext-bridge`), whose
/// fixed key gives it this ID.
pub const BRIDGE_ID: &str = "aiadcoppbgnpndjdblkfhipcmdefofdp";
const BRIDGE_FILES: &[(&str, &str)] = &[
    (
        "manifest.json",
        include_str!("../resources/ext-bridge/manifest.json"),
    ),
    ("cs.js", include_str!("../resources/ext-bridge/cs.js")),
    ("sw.js", include_str!("../resources/ext-bridge/sw.js")),
];
/// The console message the bridge's content script sends with a page's
/// Chrome tab id.
pub const TAB_PING: &str = "__circle_tab__ ";

const LIST: &str = "extensions.json";
/// SHA-256 of the Chrome Web Store's publisher key: every CRX it serves is
/// also signed with it.
const PUBLISHER_KEY_HASH: [u8; 32] = [
    0x61, 0xf7, 0xf2, 0xa6, 0xbf, 0xcf, 0x74, 0xcd, 0x0b, 0xc1, 0xfe, 0x24, 0x97, 0xcc, 0x9b, 0x04,
    0x25, 0x4c, 0x65, 0x8f, 0x79, 0xf2, 0x14, 0x53, 0x92, 0x86, 0x7e, 0xa8, 0x36, 0x63, 0x67, 0xcf,
];
/// The Chromium version Circle tells the store it runs.
const PRODVERSION: &str = "154.0.0.0";
const MAX_CRX_BYTES: u64 = 256 << 20;
const MAX_UNPACKED_BYTES: u64 = 1 << 30;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Extension {
    pub id: String,
    pub version: String,
    /// Its files: Circle's copy of a store download, or a folder the person
    /// loaded unpacked (which stays where it is).
    pub path: PathBuf,
    pub from_store: bool,
    pub enabled: bool,
    pub pinned: bool,
    pub installed: u64,
}

/// The installed extensions.
pub struct Registry {
    dir: PathBuf,
    pub items: Vec<Extension>,
}

impl Registry {
    pub fn load(profile_dir: &Path) -> Self {
        let dir = profile_dir.join("extensions");
        let items = fs::read(dir.join(LIST))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let registry = Self { dir, items };
        registry.publish_icons();
        registry
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn save(&self) -> Result<(), String> {
        fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let tmp = self.dir.join(format!("{LIST}.tmp"));
        let json = serde_json::to_vec_pretty(&self.items).map_err(|e| e.to_string())?;
        fs::write(&tmp, json).map_err(|e| e.to_string())?;
        fs::rename(&tmp, self.dir.join(LIST)).map_err(|e| e.to_string())?;
        self.publish_icons();
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Extension> {
        self.items.iter().find(|e| e.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Extension> {
        self.items.iter_mut().find(|e| e.id == id)
    }

    /// Adds or replaces an extension, keeping its place and settings.
    pub fn put(&mut self, extension: Extension) {
        match self.items.iter_mut().find(|e| e.id == extension.id) {
            Some(existing) => {
                existing.version = extension.version;
                existing.path = extension.path;
                existing.from_store = extension.from_store;
            }
            None => self.items.push(extension),
        }
    }

    pub fn remove(&mut self, id: &str) -> Option<Extension> {
        let index = self.items.iter().position(|e| e.id == id)?;
        let removed = self.items.remove(index);
        // Circle's own copy goes too; a folder loaded unpacked stays.
        if removed.from_store {
            let _ = fs::remove_dir_all(self.dir.join(&removed.id));
        }
        Some(removed)
    }

    /// Removes store copies no longer in use: older versions and failed
    /// installs.
    pub fn tidy(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(extension) = self.get(&name).filter(|e| e.from_store) else {
                if is_id(&name) {
                    let _ = fs::remove_dir_all(&path);
                }
                continue;
            };
            for version in fs::read_dir(&path).into_iter().flatten().flatten() {
                if version.path() != extension.path {
                    let _ = fs::remove_dir_all(version.path());
                }
            }
        }
    }

    /// Writes the bridge extension's files and returns its folder.
    pub fn write_bridge(&self) -> Result<PathBuf, String> {
        let dir = self.dir.join(".bridge");
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for (name, contents) in BRIDGE_FILES {
            let path = dir.join(name);
            if fs::read_to_string(&path).ok().as_deref() != Some(contents) {
                fs::write(&path, contents).map_err(|e| e.to_string())?;
            }
        }
        Ok(dir)
    }

    /// Tells the `circle://ui/ext-icon/` handler where icons are.
    fn publish_icons(&self) {
        let mut icons = ICONS.lock().unwrap();
        icons.clear();
        for extension in &self.items {
            if let Ok(manifest) = Manifest::read(&extension.path)
                && let Some(icon) = manifest.icon
            {
                icons.insert(extension.id.clone(), extension.path.join(icon));
            }
        }
    }
}

/// Extension icons by ID, for `circle://ui/ext-icon/<id>` (read on the IO
/// thread).
static ICONS: LazyLock<Mutex<HashMap<String, PathBuf>>> = LazyLock::new(Default::default);

/// Shows an icon (of an extension about to be installed) before it's in the
/// list.
pub fn publish_icon(id: &str, path: PathBuf) {
    ICONS.lock().unwrap().insert(id.to_owned(), path);
}

/// The icon file for `circle://ui/ext-icon/<id>`.
pub fn icon_file(id: &str) -> Option<PathBuf> {
    ICONS.lock().unwrap().get(id).cloned()
}

/// What Circle needs from an extension's `manifest.json`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Manifest {
    pub manifest_version: u64,
    pub name: String,
    pub description: String,
    pub version: String,
    /// An icon file, relative to the extension's folder.
    pub icon: Option<String>,
    /// The toolbar button's popup page and tooltip.
    pub popup: Option<String>,
    pub action_title: Option<String>,
    pub options: Option<String>,
    /// What it can do, in words (like Chrome's install prompt).
    pub warnings: Vec<String>,
}

impl Manifest {
    pub fn read(dir: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(dir.join("manifest.json"))
            .map_err(|e| format!("no manifest.json: {e}"))?;
        // Some manifests start with a byte-order mark.
        let json: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("manifest.json isn't valid: {e}"))?;
        Ok(Self::parse(&json, |key| message(dir, &json, key)))
    }

    fn parse(json: &Value, localize: impl Fn(&str) -> Option<String>) -> Self {
        let text = |v: &Value| {
            let s = v.as_str().unwrap_or("").trim();
            match s.strip_prefix("__MSG_").and_then(|k| k.strip_suffix("__")) {
                Some(key) => localize(key).unwrap_or_default(),
                None => s.to_owned(),
            }
        };
        let action = json
            .get("action")
            .or_else(|| json.get("browser_action"))
            .or_else(|| json.get("page_action"));
        let action_icon = action.and_then(|a| a.get("default_icon"));
        let icon = pick_icon(action_icon).or_else(|| pick_icon(json.get("icons")));
        let nonempty = |s: String| (!s.is_empty()).then_some(s);
        Self {
            manifest_version: json["manifest_version"].as_u64().unwrap_or(0),
            name: text(&json["name"]),
            description: text(&json["description"]),
            version: json["version"].as_str().unwrap_or("").to_owned(),
            icon,
            popup: action
                .and_then(|a| a["default_popup"].as_str())
                .map(str::to_owned)
                .and_then(nonempty),
            action_title: action.map(|a| text(&a["default_title"])).and_then(nonempty),
            options: json["options_ui"]["page"]
                .as_str()
                .or_else(|| json["options_page"].as_str())
                .map(str::to_owned)
                .and_then(nonempty),
            warnings: warnings(json),
        }
    }
}

/// A `__MSG_key__` string from the extension's default locale.
fn message(dir: &Path, manifest: &Value, key: &str) -> Option<String> {
    let mut locales: Vec<String> = Vec::new();
    if let Some(default) = manifest["default_locale"].as_str() {
        locales.push(default.to_owned());
    }
    locales.extend(["en".to_owned(), "en_US".to_owned()]);
    for locale in locales {
        if locale.contains(['/', '\\', '.']) {
            continue;
        }
        let path = dir.join("_locales").join(&locale).join("messages.json");
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let Ok(Value::Object(messages)) = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        else {
            continue;
        };
        if let Some((_, entry)) = messages.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)) {
            return entry["message"].as_str().map(str::to_owned);
        }
    }
    None
}

/// The best icon for a toolbar button: the smallest of at least 32px, or
/// else the largest.
fn pick_icon(icons: Option<&Value>) -> Option<String> {
    match icons? {
        Value::String(path) => Some(clean_path(path)?),
        Value::Object(map) => {
            let mut sizes: Vec<(u32, &str)> = map
                .iter()
                .filter_map(|(size, path)| Some((size.parse().ok()?, path.as_str()?)))
                .collect();
            sizes.sort();
            let pick = sizes
                .iter()
                .find(|(size, _)| *size >= 32)
                .or(sizes.last())?;
            clean_path(pick.1)
        }
        _ => None,
    }
}

/// A path inside the extension's folder, or nothing.
fn clean_path(path: &str) -> Option<String> {
    let path = path.trim_start_matches('/');
    let ok = !path.is_empty()
        && !path.contains('\\')
        && !path.split('/').any(|part| part == ".." || part.is_empty());
    ok.then(|| path.to_owned())
}

/// What an extension's permissions let it do, in the words Chrome uses.
fn warnings(json: &Value) -> Vec<String> {
    let list = |key: &str| -> Vec<String> {
        json[key]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let permissions = list("permissions");
    let mut hosts = list("host_permissions");
    // Manifest V2 listed hosts among the permissions.
    hosts.extend(permissions.iter().filter(|p| is_host_pattern(p)).cloned());
    for script in json["content_scripts"].as_array().into_iter().flatten() {
        for pattern in script["matches"].as_array().into_iter().flatten() {
            if let Some(p) = pattern.as_str() {
                hosts.push(p.to_owned());
            }
        }
    }
    let has = |name: &str| permissions.iter().any(|p| p == name);
    let mut out = Vec::new();
    let all_sites = hosts.iter().any(|h| {
        h == "<all_urls>"
            || h.starts_with("*://*/")
            || h.starts_with("http://*/")
            || h.starts_with("https://*/")
    });
    if all_sites || has("debugger") || has("proxy") {
        out.push("Read and change all your data on all websites".to_owned());
    } else {
        let mut sites: Vec<String> = hosts
            .iter()
            .filter_map(|h| {
                let host = h.split("://").nth(1)?.split('/').next()?;
                Some(host.trim_start_matches("*.").to_owned())
            })
            .filter(|h| !h.is_empty())
            .collect();
        sites.sort();
        sites.dedup();
        match sites.len() {
            0 => {}
            1 => out.push(format!("Read and change your data on {}", sites[0])),
            2..=3 => out.push(format!("Read and change your data on {}", sites.join(", "))),
            n => out.push(format!("Read and change your data on {n} websites")),
        }
    }
    let table: &[(&str, &str)] = &[
        ("debugger", "Access the page debugger backend"),
        ("history", "Read and change your browsing history"),
        ("bookmarks", "Read and change your bookmarks"),
        ("downloads", "Manage your downloads"),
        ("management", "Manage your apps, extensions, and themes"),
        (
            "nativeMessaging",
            "Communicate with cooperating native applications",
        ),
        ("privacy", "Change your privacy-related settings"),
        (
            "contentSettings",
            "Change settings that control websites' access to features",
        ),
        ("geolocation", "Detect your physical location"),
        ("clipboardRead", "Read data you copy and paste"),
        ("clipboardWrite", "Modify data you copy and paste"),
        ("notifications", "Display notifications"),
        (
            "topSites",
            "Read a list of your most frequently visited websites",
        ),
        ("declarativeNetRequest", "Block content on any page"),
        ("desktopCapture", "Capture content of your screen"),
        ("tabCapture", "Read and change content of your tabs"),
        ("pageCapture", "Read and change content of your tabs"),
    ];
    for (permission, warning) in table {
        if has(permission) {
            out.push((*warning).to_owned());
        }
    }
    let reads_history = ["tabs", "webNavigation", "declarativeNetRequestFeedback"]
        .iter()
        .any(|p| has(p));
    if reads_history && !has("history") && !all_sites {
        out.push("Read your browsing history".to_owned());
    }
    out.dedup();
    out
}

fn is_host_pattern(permission: &str) -> bool {
    permission == "<all_urls>" || permission.contains("://")
}

/// Whether `text` looks like an extension ID: 32 letters from a to p.
pub fn is_id(text: &str) -> bool {
    text.len() == 32 && text.bytes().all(|b| (b'a'..=b'p').contains(&b))
}

/// The ID Chromium gives an extension with this public key (DER).
pub fn id_from_key(key: &[u8]) -> String {
    Sha256::digest(key)[..16]
        .iter()
        .flat_map(|b| [b >> 4, b & 15])
        .map(|n| (b'a' + n) as char)
        .collect()
}

/// The extension a Chrome Web Store page is about.
pub fn store_id(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let mut path = url.path_segments()?.filter(|s| !s.is_empty());
    match url.host_str()? {
        "chromewebstore.google.com" => {
            if path.next()? != "detail" {
                return None;
            }
        }
        "chrome.google.com" => {
            if path.next()? != "webstore" || path.next()? != "detail" {
                return None;
            }
        }
        _ => return None,
    }
    path.take(2).find(|s| is_id(s)).map(str::to_owned)
}

pub fn store_page(id: &str) -> String {
    format!("https://chromewebstore.google.com/detail/{id}")
}

// ----- CRX3 -----------------------------------------------------------------

/// Reads a protobuf message's length-delimited fields (all CRX3 needs).
fn proto_fields(mut data: &[u8]) -> Result<Vec<(u64, &[u8])>, String> {
    fn varint(data: &mut &[u8]) -> Result<u64, String> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let (&byte, rest) = data.split_first().ok_or("truncated CRX header")?;
            *data = rest;
            value |= u64::from(byte & 0x7f) << shift;
            if byte < 0x80 {
                return Ok(value);
            }
        }
        Err("bad CRX header".into())
    }
    let mut fields = Vec::new();
    while !data.is_empty() {
        let key = varint(&mut data)?;
        match key & 7 {
            0 => {
                varint(&mut data)?;
            }
            2 => {
                let len = varint(&mut data)? as usize;
                if len > data.len() {
                    return Err("truncated CRX header".into());
                }
                fields.push((key >> 3, &data[..len]));
                data = &data[len..];
            }
            _ => return Err("bad CRX header".into()),
        }
    }
    Ok(fields)
}

/// The verified contents of a CRX3 file.
pub struct Crx<'a> {
    pub id: String,
    /// The developer's public key (DER), which becomes the manifest's `key`.
    pub key: Vec<u8>,
    pub zip: &'a [u8],
}

/// Checks a CRX3 file from the Chrome Web Store: it must be signed by the
/// key its ID comes from, and by the store.
pub fn verify_crx(bytes: &[u8]) -> Result<Crx<'_>, String> {
    if bytes.len() < 12 || &bytes[..4] != b"Cr24" {
        return Err("not an extension package".into());
    }
    let version = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
    if version != 3 {
        return Err(format!("unsupported package version {version}"));
    }
    let header_len = u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize;
    let header = bytes
        .get(12..12 + header_len)
        .ok_or("truncated extension package")?;
    let zip = &bytes[12 + header_len..];
    let fields = proto_fields(header)?;
    let signed_data = fields
        .iter()
        .find(|(n, _)| *n == 10000)
        .map(|(_, d)| *d)
        .ok_or("the package isn't signed")?;
    let crx_id = proto_fields(signed_data)?
        .into_iter()
        .find(|(n, _)| *n == 1)
        .map(|(_, d)| d.to_vec())
        .ok_or("the package has no ID")?;
    if crx_id.len() != 16 {
        return Err("the package has a bad ID".into());
    }
    let mut message = b"CRX3 SignedData\x00".to_vec();
    message.extend_from_slice(&(signed_data.len() as u32).to_le_bytes());
    message.extend_from_slice(signed_data);
    message.extend_from_slice(zip);

    let mut developer = None;
    let mut publisher = false;
    for (kind, proof) in fields.iter().filter(|(n, _)| *n == 2 || *n == 3) {
        let parts = proto_fields(proof)?;
        let get = |n| parts.iter().find(|(k, _)| *k == n).map(|(_, d)| *d);
        let (Some(key), Some(signature)) = (get(1), get(2)) else {
            continue;
        };
        let hash = Sha256::digest(key);
        let is_developer = hash[..16] == crx_id[..];
        let is_publisher = hash[..] == PUBLISHER_KEY_HASH;
        if !is_developer && !is_publisher {
            continue;
        }
        let valid = if *kind == 2 {
            verify_rsa(key, signature, &message)
        } else {
            verify_ecdsa(key, signature, &message)
        };
        if !valid {
            return Err("the package's signature doesn't match".into());
        }
        if is_developer {
            developer = Some(key.to_vec());
        }
        publisher |= is_publisher;
    }
    let key = developer.ok_or("the package isn't signed by its developer")?;
    if !publisher {
        return Err("the package isn't from the Chrome Web Store".into());
    }
    Ok(Crx {
        id: id_from_key(&key),
        key,
        zip,
    })
}

fn verify_rsa(key: &[u8], signature: &[u8], message: &[u8]) -> bool {
    use rsa::{RsaPublicKey, pkcs1v15::Pkcs1v15Sign, pkcs8::DecodePublicKey};
    let Ok(key) = RsaPublicKey::from_public_key_der(key) else {
        return false;
    };
    key.verify(
        Pkcs1v15Sign::new::<Sha256>(),
        &Sha256::digest(message),
        signature,
    )
    .is_ok()
}

fn verify_ecdsa(key: &[u8], signature: &[u8], message: &[u8]) -> bool {
    use p256::ecdsa::{DerSignature, VerifyingKey, signature::Verifier};
    use p256::pkcs8::DecodePublicKey;
    let (Ok(key), Ok(signature)) = (
        VerifyingKey::from_public_key_der(key),
        DerSignature::from_bytes(signature),
    ) else {
        return false;
    };
    key.verify(message, &signature).is_ok()
}

/// Unpacks a verified CRX into `extensions/<id>/<version>/`, adding the
/// developer's key to the manifest so the extension keeps its store ID.
pub fn unpack(crx: &Crx, extensions_dir: &Path) -> Result<(PathBuf, Manifest), String> {
    let home = extensions_dir.join(&crx.id);
    let staging = home.join(format!(".staging-{}", std::process::id()));
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let result = (|| {
        extract_zip(crx.zip, &staging)?;
        let manifest_path = staging.join("manifest.json");
        let text =
            fs::read_to_string(&manifest_path).map_err(|e| format!("no manifest.json: {e}"))?;
        let mut json: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
            .map_err(|e| format!("manifest.json isn't valid: {e}"))?;
        json["key"] = Value::String(base64(&crx.key));
        fs::write(&manifest_path, serde_json::to_vec_pretty(&json).unwrap())
            .map_err(|e| e.to_string())?;
        let manifest = Manifest::read(&staging)?;
        check_supported(&manifest)?;
        let version = &manifest.version;
        if version.is_empty() || !version.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return Err("the extension has no proper version".into());
        }
        let target = home.join(version);
        let _ = fs::remove_dir_all(&target);
        fs::rename(&staging, &target).map_err(|e| e.to_string())?;
        Ok((target, manifest))
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

/// Chromium only runs Manifest V3 extensions now.
pub fn check_supported(manifest: &Manifest) -> Result<(), String> {
    match manifest.manifest_version {
        3 => Ok(()),
        2 => Err("This extension uses Manifest V2, which Chromium no longer runs.".into()),
        _ => Err("This isn't a Chrome extension Circle can run.".into()),
    }
}

fn extract_zip(data: &[u8], dest: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(data))
        .map_err(|e| format!("bad package: {e}"))?;
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("bad package: {e}"))?;
        let Some(name) = file.enclosed_name() else {
            return Err("the package has a file outside its folder".into());
        };
        let path = dest.join(name);
        if file.is_dir() {
            fs::create_dir_all(&path).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut contents = Vec::new();
        (&mut file)
            .take(MAX_UNPACKED_BYTES - total + 1)
            .read_to_end(&mut contents)
            .map_err(|e| format!("bad package: {e}"))?;
        total += contents.len() as u64;
        if total > MAX_UNPACKED_BYTES {
            return Err("the package is too large".into());
        }
        fs::write(&path, contents).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

// ----- the Chrome Web Store -------------------------------------------------

fn store_url(id: &str, update_check: Option<&str>) -> String {
    let x = match update_check {
        Some(version) => format!("id%3D{id}%26v%3D{version}%26uc"),
        None => format!("id%3D{id}%26installsource%3Dondemand%26uc"),
    };
    let response = if update_check.is_some() {
        "updatecheck"
    } else {
        "redirect"
    };
    format!(
        "https://clients2.google.com/service/update2/crx?response={response}&prodversion={PRODVERSION}&acceptformat=crx3&x={x}"
    )
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    let output = crate::updater::curl()
        .args([
            "--max-time",
            "300",
            "--max-filesize",
            &MAX_CRX_BYTES.to_string(),
        ])
        .arg(url)
        .output()
        .map_err(|e| format!("couldn't run curl: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "couldn't download the extension: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

/// Downloads an extension from the Chrome Web Store, verifies and unpacks
/// it. Blocking; run off the UI thread.
pub fn fetch_from_store(id: &str, extensions_dir: &Path) -> Result<(PathBuf, Manifest), String> {
    let bytes = download(&store_url(id, None))?;
    install_crx_bytes(&bytes, id, extensions_dir)
}

fn install_crx_bytes(
    bytes: &[u8],
    id: &str,
    extensions_dir: &Path,
) -> Result<(PathBuf, Manifest), String> {
    if bytes.is_empty() {
        return Err("The Chrome Web Store has no download for this extension.".into());
    }
    let crx = verify_crx(bytes)?;
    if crx.id != id {
        return Err("the download is a different extension".into());
    }
    unpack(&crx, extensions_dir)
}

/// Checks the store for a newer version and, if there is one, downloads and
/// unpacks it. Blocking.
pub fn fetch_update(
    extension: &Extension,
    extensions_dir: &Path,
) -> Result<Option<(PathBuf, Manifest)>, String> {
    let output = crate::updater::curl()
        .args(["--max-time", "30"])
        .arg(store_url(&extension.id, Some(&extension.version)))
        .output()
        .map_err(|e| format!("couldn't run curl: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let xml = String::from_utf8_lossy(&output.stdout);
    let Some((version, codebase)) = parse_update(&xml) else {
        return Ok(None);
    };
    if !crate::updater::is_newer(&version, &extension.version) || !codebase.starts_with("https://")
    {
        return Ok(None);
    }
    let bytes = download(&codebase)?;
    install_crx_bytes(&bytes, &extension.id, extensions_dir).map(Some)
}

/// The version and download URL in an update check's answer, if it has one.
fn parse_update(xml: &str) -> Option<(String, String)> {
    let start = xml.find("<updatecheck")?;
    let tag = &xml[start..start + xml[start..].find('>')?];
    let attribute = |name: &str| {
        let needle = format!(" {name}=\"");
        let from = tag.find(&needle)? + needle.len();
        let value = &tag[from..from + tag[from..].find('"')?];
        Some(value.replace("&amp;", "&"))
    };
    if attribute("status")? != "ok" {
        return None;
    }
    Some((attribute("version")?, attribute("codebase")?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn finds_the_extension_of_a_store_page() {
        let id = "ddkjiahejlhfcafbddmgiahcphecmpfh";
        assert_eq!(
            store_id(&format!(
                "https://chromewebstore.google.com/detail/ublock-origin-lite/{id}?hl=en"
            )),
            Some(id.into())
        );
        assert_eq!(
            store_id(&format!("https://chromewebstore.google.com/detail/{id}")),
            Some(id.into())
        );
        assert_eq!(
            store_id(&format!("https://chrome.google.com/webstore/detail/x/{id}")),
            Some(id.into())
        );
        assert_eq!(
            store_id(&format!("https://example.com/detail/x/{id}")),
            None
        );
        assert_eq!(
            store_id("https://chromewebstore.google.com/category/extensions"),
            None
        );
    }

    #[test]
    fn ids_come_from_keys() {
        assert!(is_id(BRIDGE_ID));
        assert!(!is_id("abcdefghijklmnopqrstuvwxyzabcdef"));
        let manifest: Value = serde_json::from_str(BRIDGE_FILES[0].1).unwrap();
        let key = manifest["key"].as_str().unwrap();
        let der = decode_base64(key);
        assert_eq!(id_from_key(&der), BRIDGE_ID);
        assert_eq!(base64(&der), key);
    }

    fn decode_base64(text: &str) -> Vec<u8> {
        const TABLE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let bits: Vec<u8> = text
            .trim_end_matches('=')
            .bytes()
            .map(|c| TABLE.find(c as char).unwrap() as u8)
            .collect();
        bits.chunks(4)
            .flat_map(|c| {
                let n = c
                    .iter()
                    .enumerate()
                    .fold(0u32, |n, (i, v)| n | u32::from(*v) << (18 - 6 * i));
                [(n >> 16) as u8, (n >> 8) as u8, n as u8]
                    .into_iter()
                    .take(c.len() - 1)
            })
            .collect()
    }

    #[test]
    fn reads_what_matters_from_a_manifest() {
        let json = json!({
            "manifest_version": 3,
            "name": "__MSG_name__",
            "version": "1.2.3",
            "action": { "default_popup": "popup.html", "default_icon": { "16": "a.png", "48": "b.png", "32": "c.png" } },
            "options_ui": { "page": "options.html" },
            "permissions": ["storage", "tabs", "declarativeNetRequest"],
            "host_permissions": ["<all_urls>"]
        });
        let manifest = Manifest::parse(&json, |k| (k == "name").then(|| "Blocker".to_owned()));
        assert_eq!(manifest.name, "Blocker");
        assert_eq!(manifest.icon.as_deref(), Some("c.png"));
        assert_eq!(manifest.popup.as_deref(), Some("popup.html"));
        assert_eq!(manifest.options.as_deref(), Some("options.html"));
        assert_eq!(
            manifest.warnings,
            [
                "Read and change all your data on all websites",
                "Block content on any page"
            ]
        );
        let few = json!({ "manifest_version": 3, "permissions": ["tabs"], "content_scripts": [{ "matches": ["https://*.github.com/*"] }] });
        assert_eq!(
            Manifest::parse(&few, |_| None).warnings,
            [
                "Read and change your data on github.com",
                "Read your browsing history"
            ]
        );
    }

    #[test]
    fn icon_paths_stay_inside() {
        assert_eq!(
            pick_icon(Some(&json!("/icons/a.png"))),
            Some("icons/a.png".into())
        );
        assert_eq!(pick_icon(Some(&json!("../a.png"))), None);
        assert_eq!(pick_icon(Some(&json!({ "16": "a/../../b.png" }))), None);
    }

    #[test]
    fn reads_update_checks() {
        let xml = r#"<?xml version="1.0"?><gupdate><app appid="x" status="ok"><updatecheck codebase="https://example.com/x.crx?a=1&amp;b=2" fp="1" hash_sha256="0" size="5" status="ok" version="2.0.1"/></app></gupdate>"#;
        assert_eq!(
            parse_update(xml),
            Some(("2.0.1".into(), "https://example.com/x.crx?a=1&b=2".into()))
        );
        assert_eq!(parse_update(r#"<updatecheck status="noupdate"/>"#), None);
    }

    #[test]
    fn rejects_tampered_packages() {
        assert!(verify_crx(b"PK\x03\x04").is_err());
        let mut fake = b"Cr24".to_vec();
        fake.extend_from_slice(&3u32.to_le_bytes());
        fake.extend_from_slice(&0u32.to_le_bytes());
        assert!(verify_crx(&fake).is_err());
    }

    /// A real store download, if one is lying around (`CIRCLE_TEST_CRX`).
    #[test]
    fn verifies_a_store_package() {
        let Some(path) = std::env::var_os("CIRCLE_TEST_CRX") else {
            return;
        };
        let mut bytes = fs::read(path).unwrap();
        let crx = verify_crx(&bytes).unwrap();
        assert!(is_id(&crx.id));
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(verify_crx(&bytes).is_err());
    }
}
