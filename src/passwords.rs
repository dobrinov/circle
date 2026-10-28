//! Saved passwords: an encrypted vault in the profile, whose key lives in the
//! operating system's own secret store (the macOS Keychain, the Windows
//! credential store). Pages are read and filled by a script in an isolated
//! world (`ui/pw-content.js`) that the page's own scripts can't see.

use aes_gcm::{
    Aes256Gcm, Key, Nonce,
    aead::{Aead, KeyInit},
};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// The script that finds login forms in a page and fills them. Circle runs
/// it in [`WORLD`] after each page load.
pub const CONTENT_JS: &str = include_str!("../ui/pw-content.js");
/// The isolated world the content script runs in.
pub const WORLD: &str = "circle-passwords";
/// The console message the content script uses to say it has news (the
/// news itself is fetched from its world, never logged).
pub const PING: &str = "__circle_pw__";

const MAGIC: &[u8; 4] = b"CPV1";
const FILE: &str = "passwords.vault";
/// The keychain item (macOS) or credential (Windows) holding the vault key.
#[cfg(all(not(debug_assertions), any(target_os = "macos", windows)))]
const KEY_SERVICE: &str = "Circle Passwords";
#[cfg(all(not(debug_assertions), any(target_os = "macos", windows)))]
const KEY_ACCOUNT: &str = "vault-key";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Credential {
    pub id: u64,
    /// `scheme://host[:port]` of the page the password was saved on.
    pub origin: String,
    pub username: String,
    pub password: String,
    pub created: u64,
    pub updated: u64,
    #[serde(default)]
    pub used: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Contents {
    credentials: Vec<Credential>,
    /// Origins the user said never to save passwords for.
    never: Vec<String>,
    next_id: u64,
}

/// What saving a submitted login would do.
#[derive(Clone, Debug, PartialEq)]
pub enum Proposal {
    /// Already saved as is, or the site is on the never list.
    Nothing,
    New,
    /// A saved login with this username has another password.
    Update(u64),
}

pub struct Vault {
    path: PathBuf,
    key: [u8; 32],
    contents: Contents,
}

/// `scheme://host[:port]`, the key passwords are saved under; `None` for
/// anything but http(s) pages.
pub fn origin_of(url: &str) -> Option<String> {
    let url = url::Url::parse(url).ok()?;
    matches!(url.scheme(), "https" | "http").then(|| url.origin().ascii_serialization())
}

impl Vault {
    /// Opens the vault in `dir`, creating its key on first use. Fails rather
    /// than replacing a key it can't read, which would lose every password.
    pub fn open(dir: &Path) -> Result<Vault, String> {
        let path = dir.join(FILE);
        let exists = path.exists();
        let key = match load_key(dir)? {
            Some(key) => key,
            None if exists => {
                return Err("the key for your saved passwords is missing".into());
            }
            None => {
                let mut key = [0u8; 32];
                getrandom::fill(&mut key).map_err(|e| e.to_string())?;
                store_key(dir, &key)?;
                key
            }
        };
        let contents = if exists {
            let data = fs::read(&path).map_err(|e| e.to_string())?;
            decrypt(&key, &data)?
        } else {
            Contents::default()
        };
        Ok(Vault {
            path,
            key,
            contents,
        })
    }

    fn save(&self) -> Result<(), String> {
        let data = encrypt(&self.key, &self.contents)?;
        let tmp = self.path.with_extension("vault.tmp");
        fs::write(&tmp, data).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    pub fn all(&self) -> &[Credential] {
        &self.contents.credentials
    }

    pub fn get(&self, id: u64) -> Option<&Credential> {
        self.contents.credentials.iter().find(|c| c.id == id)
    }

    /// Saved logins for a page's origin, most recently used first.
    pub fn for_origin(&self, origin: &str) -> Vec<&Credential> {
        let mut found: Vec<_> = self
            .contents
            .credentials
            .iter()
            .filter(|c| c.origin == origin)
            .collect();
        found.sort_by_key(|c| std::cmp::Reverse(c.used.max(c.updated)));
        found
    }

    pub fn is_never(&self, origin: &str) -> bool {
        self.contents.never.iter().any(|o| o == origin)
    }

    pub fn propose(&self, origin: &str, username: &str, password: &str) -> Proposal {
        if password.is_empty() || self.is_never(origin) {
            return Proposal::Nothing;
        }
        match self
            .for_origin(origin)
            .into_iter()
            .find(|c| c.username == username)
        {
            Some(c) if c.password == password => Proposal::Nothing,
            Some(c) => Proposal::Update(c.id),
            None => Proposal::New,
        }
    }

    /// Saves a login, replacing the password of a saved one with the same
    /// username.
    pub fn save_login(
        &mut self,
        origin: &str,
        username: &str,
        password: &str,
    ) -> Result<u64, String> {
        let now = crate::model::now();
        let existing = self
            .contents
            .credentials
            .iter_mut()
            .find(|c| c.origin == origin && c.username == username);
        let id = match existing {
            Some(c) => {
                c.password = password.to_owned();
                c.updated = now;
                c.id
            }
            None => {
                self.contents.next_id += 1;
                let id = self.contents.next_id;
                self.contents.credentials.push(Credential {
                    id,
                    origin: origin.to_owned(),
                    username: username.to_owned(),
                    password: password.to_owned(),
                    created: now,
                    updated: now,
                    used: 0,
                });
                id
            }
        };
        self.save()?;
        Ok(id)
    }

    pub fn mark_used(&mut self, id: u64) {
        if let Some(c) = self.contents.credentials.iter_mut().find(|c| c.id == id) {
            c.used = crate::model::now();
            let _ = self.save();
        }
    }

    pub fn remove(&mut self, id: u64) -> Result<(), String> {
        self.contents.credentials.retain(|c| c.id != id);
        self.save()
    }

    pub fn never_save(&mut self, origin: &str) -> Result<(), String> {
        if !self.is_never(origin) {
            self.contents.never.push(origin.to_owned());
        }
        self.save()
    }

    pub fn forget_never(&mut self, origin: &str) -> Result<(), String> {
        self.contents.never.retain(|o| o != origin);
        self.save()
    }

    pub fn never_list(&self) -> &[String] {
        &self.contents.never
    }
}

fn encrypt(key: &[u8; 32], contents: &Contents) -> Result<Vec<u8>, String> {
    let plain = serde_json::to_vec(contents).map_err(|e| e.to_string())?;
    let mut nonce = [0u8; 12];
    getrandom::fill(&mut nonce).map_err(|e| e.to_string())?;
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let sealed = cipher
        .encrypt(Nonce::from_slice(&nonce), plain.as_slice())
        .map_err(|_| "couldn't encrypt the passwords".to_owned())?;
    Ok([MAGIC.as_slice(), &nonce, &sealed].concat())
}

fn decrypt(key: &[u8; 32], data: &[u8]) -> Result<Contents, String> {
    let rest = data
        .strip_prefix(MAGIC.as_slice())
        .filter(|r| r.len() > 12)
        .ok_or("the password file is damaged")?;
    let (nonce, sealed) = rest.split_at(12);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let plain = cipher
        .decrypt(Nonce::from_slice(nonce), sealed)
        .map_err(|_| "couldn't decrypt the saved passwords".to_owned())?;
    serde_json::from_slice(&plain).map_err(|e| e.to_string())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<[u8; 32]> {
    let text = text.trim();
    if text.len() != 64 {
        return None;
    }
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(key)
}

/// Debug builds keep the key in a file: every rebuild changes their ad-hoc
/// code signature, so the Keychain would ask for access each time. So does
/// Linux, until Circle uses the desktop's keyring there.
#[cfg(any(debug_assertions, not(any(target_os = "macos", windows))))]
fn load_key(dir: &Path) -> Result<Option<[u8; 32]>, String> {
    match fs::read_to_string(dir.join("passwords.key")) {
        Ok(text) => unhex(&text)
            .map(Some)
            .ok_or("the password key file is damaged".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(any(debug_assertions, not(any(target_os = "macos", windows))))]
fn store_key(dir: &Path, key: &[u8; 32]) -> Result<(), String> {
    let path = dir.join("passwords.key");
    fs::write(&path, hex(key)).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[cfg(all(not(debug_assertions), any(target_os = "macos", windows)))]
fn load_key(_dir: &Path) -> Result<Option<[u8; 32]>, String> {
    let entry = keyring::Entry::new(KEY_SERVICE, KEY_ACCOUNT).map_err(|e| e.to_string())?;
    match entry.get_password() {
        Ok(text) => unhex(&text)
            .map(Some)
            .ok_or("the key for your saved passwords is damaged".into()),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!(
            "couldn't read the key for your saved passwords: {e}"
        )),
    }
}

#[cfg(all(not(debug_assertions), any(target_os = "macos", windows)))]
fn store_key(_dir: &Path, key: &[u8; 32]) -> Result<(), String> {
    keyring::Entry::new(KEY_SERVICE, KEY_ACCOUNT)
        .and_then(|entry| entry.set_password(&hex(key)))
        .map_err(|e| format!("couldn't store the key for your saved passwords: {e}"))
}

/// Asks the person at the computer to prove it's them (Touch ID or the Mac
/// password; Windows Hello) before a password is shown or copied. Blocks:
/// call it off the UI thread. Without such a check (Linux) it allows.
pub fn confirm_owner(reason: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        mac_confirm(reason)
    }
    #[cfg(windows)]
    {
        windows_confirm(reason)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = reason;
        true
    }
}

#[cfg(target_os = "macos")]
fn mac_confirm(reason: &str) -> bool {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_foundation::{NSError, NSString};
    use objc2_local_authentication::{LAContext, LAPolicy};
    use std::sync::mpsc;

    let context = unsafe { LAContext::new() };
    let (tx, rx) = mpsc::channel();
    let reply = RcBlock::new(move |ok: Bool, _error: *mut NSError| {
        let _ = tx.send(ok.as_bool());
    });
    unsafe {
        context.evaluatePolicy_localizedReason_reply(
            LAPolicy::DeviceOwnerAuthentication,
            &NSString::from_str(reason),
            &reply,
        );
    }
    rx.recv().unwrap_or(false)
}

#[cfg(windows)]
fn windows_confirm(reason: &str) -> bool {
    use windows::Security::Credentials::UI::{
        UserConsentVerificationResult, UserConsentVerifier, UserConsentVerifierAvailability,
    };
    use windows::core::HSTRING;
    let available = UserConsentVerifier::CheckAvailabilityAsync()
        .and_then(|op| op.get())
        .is_ok_and(|a| a == UserConsentVerifierAvailability::Available);
    if !available {
        // No Windows Hello set up: the signed-in session is the check.
        return true;
    }
    UserConsentVerifier::RequestVerificationAsync(&HSTRING::from(reason))
        .and_then(|op| op.get())
        .is_ok_and(|r| r == UserConsentVerificationResult::Verified)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("circle-pw-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_vault_is_encrypted_and_reopens() {
        let dir = temp_dir("reopen");
        let mut vault = Vault::open(&dir).unwrap();
        vault
            .save_login("https://github.com", "octo", "hunter2")
            .unwrap();
        let raw = fs::read(dir.join(FILE)).unwrap();
        assert!(raw.starts_with(MAGIC));
        assert!(
            !raw.windows(7).any(|w| w == b"hunter2"),
            "stored in the clear"
        );
        let vault = Vault::open(&dir).unwrap();
        assert_eq!(
            vault.for_origin("https://github.com")[0].password,
            "hunter2"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_wrong_key_does_not_open_or_replace_the_vault() {
        let dir = temp_dir("wrongkey");
        let mut vault = Vault::open(&dir).unwrap();
        vault.save_login("https://a.test", "me", "pw").unwrap();
        let other = [7u8; 32];
        assert!(decrypt(&other, &fs::read(dir.join(FILE)).unwrap()).is_err());
        fs::remove_file(dir.join("passwords.key")).unwrap();
        assert!(
            Vault::open(&dir).is_err(),
            "a missing key must not start a new vault"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn proposals_follow_what_is_saved() {
        let dir = temp_dir("propose");
        let mut vault = Vault::open(&dir).unwrap();
        let site = "https://example.com";
        assert_eq!(vault.propose(site, "me", "one"), Proposal::New);
        let id = vault.save_login(site, "me", "one").unwrap();
        assert_eq!(vault.propose(site, "me", "one"), Proposal::Nothing);
        assert_eq!(vault.propose(site, "me", "two"), Proposal::Update(id));
        assert_eq!(vault.propose(site, "me", ""), Proposal::Nothing);
        assert_eq!(
            vault.propose("https://other.example", "me", "one"),
            Proposal::New
        );
        vault.never_save(site).unwrap();
        assert_eq!(vault.propose(site, "you", "x"), Proposal::Nothing);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn origins_are_scheme_host_and_port() {
        assert_eq!(
            origin_of("https://github.com/login?x=1").as_deref(),
            Some("https://github.com")
        );
        assert_eq!(
            origin_of("http://localhost:3000/a").as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(origin_of("file:///etc/passwd"), None);
        assert_eq!(origin_of("circle://ui/settings.html"), None);
    }
}
