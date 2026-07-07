use std::path::Path;

use mtp_rs::mtp::{MtpDevice, NewObjectInfo, ObjectHandle, Storage, StorageType};

pub use mtp_rs::mtp::MtpDeviceInfo;

/// Scan for connected MTP devices.
pub fn scan_devices() -> Vec<MtpDeviceInfo> {
    MtpDevice::list_devices().unwrap_or_default()
}

/// Sync an episode file to all connected MTP devices.
///
/// `base_path` is a path like `Podcasts` or `Music/Podcasts`.
/// Returns the number of devices the file was successfully uploaded to.
pub async fn sync_episode_to_all(
    source_path: &Path,
    show_title: &str,
    episode_title: &str,
    base_path: &str,
) -> Result<u32, String> {
    let devices = scan_devices();
    if devices.is_empty() {
        return Err("No MTP devices found".into());
    }

    let mut success_count: u32 = 0;
    let mut errors: Vec<String> = Vec::new();

    for dev_info in &devices {
        let label = dev_info
            .product
            .as_deref()
            .or_else(|| dev_info.manufacturer.as_deref())
            .unwrap_or("MTP device");

        match sync_episode_to_device(source_path, show_title, episode_title, dev_info.location_id, base_path)
            .await
        {
            Ok(_) => success_count += 1,
            Err(e) => errors.push(format!("{}: {}", label, e)),
        }
    }

    if success_count > 0 {
        Ok(success_count)
    } else {
        Err(errors.join("; "))
    }
}

/// Upload a single episode file to a specific MTP device by USB location ID.
async fn sync_episode_to_device(
    source_path: &Path,
    show_title: &str,
    episode_title: &str,
    location_id: u64,
    base_path: &str,
) -> Result<String, String> {
    let device = MtpDevice::open_by_location(location_id)
        .await
        .map_err(fmt_mtp_error)?;

    let storages = device
        .storages()
        .await
        .map_err(fmt_mtp_error)?;

    let storage = storages
        .into_iter()
        .find(|s| s.info().free_space > 0)
        .ok_or_else(|| "no writable storage".to_string())?;

    // Create each component of the base path (e.g. "Music/Podcasts")
    let mut parent = None;
    for component in base_path.split('/').filter(|s| !s.is_empty()) {
        let sanitized = sanitize_filename(component);
        parent = Some(find_or_create_folder(&storage, parent, &sanitized).await?);
    }

    let show_folder_name = sanitize_filename(show_title);
    let show_folder = find_or_create_folder(&storage, parent, &show_folder_name).await?;

    let file = tokio::fs::File::open(source_path)
        .await
        .map_err(|e| format!("open: {}", e))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|e| format!("metadata: {}", e))?;
    let file_len = metadata.len();

    let extension = source_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp3");
    let dest_filename = format!("{}.{}", sanitize_filename(episode_title), extension);

    let info = NewObjectInfo::file(&dest_filename, file_len);
    let stream = tokio_util::io::ReaderStream::new(file);

    storage
        .upload(Some(show_folder), info, stream)
        .await
        .map_err(|e| format!("Upload failed: {}", e))?;

    Ok(dest_filename)
}

/// Find a folder by name, or create it if it doesn't exist.
async fn find_or_create_folder(
    storage: &Storage,
    parent: Option<ObjectHandle>,
    name: &str,
) -> Result<ObjectHandle, String> {
    let objects = storage.list_objects(parent).await.map_err(fmt_mtp_error)?;

    for obj in &objects {
        if obj.is_folder() && obj.filename == name {
            return Ok(obj.handle);
        }
    }

    storage
        .create_folder(parent, name)
        .await
        .map_err(fmt_mtp_error)
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn to_item_info(o: mtp_rs::mtp::ObjectInfo) -> MtpItemInfo {
    let is_folder = o.is_folder();
    MtpItemInfo {
        handle: o.handle,
        name: o.filename,
        is_folder,
    }
}

/// Wrap an mtp-rs error with an actionable user hint.
fn fmt_mtp_error(e: mtp_rs::Error) -> String {
    if e.is_exclusive_access() {
        "Device is in use by another app (e.g. file manager).\n\
         Unmount it first: eject from your file manager, or run:\n  \
         gio mount -e mtp://...\n\
         Then unplug and reconnect the device."
            .to_string()
    } else if e.is_permission_denied() {
        "Permission denied. You may need udev rules:\n  \
         sudo apt install mtp-tools\n\
         or add yourself to the 'plugdev' group and reboot."
            .to_string()
    } else {
        format!("{}", e)
    }
}

// ── Folder browser ──────────────────────────────────────────────────────────

/// Info about one item in a folder listing.
#[derive(Debug, Clone)]
pub struct MtpItemInfo {
    pub handle: ObjectHandle,
    pub name: String,
    pub is_folder: bool,
}

/// Human-readable summary of a storage for the UI.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct MtpStorageDisplay {
    pub index: usize,
    pub description: String,
    pub free_space: u64,
    pub total_capacity: u64,
    pub label: String, // "Internal" / "SD card"
}

/// Stateful browser that keeps an MTP connection open for interactive navigation.
#[allow(dead_code)]
pub struct MtpBrowser {
    device: MtpDevice,
    storages: Vec<Storage>,
    selected_storage: usize,
    /// Stack of (folder_name, folder_handle) — empty = at root.
    pub path_segments: Vec<(String, ObjectHandle)>,
    pub items: Vec<MtpItemInfo>,
    pub error: String,
}

impl MtpBrowser {
    /// Open a device and list the root of the first writable storage.
    pub fn open(location_id: u64) -> Result<Self, String> {
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            let device = MtpDevice::open_by_location(location_id)
                .await
                .map_err(fmt_mtp_error)?;
            let storages = device.storages().await.map_err(fmt_mtp_error)?;
            if storages.is_empty() {
                return Err("No storages found on device".into());
            }
            let selected = storages
                .iter()
                .position(|s| s.info().free_space > 0)
                .unwrap_or(0);
            let objects = storages[selected]
                .list_objects(None)
                .await
                .map_err(fmt_mtp_error)?;
            let items: Vec<MtpItemInfo> = objects.into_iter().map(to_item_info).collect();
            Ok(MtpBrowser {
                device,
                storages,
                selected_storage: selected,
                path_segments: Vec::new(),
                items,
                error: String::new(),
            })
        })
    }

    /// Select a different storage by index.
    pub fn select_storage(&mut self, index: usize) {
        if index >= self.storages.len() || index == self.selected_storage {
            return;
        }
        self.selected_storage = index;
        self.path_segments.clear();
        self.refresh();
    }

    /// Current storage index.
    pub fn current_storage(&self) -> usize {
        self.selected_storage
    }

    /// Info for all storages (for the UI picker).
    pub fn storage_info_list(&self) -> Vec<MtpStorageDisplay> {
        self.storages
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let info = s.info();
                let label = match info.storage_type {
                    StorageType::RemovableRam | StorageType::RemovableRom => "SD card",
                    StorageType::FixedRam => "Internal",
                    StorageType::FixedRom => "ROM (read-only)",
                    _ => "Other",
                };
                MtpStorageDisplay {
                    index: i,
                    description: info.description.clone(),
                    free_space: info.free_space,
                    total_capacity: info.total_capacity,
                    label: label.to_string(),
                }
            })
            .collect()
    }

    fn storage(&self) -> &Storage {
        &self.storages[self.selected_storage]
    }

    /// Handle of the current folder (None = root).
    fn current_handle(&self) -> Option<ObjectHandle> {
        self.path_segments.last().map(|(_, h)| *h)
    }

    /// Navigate into a sub-folder by name.
    pub fn enter(&mut self, name: &str) {
        let handle = self
            .items
            .iter()
            .find(|i| i.is_folder && i.name == name)
            .map(|i| i.handle);
        let Some(handle) = handle else {
            self.error = format!("folder '{}' not found", name);
            return;
        };
        self.error.clear();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            match self.storage().list_objects(Some(handle)).await {
                Ok(objects) => {
                    self.items = objects.into_iter().map(to_item_info).collect();
                    self.path_segments.push((name.to_string(), handle));
                }
                Err(e) => self.error = fmt_mtp_error(e),
            }
        });
    }

    /// Go up one level.
    pub fn up(&mut self) {
        if self.path_segments.is_empty() {
            return;
        }
        self.path_segments.pop();
        self.error.clear();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            match self.storage().list_objects(self.current_handle()).await {
                Ok(objects) => self.items = objects.into_iter().map(to_item_info).collect(),
                Err(e) => self.error = fmt_mtp_error(e),
            }
        });
    }

    /// Build the current path string.
    pub fn current_path(&self) -> String {
        self.path_segments
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// Refresh the listing for the current folder.
    pub fn refresh(&mut self) {
        self.error.clear();
        let rt = tokio::runtime::Handle::current();
        rt.block_on(async {
            match self.storage().list_objects(self.current_handle()).await {
                Ok(objects) => self.items = objects.into_iter().map(to_item_info).collect(),
                Err(e) => self.error = fmt_mtp_error(e),
            }
        });
    }

    /// Go back to root.
    pub fn go_root(&mut self) {
        self.path_segments.clear();
        self.refresh();
    }
}

fn sanitize_filename(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}
