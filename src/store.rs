//! Versioned browser data; never stores a loaded-document or transport assertion.
use crate::browser::{Browser, Entry, SearchProvider, normalize_address};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
const MAX_BYTES: usize = 2 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct SavedTab {
    url: String,
    pinned: bool,
    group: String,
    zoom: u16,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u32,
    groups: Vec<String>,
    bookmarks: Vec<Entry>,
    history: Vec<Entry>,
    reading_list: Vec<Entry>,
    tabs: Vec<SavedTab>,
    active: usize,
    search: SearchProvider,
    restore_session: bool,
}
impl Snapshot {
    fn capture(b: &Browser) -> Self {
        Self {
            version: 1,
            groups: b.groups.clone(),
            bookmarks: b.bookmarks.clone(),
            history: b.history.clone(),
            reading_list: b.reading_list.clone(),
            tabs: b
                .tabs
                .iter()
                .map(|t| SavedTab {
                    url: t
                        .document
                        .as_ref()
                        .map(|e| e.url.clone())
                        .unwrap_or_else(|| normalize_address(&t.draft).unwrap_or_default()),
                    pinned: t.pinned,
                    group: t.group.clone(),
                    zoom: t.zoom,
                })
                .collect(),
            active: b.active,
            search: b.search,
            restore_session: b.restore_session,
        }
    }
    fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || self.tabs.is_empty()
            || self.tabs.len() > 64
            || self.active >= self.tabs.len()
            || self.bookmarks.len() > 1000
            || self.reading_list.len() > 1000
            || self.history.len() > 200
        {
            return Err("Unsupported or invalid browser data.".into());
        }
        for entry in self
            .bookmarks
            .iter()
            .chain(&self.history)
            .chain(&self.reading_list)
        {
            normalize_address(&entry.url)?;
            if entry.title.len() > 4096 || entry.title.chars().any(char::is_control) {
                return Err("Invalid saved page title.".into());
            }
        }
        if self.groups.is_empty() || self.groups.len() > 16 || self.groups[0] != "Default" {
            return Err("Invalid tab groups".into());
        }
        let mut seen = std::collections::HashSet::new();
        for group in &self.groups {
            if group.trim().is_empty()
                || group.chars().count() > 64
                || group.chars().any(char::is_control)
                || !seen.insert(group.to_lowercase())
            {
                return Err("Invalid tab group name".into());
            }
        }
        let mut ordinary = false;
        for tab in &self.tabs {
            if !tab.url.is_empty() {
                normalize_address(&tab.url)?;
            }
            if !(50..=300).contains(&tab.zoom)
                || (ordinary && tab.pinned)
                || !self.groups.contains(&tab.group)
            {
                return Err("Invalid saved tab.".into());
            }
            ordinary |= !tab.pinned;
        }
        Ok(())
    }
}
/// Compare-before-replace storage. A second process cannot silently overwrite changes.
pub(crate) struct Store {
    path: PathBuf,
    previous: Option<Vec<u8>>,
}
impl Store {
    /// Opens one user's data. Corruption is an error, never an empty successful load.
    pub fn load(path: PathBuf, browser: &mut Browser) -> Result<Self, String> {
        let previous = read(&path)?;
        if let Some(bytes) = &previous {
            let data: Snapshot = serde_json::from_slice(bytes)
                .map_err(|_| "Browser data is damaged; it has not been overwritten.")?;
            data.validate()?;
            browser.bookmarks = data.bookmarks;
            browser.history = data.history;
            browser.reading_list = data.reading_list;
            browser.search = data.search;
            browser.restore_session = data.restore_session;
            browser.groups = data.groups;
            if data.restore_session {
                for (index, saved) in data.tabs.into_iter().enumerate() {
                    if index > 0 {
                        browser.new_tab()?;
                    }
                    browser.tabs[index].restore_pending = !saved.url.is_empty();
                    browser.tabs[index].draft = saved.url;
                    browser.tabs[index].pinned = saved.pinned;
                    browser.tabs[index].group = saved.group;
                    browser.tabs[index].zoom = saved.zoom;
                }
                browser.active = data.active;
            }
        }
        Ok(Self { path, previous })
    }
    /// Atomically saves validated data under a short exclusive sibling lock.
    pub fn save(&mut self, browser: &Browser) -> Result<(), String> {
        let data = Snapshot::capture(browser);
        data.validate()?;
        let bytes = serde_json::to_vec(&data).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_BYTES {
            return Err("Browser data is too large to save.".into());
        }
        if self.previous.as_ref() == Some(&bytes) {
            return Ok(());
        }
        let parent = self.path.parent().ok_or("No browser data directory")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let lock_path = self.path.with_extension("lock");
        let _file = private_options()
            .create_new(true)
            .open(&lock_path)
            .map_err(|_| "Browser data is in use; changes have not been saved.")?;
        let _lock = Cleanup(lock_path);
        if read(&self.path)? != self.previous {
            return Err(
                "Browser data changed in another window. Restart Terra before saving.".into(),
            );
        }
        let temporary = self.path.with_extension("pending");
        let mut file = private_options()
            .create_new(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        let _temporary = Cleanup(temporary.clone());
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(&temporary, &self.path).map_err(|e| e.to_string())?;
        self.previous = Some(bytes);
        Ok(())
    }
}
struct Cleanup(PathBuf);
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}
fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = vec![];
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("Browser data exceeds the size limit.".into());
    }
    Ok(Some(bytes))
}
/// Uses the existing per-user application-data location and preview override.
pub(crate) fn path() -> Result<PathBuf, String> {
    #[cfg(target_os = "linux")]
    {
        if let Some(root) = std::env::var_os("MOCHIOS_PREVIEW_CONFIG_ROOT") {
            return Ok(PathBuf::from(root).join("terra/browser-v1.json"));
        }
        return Ok(
            PathBuf::from(std::env::var_os("HOME").ok_or("No home directory")?)
                .join(".local/share/mochios/terra/browser-v1.json"),
        );
    }
    #[cfg(not(target_os = "linux"))]
    {
        let user = std::env::var("USER").map_err(|_| "No active user")?;
        if user.is_empty()
            || user.len() > 64
            || !user
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        {
            return Err("Invalid active user".into());
        }
        Ok(Path::new("/libraries/applications/org.mochios.terra/users")
            .join(user)
            .join("browser-v1.json"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "terra-store-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
        fn file(&self) -> PathBuf {
            self.0.join("browser.json")
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn libraries_groups_settings_and_addresses_restore_without_a_fake_document() {
        let directory = Directory::new();
        let mut browser = Browser::new();
        let mut store = Store::load(directory.file(), &mut browser).unwrap();
        browser.add_bookmark("Reference", "example.org").unwrap();
        browser.reading_list = browser.bookmarks.clone();
        browser.history = browser.bookmarks.clone();
        browser.search = SearchProvider::DuckDuckGo;
        browser.create_group("Work").unwrap();
        browser.tabs[1].draft = "https://example.org/work".into();
        browser.tabs[1].zoom = 130;
        browser.toggle_pin();
        store.save(&browser).unwrap();
        let mut restored = Browser::new();
        Store::load(directory.file(), &mut restored).unwrap();
        assert_eq!(restored.bookmarks, browser.bookmarks);
        assert_eq!(restored.history, browser.history);
        assert_eq!(restored.reading_list, browser.reading_list);
        assert_eq!(restored.search, SearchProvider::DuckDuckGo);
        assert_eq!(restored.tab().draft, "https://example.org/work");
        assert!(restored.tab().pinned);
        assert_eq!(restored.tab().zoom, 130);
        assert_eq!(restored.tab().group, "Work");
        assert!(restored.tabs.iter().all(|t| t.document.is_none()
            && t.security == crate::engine::Security::Unknown
            && !t.loading));
    }
    #[test]
    fn conflicts_and_existing_locks_never_overwrite_another_window() {
        let directory = Directory::new();
        let mut first = Browser::new();
        let mut second = Browser::new();
        let mut a = Store::load(directory.file(), &mut first).unwrap();
        let mut b = Store::load(directory.file(), &mut second).unwrap();
        first.add_bookmark("One", "one.example").unwrap();
        a.save(&first).unwrap();
        let bytes = fs::read(directory.file()).unwrap();
        second.add_bookmark("Two", "two.example").unwrap();
        assert!(b.save(&second).is_err());
        assert_eq!(fs::read(directory.file()).unwrap(), bytes);
        fs::write(directory.file().with_extension("lock"), b"").unwrap();
        first.add_bookmark("Three", "three.example").unwrap();
        assert!(a.save(&first).is_err());
        assert_eq!(fs::read(directory.file()).unwrap(), bytes);
    }
    #[test]
    fn malformed_or_oversized_data_is_not_silently_reset() {
        let directory = Directory::new();
        for bytes in [b"broken".to_vec(), vec![b'x'; MAX_BYTES + 1]] {
            fs::write(directory.file(), &bytes).unwrap();
            assert!(Store::load(directory.file(), &mut Browser::new()).is_err());
            assert_eq!(fs::read(directory.file()).unwrap(), bytes);
        }
        let mut data = Snapshot::capture(&Browser::new());
        data.bookmarks.push(Entry {
            url: "javascript:alert(1)".into(),
            title: "Bad".into(),
        });
        fs::write(directory.file(), serde_json::to_vec(&data).unwrap()).unwrap();
        assert!(Store::load(directory.file(), &mut Browser::new()).is_err());
    }
    #[test]
    fn restoration_can_be_disabled_and_invalid_drafts_are_not_persisted_as_urls() {
        let directory = Directory::new();
        let mut b = Browser::new();
        let mut store = Store::load(directory.file(), &mut b).unwrap();
        b.new_tab().unwrap();
        b.tabs[1].draft = "unfinished search words".into();
        b.restore_session = false;
        b.add_bookmark("Saved", "example.org").unwrap();
        store.save(&b).unwrap();
        let mut restored = Browser::new();
        Store::load(directory.file(), &mut restored).unwrap();
        assert_eq!(restored.tabs.len(), 1);
        assert_eq!(restored.bookmarks.len(), 1);
        assert!(!restored.restore_session);
    }
}
