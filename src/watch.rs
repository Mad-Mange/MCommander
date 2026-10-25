//! Automatic refresh: watches the directories shown in both panels and
//! reports them as dirty once file system activity has settled.

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct State {
    dirty: HashSet<String>,
    first: Option<Instant>,
    last: Option<Instant>,
}

pub struct DirWatcher {
    watcher: Option<RecommendedWatcher>,
    watched: [Option<PathBuf>; 2],
    state: Arc<Mutex<State>>,
}

fn key(p: &Path) -> String {
    p.to_string_lossy().trim_end_matches('\\').to_lowercase()
}

impl DirWatcher {
    pub fn new(ctx: egui::Context) -> DirWatcher {
        let state = Arc::new(Mutex::new(State::default()));
        let st = state.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            let mut s = st.lock().unwrap();
            for p in &ev.paths {
                if let Some(parent) = p.parent() {
                    s.dirty.insert(key(parent));
                }
                // The watched directory itself may be the event path.
                s.dirty.insert(key(p));
            }
            let now = Instant::now();
            s.first.get_or_insert(now);
            s.last = Some(now);
            ctx.request_repaint_after(Duration::from_millis(300));
        })
        .ok();
        DirWatcher { watcher, watched: [None, None], state }
    }

    /// Watches `dir` in `slot` (0 = left, 1 = right); `None` stops watching.
    pub fn watch(&mut self, slot: usize, dir: Option<&Path>) {
        let new = dir.map(Path::to_path_buf);
        if self.watched[slot] == new {
            return;
        }
        let Some(w) = self.watcher.as_mut() else { return };
        if let Some(old) = self.watched[slot].take() {
            if self.watched[1 - slot].as_ref() != Some(&old) {
                let _ = w.unwatch(&old);
            }
        }
        if let Some(d) = &new {
            if self.watched[1 - slot].as_ref() != Some(d) {
                let _ = w.watch(d, RecursiveMode::NonRecursive);
            }
        }
        self.watched[slot] = new;
    }

    /// Returns `true` for each slot whose directory changed and has been
    /// quiet for a moment (or busy for over a second).
    pub fn poll(&self) -> [bool; 2] {
        let mut s = self.state.lock().unwrap();
        let (Some(first), Some(last)) = (s.first, s.last) else { return [false, false] };
        if last.elapsed() < Duration::from_millis(200) && first.elapsed() < Duration::from_secs(1) {
            return [false, false];
        }
        let dirty = std::mem::take(&mut s.dirty);
        s.first = None;
        s.last = None;
        let hit = |slot: usize| self.watched[slot].as_ref().is_some_and(|d| dirty.contains(&key(d)));
        [hit(0), hit(1)]
    }
}
