use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Restart {
    pub project: Option<String>,
    pub config: Option<PathBuf>,
    pub control_port: Option<u16>,
}

pub struct Instances {
    pub root: PathBuf,
    pub id: String,
    _running: File,
    _instance: File,
}

#[derive(Debug)]
pub enum RegistrationError {
    Busy,
    Unavailable(String),
}

impl From<String> for RegistrationError {
    fn from(error: String) -> Self {
        Self::Unavailable(error)
    }
}

fn registration_lock_error(error: TryLockError) -> RegistrationError {
    match error {
        TryLockError::WouldBlock => RegistrationError::Busy,
        _ => RegistrationError::Unavailable(error.to_string()),
    }
}

pub struct Transaction {
    _owner: File,
    pub participants: Vec<String>,
    root: PathBuf,
    committed: bool,
}

pub fn lock(path: &Path) -> Result<File, String> {
    OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path).map_err(|e| format!("Could not open {}. {e}", path.display()))
}

pub fn available(file: &File) -> Result<bool, String> {
    match file.try_lock() {
        Ok(()) => {
            file.unlock().map_err(|e| e.to_string())?;
            Ok(true)
        }
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(e) => Err(e.to_string()),
    }
}

pub fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    use std::io::Read;
    let mut bytes = Vec::new();
    File::open(path).map_err(|e| e.to_string())?.take(65537).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("Update state exceeds 64 KiB.".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("Invalid update state. {e}"))
}

pub fn optional<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    if !path.exists() {
        return None;
    }
    match read(path) {
        Ok(value) => Some(value),
        Err(e) => {
            log::warn!("Ignoring invalid update state at {}. {e}", path.display());
            None
        }
    }
}

pub fn write(path: &Path, value: &impl Serialize) -> Result<(), String> {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or("No update folder.")?).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("Update state exceeds 64 KiB. Close some windows before updating.".into());
    }
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    file.as_file().sync_all().map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

impl Instances {
    pub fn open(target: &Path) -> Result<Self, RegistrationError> {
        let platform = effectcraft_host::app_dirs::resolve(
            |key| {
                if key == "EFFECTCRAFT_CONFIG_DIR" { None } else { std::env::var_os(key) }
            },
            None,
        )
        .dir
        .ok_or_else(|| "No user configuration folder is available for updates.".to_string())?;
        let canonical = target.canonicalize().map_err(|e| e.to_string())?;
        let key = format!("{:x}", Sha256::digest(canonical.as_os_str().as_encoded_bytes()));
        Self::under(platform.join("Updates").join(key))
    }

    fn under(root: PathBuf) -> Result<Self, RegistrationError> {
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        let running = lock(&root.join("running.lock"))?;
        running.try_lock_shared().map_err(registration_lock_error)?;
        let owner = lock(&root.join("owner.lock"))?;
        owner.try_lock().map_err(registration_lock_error)?;
        if root.join("transaction/commit").exists() && !root.join("transaction/cancel").exists() {
            if !available(&lock(&root.join("helper.lock"))?)? {
                return Err(RegistrationError::Busy);
            }
            log::warn!("Recovering an interrupted update at {}.", root.display());
            if let Err(e) = write(&root.join("transaction/cancel"), &true) {
                log::warn!("Could not record interrupted update recovery. {e}");
            }
        }
        let id = std::process::id().to_string();
        let instance = lock(&root.join(format!("{id}.instance")))?;
        instance.try_lock().map_err(|e| format!("Could not register this EffectCraft instance. {e}"))?;
        Ok(Self { root, id, _running: running, _instance: instance })
    }

    fn live(&self, id: &str) -> Result<bool, String> {
        Ok(!available(&lock(&self.root.join(format!("{id}.instance")))?)?)
    }

    pub fn begin(&self) -> Result<Transaction, String> {
        let owner = lock(&self.root.join("owner.lock"))?;
        owner.try_lock().map_err(|_| "Another window is already preparing an update.".to_string())?;
        let txn = self.root.join("transaction");
        if txn.exists() {
            std::fs::remove_dir_all(&txn).map_err(|e| e.to_string())?;
        }
        std::fs::create_dir(&txn).map_err(|e| e.to_string())?;
        let mut participants = Vec::new();
        for item in std::fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let path = item.map_err(|e| e.to_string())?.path();
            if path.extension().is_some_and(|e| e == "instance") {
                let id = path.file_stem().and_then(|s| s.to_str()).ok_or("Invalid instance id.")?;
                if id.len() > 10 || !id.bytes().all(|b| b.is_ascii_digit()) {
                    return Err("Invalid instance id.".into());
                }
                if self.live(id)? {
                    participants.push(id.to_string());
                }
                if participants.len() > 128 {
                    return Err("Close some EffectCraft instances before updating.".into());
                }
            }
        }
        write(&txn.join("request.json"), &participants)?;
        Ok(Transaction { _owner: owner, participants, root: txn, committed: false })
    }

    pub fn requested(&self) -> Result<bool, String> {
        let txn = self.root.join("transaction");
        if !txn.join("request.json").exists() || txn.join("cancel").exists() {
            return Ok(false);
        }
        let participants: Vec<String> = read(&txn.join("request.json"))?;
        if !participants.contains(&self.id) {
            return Ok(false);
        }
        if !txn.join("commit").exists() && available(&lock(&self.root.join("owner.lock"))?)? {
            return Ok(false);
        }
        Ok(true)
    }

    pub fn ready(&self, restart: &Restart) -> Result<(), String> {
        write(&self.root.join("transaction").join(format!("{}.ready", self.id)), restart)
    }

    pub fn cancel(&self) -> Result<(), String> {
        write(&self.root.join("transaction/cancel"), &true)
            .or_else(|_| std::fs::remove_file(self.root.join("transaction/request.json")).map_err(|e| e.to_string()))
    }

    pub fn committed(&self) -> bool {
        self.root.join("transaction/commit").exists() && !self.root.join("transaction/cancel").exists()
    }

    pub fn restarts(&self, transaction: &Transaction) -> Result<Option<Vec<Restart>>, String> {
        if self.root.join("transaction/cancel").exists() {
            return Err("Update canceled in an EffectCraft window.".into());
        }
        let mut restarts = Vec::new();
        for id in &transaction.participants {
            let path = self.root.join("transaction").join(format!("{id}.ready"));
            if path.exists() {
                restarts.push(read(&path)?);
            } else if self.live(id)? {
                return Ok(None);
            }
        }
        Ok(Some(restarts))
    }
}

impl Transaction {
    pub fn commit(&mut self) -> Result<(), String> {
        if !self.root.join("request.json").is_file() || self.root.join("cancel").exists() {
            return Err("Update canceled.".into());
        }
        write(&self.root.join("commit"), &true)?;
        self.committed = true;
        Ok(())
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_update_excludes_new_instances_and_waits_for_every_approval() {
        let dir = tempfile::tempdir().unwrap();
        let instances = Instances::under(dir.path().into()).unwrap();
        let other = lock(&dir.path().join("999999.instance")).unwrap();
        other.lock().unwrap();
        let txn = instances.begin().unwrap();
        assert!(instances.requested().unwrap());
        assert!(Instances::under(dir.path().into()).is_err());
        let restart = Restart { project: Some("saved.ecproj".into()), config: None, control_port: None };
        instances.ready(&restart).unwrap();
        assert!(instances.restarts(&txn).unwrap().is_none());
        write(&dir.path().join("transaction/999999.ready"), &restart).unwrap();
        assert_eq!(instances.restarts(&txn).unwrap().unwrap().len(), 2);
        instances.cancel().unwrap();
        assert!(!instances.requested().unwrap());
        assert!(instances.restarts(&txn).is_err());
        drop(txn);
        assert!(!dir.path().join("transaction").exists());
    }

    #[test]
    fn installation_lock_cannot_be_taken_until_all_instances_exit() {
        let dir = tempfile::tempdir().unwrap();
        let instance = Instances::under(dir.path().into()).unwrap();
        let install = lock(&dir.path().join("running.lock")).unwrap();
        assert!(!available(&install).unwrap());
        drop(instance);
        assert!(available(&install).unwrap());
    }

    #[test]
    fn an_abandoned_committed_update_does_not_prevent_launching() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("transaction")).unwrap();
        write(&dir.path().join("transaction/commit"), &true).unwrap();
        let instance = Instances::under(dir.path().into()).unwrap();
        assert!(!instance.committed());
        assert!(dir.path().join("transaction/cancel").is_file());
    }

    #[test]
    fn committed_restart_can_be_polled_again_without_starting_another_helper() {
        use super::super::{Phase, Registration, Updater, install::Install};
        let dir = tempfile::tempdir().unwrap();
        let instances = Instances::under(dir.path().into()).unwrap();
        let mut transaction = instances.begin().unwrap();
        transaction.commit().unwrap();
        let helper = dir.path().join(if cfg!(windows) { "update-helper.exe" } else { "update-helper" });
        std::fs::create_dir(&helper).unwrap();
        let mut app = effectcraft_ui_egui::EffectcraftApp::new(effectcraft_engine::Session::default());
        let ctx = egui::Context::default();
        let mut updater = Updater::new(&mut app, None, Registration::Managed(Install::AppImage(dir.path().join("app")), instances));
        updater.transaction = Some(transaction);
        updater.phase = Phase::Ready(super::super::tests::release());
        updater.coordinate(&mut app, &ctx).unwrap();
        updater.coordinate(&mut app, &ctx).unwrap();
        assert_eq!(app.restart_state(), effectcraft_ui_egui::RestartState::Ready);
        assert!(!dir.path().join("transaction/job.json").exists());
        assert!(helper.is_dir());
    }
}
