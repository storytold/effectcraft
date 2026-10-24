mod install;
mod instances;
mod release;

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::{self, Receiver, Sender},
};
use std::time::{Duration, Instant};

use effectcraft_ui_egui::{EffectcraftApp, RestartState};
use install::Install;
use release::Release;

#[derive(Clone)]
enum Phase {
    Idle,
    Checking(Option<Release>),
    Available(Release),
    Downloading(Release),
    Canceling(Release),
    Ready(Release),
    Failed(String),
}

enum Answer {
    Checked(Result<Option<Release>, String>),
    Downloaded(Result<Release, String>),
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct Preferences {
    disabled: bool,
    dismissed: String,
}

pub struct Updater {
    install: Option<Install>,
    instances: Option<instances::Instances>,
    transaction: Option<instances::Transaction>,
    phase: Phase,
    answers: (Sender<Answer>, Receiver<Answer>),
    actions: Receiver<String>,
    canceled: Arc<AtomicBool>,
    progress: Arc<AtomicU64>,
    visible: bool,
    preparing: bool,
    announced: bool,
    prefs: Preferences,
    next_check: Instant,
    next_poll: Instant,
    control_port: Option<u16>,
    status: Arc<std::sync::Mutex<serde_json::Value>>,
}

pub enum Registration {
    Managed(Install, instances::Instances),
    External,
    Unavailable(String),
}

impl Registration {
    pub fn new() -> Result<Self, String> {
        let install = match install::detect() {
            Ok(Some(install)) => install,
            Ok(None) => return Ok(Self::External),
            Err(e) => return Ok(Self::Unavailable(e)),
        };
        match instances::Instances::open(install.target()) {
            Ok(instances) => Ok(Self::Managed(install, instances)),
            Err(instances::RegistrationError::Busy) => {
                Err("An update is pending. Finish or cancel it in the other EffectCraft window before opening another instance.".into())
            }
            Err(instances::RegistrationError::Unavailable(e)) => Ok(Self::Unavailable(e)),
        }
    }
}

impl Updater {
    pub fn new(app: &mut EffectcraftApp, control_port: Option<u16>, registration: Registration) -> Self {
        let (install, instances, error) = match registration {
            Registration::Managed(install, instances) => (Some(install), Some(instances), None),
            Registration::External => (None, None, None),
            Registration::Unavailable(e) => {
                app.ui.status = format!("Updating is unavailable. {e}");
                (None, None, Some(e))
            }
        };
        let prefs = instances.as_ref().and_then(|i| instances::optional(&i.root.join("preferences.json"))).unwrap_or_default();
        let mut phase = instances
            .as_ref()
            .and_then(|i| {
                instances::optional::<Release>(&i.root.join("release.json")).filter(|r| {
                    semver::Version::parse(&r.version).ok().zip(semver::Version::parse(env!("CARGO_PKG_VERSION")).ok()).is_some_and(|(n, c)| n > c)
                        && install.as_ref().and_then(|kind| kind.suffix().ok()).is_some_and(|suffix| r.name == format!("effectcraft-{}-{suffix}", r.version))
                        && release::validate_download(r).is_ok()
                        && i.root.join(&r.name).is_file()
                })
            })
            .map_or(Phase::Idle, Phase::Available);
        if let Some(e) = error {
            phase = Phase::Failed(e);
        }
        let (tx, actions) = mpsc::channel();
        let status = Arc::new(std::sync::Mutex::new(serde_json::json!({"phase": "idle"})));
        let read_status = status.clone();
        app.hooks.update_status = Some(Box::new(move || read_status.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()));
        let previous = app.hooks.app_action.take();
        app.hooks.app_action = Some(Box::new(move |id| {
            if matches!(id, "help.checkForUpdates" | "app.updateStatus" | "app.downloadUpdate" | "app.installUpdate" | "app.cancelUpdate") {
                return tx.send(id.into()).is_ok();
            }
            previous.as_ref().is_some_and(|f| f(id))
        }));
        Self {
            install,
            instances,
            transaction: None,
            phase,
            answers: mpsc::channel(),
            actions,
            canceled: Arc::new(AtomicBool::new(false)),
            progress: Arc::new(AtomicU64::new(0)),
            visible: false,
            preparing: false,
            announced: false,
            prefs,
            next_check: Instant::now() + Duration::from_secs(10),
            next_poll: Instant::now(),
            control_port,
            status,
        }
    }

    fn check(&mut self, ctx: &egui::Context) {
        if matches!(self.phase, Phase::Checking(_) | Phase::Downloading(_) | Phase::Canceling(_)) || self.preparing {
            return;
        }
        self.next_check = Instant::now() + Duration::from_secs(6 * 3600);
        let Some(install) = &self.install else { return };
        let suffix = match install.suffix() {
            Ok(s) => s,
            Err(e) => {
                self.phase = Phase::Failed(e);
                return;
            }
        };
        let ready = if let Phase::Ready(r) = &self.phase { Some(r.clone()) } else { None };
        self.phase = Phase::Checking(ready);
        let tx = self.answers.0.clone();
        let ctx = ctx.clone();
        if let Err(e) = std::thread::Builder::new().name("effectcraft-update".into()).spawn(move || {
            let answer = release::latest(env!("CARGO_PKG_VERSION"), &suffix);
            let _ = tx.send(Answer::Checked(answer));
            ctx.request_repaint();
        }) {
            self.phase = Phase::Failed(format!("Could not start the update worker. {e}"));
        }
    }

    fn download(&mut self, ctx: &egui::Context) {
        let Phase::Available(release) = &self.phase else { return };
        let Some(instances) = &self.instances else { return };
        let release = release.clone();
        self.phase = Phase::Downloading(release.clone());
        self.canceled = Arc::new(AtomicBool::new(false));
        self.progress.store(0, Ordering::Relaxed);
        let (cancel, progress) = (self.canceled.clone(), self.progress.clone());
        let (tx, ctx, root) = (self.answers.0.clone(), ctx.clone(), instances.root.clone());
        if let Err(e) = std::thread::Builder::new().name("effectcraft-update".into()).spawn(move || {
            let result = (|| {
                let lock = instances::lock(&root.join("download.lock"))?;
                lock.try_lock().map_err(|_| "Another window is downloading an update.".to_string())?;
                release::download(&release, &root, &cancel, &progress)?;
                instances::write(&root.join("release.json"), &release)?;
                Ok(release)
            })();
            let _ = tx.send(Answer::Downloaded(result));
            ctx.request_repaint();
        }) {
            self.phase = Phase::Failed(format!("Could not start the update worker. {e}"));
        }
    }

    fn begin(&mut self) {
        if !matches!(self.phase, Phase::Ready(_)) || self.preparing {
            return;
        }
        if let Some(instances) = &self.instances {
            match instances.begin().and_then(|txn| {
                install::check_processes(&txn.participants)?;
                Ok(txn)
            }) {
                Ok(txn) => self.transaction = Some(txn),
                Err(e) => self.phase = Phase::Failed(e),
            }
        }
    }

    fn cancel(&mut self) {
        self.canceled.store(true, Ordering::Relaxed);
        if let Phase::Downloading(r) = &self.phase {
            self.phase = Phase::Canceling(r.clone());
        }
        if self.preparing
            && let Some(instances) = &self.instances
        {
            let _ = instances.cancel();
        }
    }

    pub fn tick(&mut self, app: &mut EffectcraftApp, ctx: &egui::Context) {
        while let Ok(action) = self.actions.try_recv() {
            self.visible = true;
            match action.as_str() {
                "help.checkForUpdates" => self.check(ctx),
                "app.downloadUpdate" => self.download(ctx),
                "app.installUpdate" => self.begin(),
                "app.cancelUpdate" => self.cancel(),
                _ => {}
            }
        }
        while let Ok(answer) = self.answers.1.try_recv() {
            let ready = if let Phase::Checking(ready) = &self.phase { ready.clone() } else { None };
            self.phase = match answer {
                Answer::Checked(Ok(Some(r))) if ready.as_ref().is_some_and(|old| old.version == r.version && old.sha256 == r.sha256) => Phase::Ready(r),
                Answer::Checked(Ok(Some(r))) => {
                    if self.prefs.dismissed != r.version {
                        self.visible = true;
                    }
                    Phase::Available(r)
                }
                Answer::Checked(Ok(None)) => Phase::Idle,
                Answer::Checked(Err(e)) => {
                    self.next_check = Instant::now() + Duration::from_secs(15 * 60);
                    if let Some(ready) = ready {
                        app.ui.status = e;
                        Phase::Ready(ready)
                    } else {
                        Phase::Failed(e)
                    }
                }
                Answer::Downloaded(Err(_)) if self.canceled.load(Ordering::Relaxed) => match &self.phase {
                    Phase::Canceling(r) => Phase::Available(r.clone()),
                    _ => Phase::Idle,
                },
                Answer::Downloaded(Err(e)) => Phase::Failed(e),
                Answer::Downloaded(Ok(r)) if !self.canceled.load(Ordering::Relaxed) => {
                    self.visible = true;
                    Phase::Ready(r)
                }
                Answer::Downloaded(Ok(r)) => Phase::Available(r),
            };
        }
        if !self.prefs.disabled && self.control_port.is_none() && Instant::now() >= self.next_check {
            self.check(ctx);
        }
        if self.instances.is_some() {
            ctx.request_repaint_after(Duration::from_millis(if self.preparing || matches!(self.phase, Phase::Downloading(_)) { 250 } else { 2000 }));
        }
        let (phase, version) = match &self.phase {
            Phase::Idle => ("idle", None),
            Phase::Checking(_) => ("checking", None),
            Phase::Failed(_) => ("failed", None),
            Phase::Available(r) => ("available", Some(&r.version)),
            Phase::Downloading(r) => ("downloading", Some(&r.version)),
            Phase::Canceling(r) => ("canceling", Some(&r.version)),
            Phase::Ready(r) => ("ready", Some(&r.version)),
        };
        let error = if let Phase::Failed(e) = &self.phase { Some(e) } else { None };
        *self.status.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = serde_json::json!({"phase": phase, "version": version, "error": error, "preparing": self.preparing, "downloaded": self.progress.load(Ordering::Relaxed), "supported": self.install.is_some()});
        if Instant::now() < self.next_poll {
            return;
        }
        self.next_poll = Instant::now() + Duration::from_millis(250);
        if let Err(e) = self.coordinate(app, ctx) {
            self.cancel();
            self.transaction = None;
            self.phase = Phase::Failed(e);
            self.visible = true;
            self.preparing = false;
            self.announced = false;
            app.cancel_restart();
        }
    }

    fn coordinate(&mut self, app: &mut EffectcraftApp, ctx: &egui::Context) -> Result<(), String> {
        let Some(instances) = &self.instances else { return Ok(()) };
        if !instances.requested()? {
            if self.preparing {
                self.preparing = false;
                self.announced = false;
                app.cancel_restart();
                app.ui.status = "Update canceled. Your windows remain open.".into();
            }
            self.transaction = None;
            return Ok(());
        }
        if !self.preparing {
            self.preparing = true;
            app.prepare_restart(ctx)?;
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        }
        match app.restart_state() {
            RestartState::Canceled => {
                instances.cancel()?;
                return Ok(());
            }
            RestartState::Ready => {
                if !self.announced {
                    instances.ready(&instances::Restart {
                        project: app.session.path.clone(),
                        config: effectcraft_host::config_dir(),
                        control_port: self.control_port,
                    })?;
                    self.announced = true;
                }
            }
            _ => return Ok(()),
        }
        if let Some(transaction) = &mut self.transaction
            && !instances.committed()
            && let Some(restarts) = instances.restarts(transaction)?
        {
            let Phase::Ready(release) = &self.phase else { return Err("No verified download is ready.".into()) };
            let install = self.install.clone().ok_or("This copy requires a manual update.")?;
            install::spawn(&instances.root, &install::Job { install, release: release.clone(), restarts })?;
            transaction.commit()?;
        }
        if instances.committed() {
            app.commit_restart(ctx);
        }
        Ok(())
    }

    pub fn frozen(&self, app: &EffectcraftApp) -> bool {
        self.preparing && app.restart_state() == RestartState::Ready
    }

    pub fn ui(&mut self, app: &mut EffectcraftApp, root: &mut egui::Ui) {
        let context = root.ctx().clone();
        let ctx = &context;
        if self.preparing {
            if self.frozen(app) {
                egui::CentralPanel::default().show(root, |ui| {
                    ui.heading("Preparing to update EffectCraft");
                    ui.label(
                        "Waiting for every EffectCraft window to approve. Finish saving in the other windows. No files are replaced until every window closes.",
                    );
                    let r = ui.button("Cancel update");
                    app.auto.add("update.cancel", r.rect, "Cancel update");
                    if !self.instances.as_ref().is_some_and(|i| i.committed())
                        && (r.clicked() || ctx.input(|i| i.viewport().close_requested() || i.key_pressed(egui::Key::Escape)))
                    {
                        self.cancel();
                        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    }
                });
            }
            return;
        }
        if !self.visible {
            return;
        }
        let mut visible = true;
        let mut dismiss = false;
        egui::Window::new("EffectCraft updates").open(&mut visible).collapsible(false).resizable(false).show(ctx, |ui| {
            ui.label(format!("Installed version {}", env!("CARGO_PKG_VERSION")));
            if self.install.is_none() {
                if let Phase::Failed(e) = &self.phase { ui.label(e); }
                ui.label("This copy is managed externally. Use your package manager, ArtCraft Launcher, or the release downloads to update it.");
            } else {
                match self.phase.clone() {
                    Phase::Idle => { ui.label("EffectCraft is up to date."); }
                    Phase::Checking(_) => { ui.spinner(); ui.label("Checking GitHub releases..."); }
                    Phase::Failed(e) => { ui.label(e); }
                    Phase::Available(r) => {
                        ui.label(format!("Version {} is available.", r.version));
                        ui.label("Download while you work. Installation and restart happen only when you choose.");
                        let button = ui.button("Download update");
                        app.auto.add("update.download", button.rect, "Download update");
                        if button.clicked() { self.download(ctx); }
                    }
                    Phase::Downloading(r) => {
                        let done = self.progress.load(Ordering::Relaxed);
                        ui.add(egui::ProgressBar::new((done as f64 / r.size.max(1) as f64) as f32).text(format!("{done} / {} bytes", r.size)));
                        let button = ui.button("Cancel download");
                        app.auto.add("update.cancel", button.rect, "Cancel download");
                        if button.clicked() { self.cancel(); }
                    }
                    Phase::Canceling(_) => { ui.label("Canceling the download. The current network request can take up to 45 seconds to stop."); }
                    Phase::Ready(r) => {
                        ui.label(format!("Version {} is downloaded and verified.", r.version));
                        ui.label("Every open instance will ask to save changes. Cancel in any window to stop. Projects and settings are kept and saved projects reopen after installation.");
                        let button = ui.button("Install and restart");
                        app.auto.add("update.install", button.rect, "Install and restart");
                        if button.clicked() { self.begin(); }
                    }
                }
                let button = ui.button("Check for updates");
                app.auto.add("update.check", button.rect, "Check for updates");
                if button.clicked() { self.check(ctx); }
                let mut automatic = !self.prefs.disabled;
                let checkbox = ui.checkbox(&mut automatic, "Check automatically every six hours");
                app.auto.add("update.automatic", checkbox.rect, "Check automatically every six hours");
                if checkbox.changed() {
                    self.prefs.disabled = !automatic;
                    self.save_preferences();
                }
            }
            let notes = ui.hyperlink_to("Release notes and downloads", release::RELEASES);
            app.auto.add("update.notes", notes.rect, "Release notes and downloads");
            let later = ui.button("Later");
            app.auto.add("update.later", later.rect, "Later");
            dismiss = later.clicked() || ctx.input(|i| i.key_pressed(egui::Key::Escape));
        });
        if !visible || dismiss {
            self.visible = false;
            if let Phase::Available(r) | Phase::Ready(r) = &self.phase {
                self.prefs.dismissed = r.version.clone();
            }
            self.save_preferences();
        }
    }

    fn save_preferences(&self) {
        if let Some(instances) = &self.instances
            && let Err(e) = instances::write(&instances.root.join("preferences.json"), &self.prefs)
        {
            log::warn!("Could not save update preferences. {e}");
        }
    }
}

impl Drop for Updater {
    fn drop(&mut self) {
        self.canceled.store(true, Ordering::Relaxed);
        if self.preparing && !self.instances.as_ref().is_some_and(|i| i.committed()) {
            self.cancel();
        }
    }
}

pub fn run_helper_if_requested() -> bool {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--apply-update")) {
        return false;
    }
    let root = args.next().map(PathBuf::from);
    let result = match &root {
        Some(root) if args.next().is_none() => install::helper(root.as_path()),
        _ => Err("The update helper needs exactly one state directory.".into()),
    };
    if let Err(e) = result {
        if let Some(root) = &root {
            let _ = instances::write(&root.join("transaction/cancel"), &true);
        }
        log::error!("Update failed. {e}");
        rfd::MessageDialog::new()
            .set_title("EffectCraft update failed")
            .set_description(format!("{e}\n\nSaved projects and settings were kept. Reopen EffectCraft or download an installer from GitHub to retry."))
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restart_reopens_a_saved_project_even_without_a_standard_extension() {
        let args = crate::parse_args(["--reopen-project", "my saved work"].map(String::from), Err(std::env::VarError::NotPresent)).unwrap().unwrap();
        assert_eq!(args.reopen_project.as_deref(), Some("my saved work"));
        assert!(args.files.is_empty());
        assert!(crate::parse_args(["--reopen-project"].map(String::from), Err(std::env::VarError::NotPresent)).is_err());
    }

    pub(super) fn release() -> Release {
        Release {
            tag: "v99.0.0".into(),
            version: "99.0.0".into(),
            name: "effectcraft-99.0.0-linux-x86_64.AppImage".into(),
            url: String::new(),
            size: 3,
            sha256: String::new(),
        }
    }

    #[test]
    fn download_and_install_are_distinct_visible_actions_and_ready_is_preserved() {
        let mut app = EffectcraftApp::new(effectcraft_engine::Session::default());
        let mut updater = Updater::new(&mut app, Some(9877), Registration::External);
        updater.install = Some(Install::AppImage("test.AppImage".into()));
        updater.visible = true;
        let ctx = egui::Context::default();
        updater.phase = Phase::Available(release());
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.auto.begin_frame();
            updater.ui(&mut app, ui);
        });
        output.textures_delta.clear();
        assert!(app.auto.elements.iter().any(|e| e.id == "update.download"));
        assert!(!app.auto.elements.iter().any(|e| e.id == "update.install"));
        updater.phase = Phase::Ready(release());
        updater.phase = Phase::Checking(Some(release()));
        updater.answers.0.send(Answer::Checked(Ok(Some(release())))).unwrap();
        updater.tick(&mut app, &ctx);
        assert!(matches!(updater.phase, Phase::Ready(_)));
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            app.auto.begin_frame();
            updater.ui(&mut app, ui);
        });
        output.textures_delta.clear();
        assert!(!app.auto.elements.iter().any(|e| e.id == "update.download"));
        assert!(app.auto.elements.iter().any(|e| e.id == "update.install"));
        updater.phase = Phase::Checking(Some(release()));
        updater.answers.0.send(Answer::Checked(Err("offline".into()))).unwrap();
        updater.tick(&mut app, &ctx);
        assert!(matches!(updater.phase, Phase::Ready(_)));
        updater.phase = Phase::Checking(Some(release()));
        let mut newer = release();
        newer.version = "100.0.0".into();
        updater.answers.0.send(Answer::Checked(Ok(Some(newer)))).unwrap();
        updater.tick(&mut app, &ctx);
        assert!(matches!(&updater.phase, Phase::Available(r) if r.version == "100.0.0"));
    }
}
