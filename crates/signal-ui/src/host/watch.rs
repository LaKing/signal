//! The project folder's watch: live reloads of the running project's files,
//! the stopped project's graph push, and the pattern files.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use signal_project::PATTERNS;
use signal_project::live::{
    ProjectWatcher, ReloadError, WATCH_INTERVAL_MS, is_presentation, reload_observed,
};

use crate::panel::Panel;
use crate::panel_write::sha256_hex;

use super::run::{Observed, take_muted};
use super::{Host, joined};

impl Host {
    /// Every 100 ms: a changed project folder is applied live.
    pub(super) fn watch(&mut self) {
        let Some(run) = self.run.as_mut() else {
            return;
        };
        if run.watched.elapsed() < Duration::from_millis(WATCH_INTERVAL_MS) {
            return;
        }
        run.watched = Instant::now();
        let changed = match run.watcher.as_mut().map(ProjectWatcher::changes) {
            Some(Ok(changed)) => changed,
            Some(Err(error)) => {
                self.messages.push(format!("watch: {error}"));
                Vec::new()
            }
            None => Vec::new(),
        };
        if changed.is_empty() && !run.retry {
            return;
        }
        let files: Vec<String> = changed
            .iter()
            .map(|f| f.to_string_lossy().into_owned())
            .collect();
        // Presentation only (`ui/…`): no load reads it, so nothing is
        // reloaded or planned; the clients are told, and `graph` (the
        // schematic's positions) is pushed.
        if !run.retry && changed.iter().all(|f| is_presentation(f)) {
            let message = format!(
                "presentation files changed ({}): nothing to apply",
                files.join(", ")
            );
            self.messages.push(format!("watch: {message}"));
            if let Some(mode) = self.app.as_mut() {
                mode.reload = Some((message.clone(), false));
            }
            self.broadcast(&json!({
                "type": "reload",
                "sets": 0,
                "swap": null,
                "removed": 0,
                "replaced": 0,
                "loadSwap": null,
                "message": message,
                "files": files,
                "presentation": true,
            }));
            self.push_graph(false);
            return;
        }
        run.retry = false;
        self.reloads += 1;
        // Read before the reload: a later edit makes it differ from the file.
        let file = signal_project::read_regular_to_string(
            self.opening.dir.join(signal_project::PROJECT_FILE),
        )
        .ok();
        run.file = None;
        let mut observed = Observed {
            values: &mut self.live.values,
            reports: &mut self.reports,
            muted: &mut self.muted,
        };
        let result = reload_observed(
            &mut run.controller,
            &mut run.live,
            &mut observed,
            &mut run.current,
            &self.opening.dir,
            &self.registry,
        );
        // This run's, taken now: a later run must never show them.
        take_muted(&mut self.muted, run, self.plan.as_ref(), &mut self.messages);
        let mut reloaded = None;
        // A swap's `graph` is pushed once it commits (`poll`).
        let swapping = matches!(&result, Ok(applied) if applied.swap.is_some());
        let notice = match result {
            Ok(applied) => {
                run.file = file;
                reloaded = Some((
                    Panel::new(&run.current, &self.registry),
                    run.current.plan(&self.registry).ok().map(|p| p.plan),
                ));
                self.messages.push(format!("watch: {applied}"));
                let edited = applied.sets > 0 || applied.swap.is_some();
                (
                    json!({
                        "type": "reload",
                        "sets": applied.sets,
                        "swap": applied.swap,
                        "removed": applied.removed,
                        "replaced": applied.replaced,
                        "loadSwap": applied.load_swap,
                        "message": applied.to_string(),
                        "files": files,
                        "presentation": false,
                    }),
                    edited && applied.swap.is_none(),
                )
            }
            Err(ReloadError::Invalid(diagnostics)) => {
                let message = format!("{}; keeping the last good version", joined(diagnostics));
                self.messages.push(format!("watch: {message}"));
                (json!({"type": "reload.error", "message": message}), false)
            }
            Err(ReloadError::Failed(message)) => {
                run.retry = true;
                let message = format!("{message}; keeping the last good version, retrying");
                self.messages.push(format!("watch: {message}"));
                (json!({"type": "reload.error", "message": message}), false)
            }
        };
        if let Some((panel, plan)) = reloaded {
            // The running project changed (a swap is a new version too).
            self.graph_playing = None;
            // The reload sent every port what differs from the file: a
            // control written while its live send was refused now holds it.
            for (instance, parameter) in std::mem::take(&mut self.live.resend) {
                self.panel.release(&instance, &parameter);
            }
            self.reconcile(panel);
            self.replace_plan(plan);
            self.reshape_vectors(true);
        }
        if let Some(mode) = self.app.as_mut() {
            let error = notice.0.get("type") == Some(&json!("reload.error"));
            let message = notice.0.get("message").and_then(Value::as_str);
            mode.reload = message.map(|m| (m.to_owned(), error));
        }
        self.take_reports();
        self.broadcast(&notice.0);
        // A swap is described once it commits (`poll`); new bases now.
        if notice.1 {
            let describe = self.describe(false);
            self.broadcast(&describe);
        }
        // `ui/schematic.toml` changes come this way too.
        if !swapping {
            self.push_graph(false);
        }
        self.push_patterns(&changed);
    }

    /// Every pattern file among `changed` (`patterns/<name>.toml`, the
    /// project's own) pushed as `pattern.written` with its text and base
    /// (`origin: "file"`): a pattern changed on disk (an editor, another
    /// program, an agent writing files) reaches the open pattern editors
    /// at once, as another client's `pattern.write` does (external review,
    /// C). A removed file, or one reached through a link, is not pushed.
    fn push_patterns(&mut self, changed: &[PathBuf]) {
        for file in changed {
            let in_patterns = file.parent() == Some(Path::new(PATTERNS))
                && file.extension().is_some_and(|e| e == "toml");
            let Some(name) = file
                .file_stem()
                .and_then(|n| n.to_str())
                .filter(|_| in_patterns)
            else {
                continue;
            };
            let message = json!({"name": name});
            let Ok((name, path, linked)) = self.pattern_path(&message) else {
                continue;
            };
            if linked {
                continue;
            }
            let Ok(text) = signal_project::read_regular_to_string(&path) else {
                continue;
            };
            self.broadcast(&json!({
                "type": "pattern.written",
                "name": name,
                "base": sha256_hex(text.as_bytes()),
                "text": text,
                "origin": "file",
            }));
        }
    }

    /// App mode, stopped: every 100 ms a changed folder pushes `graph` (its
    /// structure or its positions). Not while switched off: switching on
    /// pushes what changed meanwhile (`handle`).
    pub(super) fn watch_stopped(&mut self) {
        let opened = self.app.as_ref().is_some_and(|m| m.opened);
        if self.run.is_some() || !opened {
            self.idle_watch = None;
            return;
        }
        if !self.powered() {
            return;
        }
        let Some((watcher, polled)) = self.idle_watch.as_mut() else {
            self.idle_watch = ProjectWatcher::new(&self.opening.dir)
                .ok()
                .map(|w| (w, Instant::now()));
            return;
        };
        if polled.elapsed() < Duration::from_millis(WATCH_INTERVAL_MS) {
            return;
        }
        *polled = Instant::now();
        let changed = watcher.changes().unwrap_or_default();
        if !changed.is_empty() {
            // Waiting for the device: the project's files changed (its
            // `sample_rate`, say), so it is tried again.
            if let Some(wait) = self.app.as_mut().and_then(|m| m.device_wait.as_mut()) {
                wait.once = true;
            }
            self.push_graph(false);
            self.push_patterns(&changed);
        }
    }

    /// Takes `file` as it is now as seen by the watches.
    pub(super) fn seen(&mut self, file: &Path) {
        let run = self.run.as_mut().and_then(|r| r.watcher.as_mut());
        let idle = self.idle_watch.as_mut().map(|(w, _)| w);
        for watcher in run.into_iter().chain(idle) {
            let stamp = watcher.stamp(file);
            watcher.accept(file, stamp);
        }
    }
}
