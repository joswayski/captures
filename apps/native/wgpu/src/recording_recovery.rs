//! Interrupted capture recovery on a serialized worker, separate from editor drafts.
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
};

use captures_media::{CancelToken, MediaToolchain};
use captures_recording::RecordingKind;
use captures_recording_platform::{
    RecordingRecovery, RecoveryDraft, RecoveryOutcome, RecoveryProgress,
};
use eframe::egui;

use crate::tokens::Tokens;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    session_id: String,
    identity: String,
}

impl Target {
    fn from_draft(draft: &RecoveryDraft) -> Option<Self> {
        (draft.status == "recoverable").then_some(())?;
        Some(Self {
            session_id: draft.session_id.clone(),
            identity: draft.identity.clone()?,
        })
    }
}

enum Pending {
    Listing,
    Recovering {
        cancel: CancelToken,
        directory: PathBuf,
    },
    Discarding,
}

enum Job {
    List,
    Recover(Target, CancelToken),
    Discard(Target),
    Shutdown,
}

enum Reply {
    Listed(Result<Vec<RecoveryDraft>, String>),
    Progress(RecoveryProgress),
    Recovered(Result<Box<RecoveryOutcome>, String>),
    Discarded(Result<&'static str, String>),
}

#[derive(Default)]
struct View {
    drafts: Vec<RecoveryDraft>,
    pending: Option<Pending>,
    refresh_after_list: bool,
    confirmation: Option<Target>,
    stage: Option<RecoveryProgress>,
    list_error: Option<String>,
    error: Option<String>,
    message: Option<String>,
    recovered: Option<(RecoveryOutcome, PathBuf)>,
}

impl View {
    fn current(&self, target: &Target) -> bool {
        self.drafts
            .iter()
            .any(|draft| Target::from_draft(draft).as_ref() == Some(target))
    }

    fn send(&mut self, tx: &Sender<Job>, job: Job, pending: Pending) {
        if let Err(error) = tx.send(job) {
            self.error = Some(format!("Recovery worker stopped: {error}"));
        } else {
            self.pending = Some(pending);
        }
    }

    fn refresh(&mut self, tx: &Sender<Job>) {
        // An in-flight list may precede recorder retirement. Coalesce another
        // refresh after it, so an old busy result cannot hide newly released media.
        // Mutations always refresh themselves after completion.
        if self.pending.is_some() {
            self.refresh_after_list |= matches!(self.pending, Some(Pending::Listing));
            return;
        }
        self.confirmation = None;
        self.send(tx, Job::List, Pending::Listing);
    }

    fn recover(&mut self, tx: &Sender<Job>, target: Target, directory: PathBuf) {
        if self.pending.is_some() || self.confirmation.is_some() || !self.current(&target) {
            return;
        }
        self.error = None;
        self.message = None;
        self.stage = Some(RecoveryProgress::Scanning);
        let cancel = CancelToken::default();
        self.send(
            tx,
            Job::Recover(target, cancel.clone()),
            Pending::Recovering { cancel, directory },
        );
    }

    fn discard(&mut self, tx: &Sender<Job>) {
        let Some(target) = self.confirmation.take() else {
            return;
        };
        if self.pending.is_some() || !self.current(&target) {
            self.error = Some("Recording changed. Refresh and confirm again.".into());
            return;
        }
        self.error = None;
        self.message = None;
        self.send(tx, Job::Discard(target), Pending::Discarding);
    }

    fn receive(&mut self, tx: &Sender<Job>, reply: Reply) {
        match reply {
            Reply::Listed(result) => {
                self.pending = None;
                match result {
                    Ok(drafts) => {
                        self.drafts = drafts;
                        self.list_error = None;
                    }
                    Err(error) => {
                        self.drafts.clear();
                        self.list_error = Some(error);
                    }
                }
                if std::mem::take(&mut self.refresh_after_list) {
                    self.refresh(tx);
                }
            }
            Reply::Progress(stage) => {
                if matches!(self.pending, Some(Pending::Recovering { .. })) {
                    self.stage = Some(stage);
                }
            }
            Reply::Recovered(result) => {
                let Some(Pending::Recovering { directory, .. }) = self.pending.take() else {
                    return;
                };
                self.stage = None;
                match result {
                    Ok(outcome) => {
                        // A late cancellation cannot conceal committed media.
                        self.message = Some(
                            outcome
                                .warning
                                .clone()
                                .unwrap_or_else(|| "Recording recovered into History.".into()),
                        );
                        self.recovered = Some((*outcome, directory));
                    }
                    Err(error) => self.error = Some(error),
                }
                self.refresh(tx);
            }
            Reply::Discarded(result) => {
                self.pending = None;
                match result {
                    Ok(_) => self.message = Some("Interrupted recording discarded.".into()),
                    Err(error) => self.error = Some(error),
                }
                self.refresh(tx);
            }
        }
    }

    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        tokens: &Tokens,
        enabled: bool,
        tx: &Sender<Job>,
    ) -> Option<Target> {
        if self.drafts.is_empty()
            && self.pending.is_none()
            && self.list_error.is_none()
            && self.error.is_none()
            && self.message.is_none()
        {
            return None;
        }
        let idle = self.pending.is_none();
        let mut refresh = false;
        let mut confirm = None;
        let mut discard = false;
        let mut recover = None;
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.strong("Interrupted recordings");
            refresh = ui
                .add_enabled(enabled && idle, egui::Button::new("Refresh").small())
                .clicked();
        });
        let height = (ui.available_height() * 0.5).min(240.);
        egui::ScrollArea::vertical().id_salt("recording-recovery").max_height(height).show(ui, |ui| {
            if matches!(self.pending, Some(Pending::Listing)) {
                ui.label("Checking recovery files…");
            }
            if let Some(Pending::Recovering { cancel, .. }) = &self.pending {
                ui.label(match self.stage {
                    Some(RecoveryProgress::Assembling) => "Assembling playable segments…",
                    Some(RecoveryProgress::Poster) => "Preparing recovered preview…",
                    Some(RecoveryProgress::Publishing) => "Adding recording to History…",
                    _ => "Checking playable segments…",
                });
                if ui.add_enabled(!cancel.is_cancelled() && !matches!(self.stage, Some(RecoveryProgress::Publishing)),
                    egui::Button::new(if cancel.is_cancelled() { "Cancelling…" } else { "Cancel recovery" }))
                    .on_hover_text("Cancellation stops preparation; it cannot undo a recording already published to History.").clicked()
                {
                    cancel.cancel();
                }
            }
            if matches!(self.pending, Some(Pending::Discarding)) {
                ui.label("Discarding interrupted recording…");
            }
            for error in [&self.list_error, &self.error].into_iter().flatten() {
                ui.colored_label(tokens.color("danger-text"), error);
            }
            if let Some(message) = &self.message {
                ui.label(message);
            }
            for draft in &self.drafts {
                ui.push_id(&draft.session_id, |ui| {
                    ui.group(|ui| {
                        ui.strong(match draft.kind {
                            Some(RecordingKind::Video) => "Video recording",
                            Some(RecordingKind::Gif) => "GIF recording",
                            None => "Unavailable recording",
                        });
                        let date = draft.created_at_ms
                            .and_then(|ms| i64::try_from(ms).ok())
                            .and_then(chrono::DateTime::from_timestamp_millis)
                            .map(|date| date.with_timezone(&chrono::Local).format("%b %d, %H:%M").to_string())
                            .unwrap_or_else(|| "Unknown time".into());
                        let seconds = draft.completed_duration_ms / 1000;
                        ui.label(format!("{date} · {}:{:02} completed", seconds / 60, seconds % 60));
                        if let Some(reason) = &draft.reason {
                            ui.label(reason);
                        }
                        let target = Target::from_draft(draft);
                        ui.horizontal(|ui| {
                            let available = enabled && idle && self.confirmation.is_none() && target.is_some();
                            if ui.add_enabled(available, egui::Button::new("Recover")).clicked() {
                                recover = target.clone();
                            }
                            if ui.add_enabled(available, egui::Button::new("Discard…")).clicked() {
                                confirm = target;
                            }
                        });
                    });
                });
            }
        });
        ui.separator();
        if let Some(target) = confirm {
            self.confirmation = Some(target);
        }
        if let Some(target) = &self.confirmation {
            let mut keep = false;
            egui::Window::new("Discard interrupted recording?")
                .id(egui::Id::unique("confirm-recording-discard"))
                .collapsible(false).resizable(false).default_width(360.)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ui.ctx(), |ui| {
                    ui.label("Permanently deletes this recording's recovery files. This cannot be undone.");
                    ui.monospace(&target.session_id);
                    ui.horizontal(|ui| {
                        keep = ui.button("Keep recording").clicked();
                        discard = ui.add_enabled(enabled && idle,
                            egui::Button::new(egui::RichText::new("Discard permanently")
                                .color(tokens.color("danger-text")))).clicked();
                    });
                });
            if keep
                || ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            {
                self.confirmation = None;
            }
        }
        if refresh {
            self.error = None;
            self.message = None;
            self.refresh(tx);
        }
        if discard {
            self.discard(tx);
        }
        recover
    }
}

pub struct Recovery {
    view: View,
    tx: Sender<Job>,
    rx: Receiver<Reply>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Recovery {
    pub fn new(ctx: egui::Context, root: PathBuf) -> Self {
        let (tx, jobs) = mpsc::channel();
        let (out, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            let service = RecordingRecovery::new(root, MediaToolchain::from_command_names());
            while let Ok(job) = jobs.recv() {
                let reply = match job {
                    Job::List => Reply::Listed(service.list()),
                    Job::Recover(target, cancel) => Reply::Recovered(
                        service
                            .recover(&target.session_id, &target.identity, &cancel, |stage| {
                                let _ = out.send(Reply::Progress(stage));
                                ctx.request_repaint_of(egui::ViewportId::ROOT);
                            })
                            .map(Box::new),
                    ),
                    Job::Discard(target) => {
                        Reply::Discarded(service.discard(&target.session_id, &target.identity))
                    }
                    Job::Shutdown => break,
                };
                let _ = out.send(reply);
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
        });
        Self {
            view: View::default(),
            tx,
            rx,
            worker: Some(worker),
        }
    }

    pub fn blocking(&self) -> bool {
        self.view.pending.is_some() || self.view.confirmation.is_some()
    }

    pub fn refresh(&mut self) {
        self.view.refresh(&self.tx);
    }

    pub fn receive(&mut self) -> Option<(RecoveryOutcome, PathBuf)> {
        while let Ok(reply) = self.rx.try_recv() {
            self.view.receive(&self.tx, reply);
        }
        self.view.recovered.take()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, tokens: &Tokens, enabled: bool) -> Option<Target> {
        self.view.ui(ui, tokens, enabled, &self.tx)
    }

    pub fn recover(&mut self, target: Target, directory: PathBuf) {
        self.view.recover(&self.tx, target, directory);
    }

    pub fn can_quit(&self) -> Result<(), String> {
        if matches!(
            self.view.pending,
            Some(Pending::Recovering { .. } | Pending::Discarding)
        ) {
            return Err(
                "Cancel or wait for interrupted-recording recovery before quitting.".into(),
            );
        }
        if self.view.confirmation.is_some() {
            return Err("Dismiss the recording discard confirmation before quitting.".into());
        }
        Ok(())
    }
}

impl Drop for Recovery {
    fn drop(&mut self) {
        if let Some(Pending::Recovering { cancel, .. }) = &self.view.pending {
            cancel.cancel();
        }
        let _ = self.tx.send(Job::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_wakes_root_while_an_editor_is_active() {
        let ctx = egui::Context::default();
        let wakes = crate::root_repaint::observe_from_child(&ctx);
        let root = tempfile::tempdir().unwrap();
        let mut recovery = Recovery::new(ctx.clone(), root.path().into());
        recovery.refresh();
        assert_eq!(
            wakes
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap(),
            egui::ViewportId::ROOT
        );
        recovery.receive();
        assert!(recovery.view.pending.is_none());
        assert!(recovery.view.list_error.is_none());
        assert!(recovery.view.drafts.is_empty());
        ctx.end_pass().textures_delta.clear();
    }

    fn draft(identity: &str) -> RecoveryDraft {
        RecoveryDraft {
            session_id: "selected-bundle".into(),
            status: "recoverable",
            kind: Some(RecordingKind::Video),
            created_at_ms: Some(1_780_000_000_000),
            completed_duration_ms: 7_300,
            identity: Some(identity.into()),
            reason: None,
        }
    }

    fn view() -> View {
        View {
            drafts: vec![draft("before")],
            ..View::default()
        }
    }

    #[test]
    fn retirement_refresh_retries_a_list_started_while_recorder_held_lease() {
        let (tx, jobs) = mpsc::channel();
        let mut view = view();
        view.refresh(&tx);
        assert!(matches!(jobs.try_recv().unwrap(), Job::List));
        view.refresh(&tx);
        view.refresh(&tx);
        assert!(jobs.try_recv().is_err());
        view.receive(&tx, Reply::Listed(Err("Recorder still active".into())));
        assert!(matches!(jobs.try_recv().unwrap(), Job::List));
        assert!(jobs.try_recv().is_err(), "refreshes must coalesce");
        view.receive(&tx, Reply::Listed(Ok(vec![draft("released")])));
        assert!(view.pending.is_none() && view.list_error.is_none());
        assert_eq!(view.drafts[0].identity.as_deref(), Some("released"));
        assert!(jobs.try_recv().is_err());
    }

    #[test]
    fn stale_unavailable_and_busy_targets_never_dispatch() {
        let (tx, jobs) = mpsc::channel();
        let mut view = view();
        let target = Target::from_draft(&view.drafts[0]).unwrap();
        view.confirmation = Some(target.clone());
        view.drafts[0].identity = Some("changed".into());
        view.discard(&tx);
        assert!(jobs.try_recv().is_err());
        assert!(view.error.as_deref().unwrap().contains("changed"));
        view.recover(&tx, target, "output".into());
        assert!(jobs.try_recv().is_err());
        view.drafts[0].status = "unavailable";
        assert!(Target::from_draft(&view.drafts[0]).is_none());
        view.drafts[0].status = "recoverable";
        view.drafts[0].identity = None;
        assert!(Target::from_draft(&view.drafts[0]).is_none());
        view.drafts = vec![draft("new")];
        let target = Target::from_draft(&view.drafts[0]).unwrap();
        view.pending = Some(Pending::Listing);
        view.recover(&tx, target, "output".into());
        assert!(jobs.try_recv().is_err());
    }

    #[test]
    fn discard_requires_current_confirmation_and_error_survives_refresh() {
        let (tx, jobs) = mpsc::channel();
        let mut view = view();
        view.discard(&tx);
        assert!(jobs.try_recv().is_err());
        let target = Target::from_draft(&view.drafts[0]).unwrap();
        view.confirmation = Some(target.clone());
        view.discard(&tx);
        assert!(matches!(jobs.try_recv().unwrap(), Job::Discard(sent) if sent == target));
        view.receive(&tx, Reply::Discarded(Err("Permission denied".into())));
        assert!(matches!(jobs.try_recv().unwrap(), Job::List));
        view.receive(&tx, Reply::Listed(Ok(vec![draft("retry")])));
        assert_eq!(view.error.as_deref(), Some("Permission denied"));
        let retry = Target::from_draft(&view.drafts[0]).unwrap();
        view.confirmation = Some(retry.clone());
        view.discard(&tx);
        assert!(matches!(jobs.try_recv().unwrap(), Job::Discard(sent) if sent == retry));
        assert!(view.error.is_none());
    }

    #[test]
    fn recover_failure_refreshes_then_retries_with_new_identity() {
        let (tx, jobs) = mpsc::channel();
        let mut view = view();
        let target = Target::from_draft(&view.drafts[0]).unwrap();
        view.recover(&tx, target.clone(), "output".into());
        assert!(matches!(jobs.try_recv().unwrap(), Job::Recover(sent, _) if sent == target));
        view.refresh(&tx);
        assert!(
            jobs.try_recv().is_err(),
            "busy refresh must not enqueue stale work"
        );
        view.receive(&tx, Reply::Recovered(Err("History is read-only".into())));
        assert!(matches!(jobs.try_recv().unwrap(), Job::List));
        view.receive(&tx, Reply::Listed(Ok(vec![draft("retry")])));
        assert_eq!(view.error.as_deref(), Some("History is read-only"));
        let retry = Target::from_draft(&view.drafts[0]).unwrap();
        view.recover(&tx, retry.clone(), "other-output".into());
        assert!(matches!(jobs.try_recv().unwrap(), Job::Recover(sent, _) if sent == retry));
    }

    #[test]
    fn late_cancel_does_not_hide_published_artifact_or_cleanup_warning() {
        let (tx, jobs) = mpsc::channel();
        let mut view = view();
        let target = Target::from_draft(&view.drafts[0]).unwrap();
        view.recover(&tx, target, "chosen-directory".into());
        let Job::Recover(_, cancel) = jobs.try_recv().unwrap() else {
            panic!("recover job")
        };
        view.receive(&tx, Reply::Progress(RecoveryProgress::Publishing));
        cancel.cancel();
        let outcome = RecoveryOutcome {
            status: "recovered",
            path: "history/new/recording.mp4".into(),
            warning: Some("Saved; source cleanup failed".into()),
            entry: serde_json::from_value(serde_json::json!({
                "id":"published", "kind":"video", "created_at":"2026-05-01T00:00:00Z",
                "width":160, "height":90, "size_bytes":4731, "preview_url":"", "full_url":""
            }))
            .unwrap(),
        };
        view.receive(&tx, Reply::Recovered(Ok(Box::new(outcome))));
        let (saved, directory) = view.recovered.take().unwrap();
        assert_eq!(saved.entry.id, "published");
        assert_eq!(directory, PathBuf::from("chosen-directory"));
        assert_eq!(
            view.message.as_deref(),
            Some("Saved; source cleanup failed")
        );
        assert!(view.error.is_none());
        assert!(matches!(jobs.try_recv().unwrap(), Job::List));
    }

    #[test]
    fn quit_waits_for_mutation_or_confirmation_but_not_an_idle_list() {
        let (tx, _jobs) = mpsc::channel();
        let (_out, rx) = mpsc::channel();
        let mut recovery = Recovery {
            view: view(),
            tx,
            rx,
            worker: None,
        };
        assert!(recovery.can_quit().is_ok());
        recovery.view.pending = Some(Pending::Listing);
        assert!(recovery.can_quit().is_ok());
        recovery.view.pending = Some(Pending::Discarding);
        assert!(recovery.can_quit().is_err());
        recovery.view.pending = None;
        recovery.view.confirmation = Target::from_draft(&recovery.view.drafts[0]);
        assert!(recovery.can_quit().is_err());
        recovery.view.confirmation = None;
        let target = Target::from_draft(&recovery.view.drafts[0]).unwrap();
        recovery.recover(target, "output".into());
        assert!(recovery.can_quit().is_err());
        if let Some(Pending::Recovering { cancel, .. }) = &recovery.view.pending {
            cancel.cancel();
        }
        assert!(
            recovery.can_quit().is_err(),
            "cancellation still needs the worker result"
        );
    }
}
