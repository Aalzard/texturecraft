//! The UI state kept between runs as JSON: `ui.json` beside eframe's settings on the desktop, a key of eframe's storage on the web

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::UiState;

/// eframe storage key of the UI state as JSON, where there is no file
const STORAGE_KEY: &str = "ui.json";

/// eframe storage key under which earlier builds kept the UI state, as RON
const RON_STORAGE_KEY: &str = "ui";

/// eframe storage key of a saved state this build could not fully read, where there is no file
const UNREADABLE_STORAGE_KEY: &str = "ui.unreadable.json";

/// Largest UI state file read; a saved state is a few kilobytes
const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

/// Copies of unreadable saved states kept beside the file before saving stops
const MAX_UNREADABLE_COPIES: u32 = 10;

/// The desktop's `ui.json`, set by the app before its window opens; unset, the state stays in eframe's storage
pub static FILE: OnceLock<PathBuf> = OnceLock::new();

/// Where the UI state is saved, holding a saved state this build could not fully read until it is kept beside it
#[derive(Default)]
pub struct UiStateStore {
    file: Option<PathBuf>,
    unreadable: Option<Vec<u8>>,
    /// The unreadable state could not be kept, so the saved one is left as it is
    frozen: bool,
}

impl UiStateStore {
    /// The store at `file`, or in eframe's storage without one
    pub fn new(file: Option<PathBuf>) -> Self {
        UiStateStore { file, unreadable: None, frozen: false }
    }

    /// The saved UI state, else the one an earlier build saved; a state not fully read is kept for the next save
    pub fn load(&mut self, storage: Option<&dyn eframe::Storage>) -> Option<UiState> {
        let saved = match &self.file {
            Some(file) if too_large(file) => {
                self.frozen = true;
                return None;
            }
            Some(file) => std::fs::read(file).ok(),
            None => storage.and_then(|storage| storage.get_string(STORAGE_KEY)).map(String::into_bytes),
        };
        let Some(bytes) = saved else {
            return storage.and_then(|storage| eframe::get_value::<UiState>(storage, RON_STORAGE_KEY));
        };

        let ui = serde_json::from_slice::<UiState>(&bytes).ok();
        if !ui.as_ref().is_some_and(fully_read) {
            self.unreadable = Some(bytes);
        }
        ui
    }

    /// Save `ui` as JSON once an unreadable saved state is kept, and drop the RON copy of earlier builds
    pub fn save(&mut self, ui: &UiState, storage: &mut dyn eframe::Storage) {
        if let Some(bytes) = self.unreadable.take() {
            self.frozen = !self.keep_unreadable(&bytes, storage);
        }
        if self.frozen {
            return;
        }

        let Ok(json) = serde_json::to_string_pretty(ui) else { return };
        let saved = match &self.file {
            Some(file) => write_file(file, json.as_bytes()),
            None => {
                storage.set_string(STORAGE_KEY, json);
                true
            }
        };
        if saved {
            storage.remove_string(RON_STORAGE_KEY);
        }
    }

    /// Keep a saved state this build could not fully read, beside the file or under its own storage key
    fn keep_unreadable(&self, bytes: &[u8], storage: &mut dyn eframe::Storage) -> bool {
        match &self.file {
            Some(file) => keep_file_copy(file, bytes),
            None => {
                storage.set_string(UNREADABLE_STORAGE_KEY, String::from_utf8_lossy(bytes).into_owned());
                true
            }
        }
    }
}

/// The UI state saved in `file`, if it reads
pub fn read(file: &Path) -> Option<UiState> {
    if too_large(file) {
        return None;
    }

    serde_json::from_slice(&std::fs::read(file).ok()?).ok()
}

/// Whether both docks were read rather than replaced by a default
fn fully_read(ui: &UiState) -> bool {
    !ui.dock.replaced_unreadable() && !ui.inactive_mode_dock.replaced_unreadable()
}

/// Whether `file` is too large to be a saved UI state, so it is neither read nor saved over
fn too_large(file: &Path) -> bool {
    std::fs::metadata(file).is_ok_and(|metadata| metadata.len() > MAX_FILE_BYTES)
}

#[cfg(not(target_arch = "wasm32"))]
fn write_file(file: &Path, bytes: &[u8]) -> bool {
    if let Some(folder) = file.parent() {
        let _ = std::fs::create_dir_all(folder);
    }

    texturecraft_engine::formats::write_atomic(file, bytes).is_ok()
}

#[cfg(target_arch = "wasm32")]
fn write_file(_file: &Path, _bytes: &[u8]) -> bool {
    false
}

/// Copy `bytes` beside `file` as the first free `<stem>.unreadable.json` or `<stem>.unreadable-N.json`
fn keep_file_copy(file: &Path, bytes: &[u8]) -> bool {
    use std::io::Write as _;

    let stem = file.file_stem().map(|stem| stem.to_string_lossy().into_owned()).unwrap_or_default();
    for n in 1..=MAX_UNREADABLE_COPIES {
        let name = if n == 1 { format!("{stem}.unreadable.json") } else { format!("{stem}.unreadable-{n}.json") };
        match std::fs::OpenOptions::new().write(true).create_new(true).open(file.with_file_name(name)) {
            Ok(mut copy) => return copy.write_all(bytes).is_ok(),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return false,
        }
    }
    false
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::docking::{Dock, Panel, default_workspace, permissions};
    use craft_ui::docking::{Action, Placement};

    /// eframe's storage held in memory
    #[derive(Default)]
    pub(crate) struct MemoryStorage(pub(crate) std::collections::HashMap<String, String>);

    impl eframe::Storage for MemoryStorage {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }

        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_owned(), value);
        }

        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }

        fn flush(&mut self) {}
    }

    /// Save `ui` and load it back, as closing and reopening the app does
    pub(crate) fn restart(ui: &UiState) -> UiState {
        let mut storage = MemoryStorage::default();
        UiStateStore::default().save(ui, &mut storage);
        UiStateStore::default().load(Some(&storage)).unwrap_or_else(|| panic!("the saved state loads: {:?}", storage.0))
    }

    /// An empty folder of its own for one test
    fn test_folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("texturecraft-ui-store-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// The painting default with Layers moved into a tab beside Assets and Properties floating
    fn moved_panels() -> Dock {
        let mut workspace = default_workspace();
        let actions = [
            Action::Move { panel: Panel::Layers, anchor: Panel::Assets, placement: Placement::Tab { before: None } },
            Action::Float { panel: Panel::Properties, rect: [900.0, 120.0, 320.0, 400.0] },
        ];
        workspace.apply_all(actions, permissions).unwrap();

        let dock = Dock::from_value(serde_json::to_value(&workspace).unwrap());
        assert!(!dock.replaced_unreadable(), "the moved layout is valid");
        dock
    }

    #[test]
    fn panel_layouts_survive_a_restart_in_the_file_and_in_storage() {
        let folder = test_folder("layouts");
        for dock in [Dock::default(), moved_panels()] {
            let ui = UiState { dock, split_ratio: 0.3, ..UiState::default() };
            for file in [None, Some(folder.join("ui.json"))] {
                let mut storage = MemoryStorage::default();
                UiStateStore::new(file.clone()).save(&ui, &mut storage);

                let restarted = UiStateStore::new(file.clone()).load(Some(&storage)).unwrap();
                assert!(fully_read(&restarted), "{file:?}");
                assert_eq!(restarted, ui, "{file:?}");
            }
        }
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn the_state_an_earlier_build_saved_loads_and_its_copy_goes_on_the_next_save() {
        let mut storage = MemoryStorage::default();
        let earlier = UiState { split_ratio: 0.3, ..UiState::default() };
        eframe::set_value(&mut storage, RON_STORAGE_KEY, &earlier);

        let mut store = UiStateStore::default();
        let loaded = store.load(Some(&storage)).expect("the earlier state loads");
        assert_eq!(loaded.split_ratio, 0.3);

        store.save(&loaded, &mut storage);
        assert!(!storage.0.contains_key(RON_STORAGE_KEY));
        assert_eq!(UiStateStore::default().load(Some(&storage)).unwrap(), loaded);
    }

    #[test]
    fn an_unreadable_layout_loads_the_default_and_is_kept_beside_the_file() {
        let folder = test_folder("unreadable");
        let file = folder.join("ui.json");
        let ui = UiState { dock: moved_panels(), split_ratio: 0.3, ..UiState::default() };
        let mut storage = MemoryStorage::default();
        UiStateStore::new(Some(file.clone())).save(&ui, &mut storage);
        let saved = std::fs::read_to_string(&file).unwrap();
        assert!(saved.contains("\"Assets\"") && saved.contains("\"version\": 2"), "{saved}");

        let unreadable = [
            // A panel this build does not know
            saved.replacen("\"Assets\"", "\"Bogus\"", 1),
            // A layout from a newer format
            saved.replacen("\"version\": 2", "\"version\": 99", 1),
            // Not a saved state
            "not json".to_owned(),
        ];
        for (n, text) in unreadable.iter().enumerate() {
            std::fs::write(&file, text).unwrap();
            let mut store = UiStateStore::new(Some(file.clone()));
            let loaded = store.load(Some(&storage));
            if let Some(loaded) = &loaded {
                assert_eq!(loaded.dock, Dock::default(), "{text}");
                assert_eq!(loaded.split_ratio, 0.3, "{text}");
            }

            store.save(&loaded.unwrap_or_default(), &mut storage);
            let copy = if n == 0 { "ui.unreadable.json".to_owned() } else { format!("ui.unreadable-{}.json", n + 1) };
            assert_eq!(std::fs::read_to_string(folder.join(copy)).unwrap(), *text);
            assert_ne!(std::fs::read_to_string(&file).unwrap(), *text, "the next save replaces it once kept");
        }
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn an_unreadable_state_that_cannot_be_kept_is_not_saved_over() {
        let folder = test_folder("frozen");
        let file = folder.join("ui.json");
        for n in 1..=MAX_UNREADABLE_COPIES {
            let name = if n == 1 { "ui.unreadable.json".to_owned() } else { format!("ui.unreadable-{n}.json") };
            std::fs::write(folder.join(name), "").unwrap();
        }
        std::fs::write(&file, "not json").unwrap();

        let mut store = UiStateStore::new(Some(file.clone()));
        let mut storage = MemoryStorage::default();
        assert!(store.load(Some(&storage)).is_none());
        store.save(&UiState::default(), &mut storage);
        store.save(&UiState::default(), &mut storage);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "not json");
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn a_file_too_large_to_read_is_not_saved_over() {
        let folder = test_folder("too-large");
        let file = folder.join("ui.json");
        std::fs::write(&file, vec![b' '; MAX_FILE_BYTES as usize + 1]).unwrap();

        let mut store = UiStateStore::new(Some(file.clone()));
        let mut storage = MemoryStorage::default();
        assert!(store.load(Some(&storage)).is_none());
        store.save(&UiState::default(), &mut storage);
        assert_eq!(std::fs::metadata(&file).unwrap().len(), MAX_FILE_BYTES + 1);
        std::fs::remove_dir_all(&folder).unwrap();
    }

    #[test]
    fn an_unreadable_layout_in_storage_is_kept_under_its_own_key() {
        let mut storage = MemoryStorage::default();
        UiStateStore::default().save(&UiState { dock: moved_panels(), ..UiState::default() }, &mut storage);
        let bogus = storage.0[STORAGE_KEY].replacen("\"Assets\"", "\"Bogus\"", 1);
        storage.0.insert(STORAGE_KEY.to_owned(), bogus.clone());

        let mut store = UiStateStore::default();
        let loaded = store.load(Some(&storage)).unwrap();
        assert_eq!(loaded.dock, Dock::default());
        store.save(&loaded, &mut storage);
        assert_eq!(storage.0[UNREADABLE_STORAGE_KEY], bogus);
        assert_eq!(UiStateStore::default().load(Some(&storage)).unwrap(), loaded);
    }
}
