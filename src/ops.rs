//! Background file operations with progress, overwrite prompts and cancel.
//!
//! A job runs on its own thread. The UI polls [`Job::progress`] every frame
//! and answers overwrite prompts through [`Job::decide`].

use crate::archive::{self, AddItem, ZipIndex};
use crate::mask::apply_rename_mask;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Instant, SystemTime};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Fs(PathBuf),
    Zip { archive: PathBuf, inner: String, is_dir: bool },
}

impl Source {
    pub fn name(&self) -> String {
        match self {
            Source::Fs(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            Source::Zip { inner, .. } => inner.rsplit('/').next().unwrap_or(inner).to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Dest {
    Dir(PathBuf),
    Archive { archive: PathBuf, inner: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpKind {
    Copy,
    Move,
    Delete { recycle: bool },
    Pack { archive: PathBuf, move_files: bool },
    /// Copy individual files to exact targets, overwriting (directory sync).
    CopyPairs(Vec<(PathBuf, PathBuf)>),
}

#[derive(Clone, Debug)]
pub struct OpRequest {
    pub kind: OpKind,
    pub sources: Vec<Source>,
    pub dest: Option<Dest>,
    /// Target name mask for top-level items (`*.*` keeps names).
    pub mask: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Overwrite,
    OverwriteAll,
    OverwriteOlder,
    Skip,
    SkipAll,
    AutoRename,
    AutoRenameAll,
    Cancel,
}

#[derive(Clone, Debug)]
pub struct Conflict {
    pub target: String,
    pub src_size: u64,
    pub src_time: Option<SystemTime>,
    pub dst_size: u64,
    pub dst_time: Option<SystemTime>,
}

#[derive(Clone, Debug)]
pub struct Progress {
    pub title: String,
    pub counting: bool,
    pub current: String,
    pub target: String,
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub cur_done: u64,
    pub cur_total: u64,
    pub conflict: Option<Conflict>,
    pub errors: Vec<String>,
    pub finished: bool,
    pub cancelled: bool,
    pub started: Instant,
    /// Names of top-level sources that were fully processed (to unselect them).
    pub done_sources: Vec<String>,
}

struct Shared {
    progress: Mutex<Progress>,
    cancel: AtomicBool,
    decision: Mutex<Option<Decision>>,
    cv: Condvar,
    last_repaint: AtomicU64,
}

pub struct Job {
    pub background: bool,
    shared: Arc<Shared>,
}

impl Job {
    pub fn start(id: u64, request: OpRequest, ctx: egui::Context) -> Job {
        let title = match &request.kind {
            OpKind::Copy => "Copying",
            OpKind::Move => "Moving",
            OpKind::Delete { .. } => "Deleting",
            OpKind::Pack { .. } => "Packing",
            OpKind::CopyPairs(_) => "Synchronizing",
        };
        let shared = Arc::new(Shared {
            progress: Mutex::new(Progress {
                title: title.to_string(),
                counting: true,
                current: String::new(),
                target: String::new(),
                files_done: 0,
                files_total: 0,
                bytes_done: 0,
                bytes_total: 0,
                cur_done: 0,
                cur_total: 0,
                conflict: None,
                errors: Vec::new(),
                finished: false,
                cancelled: false,
                started: Instant::now(),
                done_sources: Vec::new(),
            }),
            cancel: AtomicBool::new(false),
            decision: Mutex::new(None),
            cv: Condvar::new(),
            last_repaint: AtomicU64::new(0),
        });
        let worker = Worker {
            sh: shared.clone(),
            ctx,
            overwrite_all: false,
            skip_all: false,
            older_all: false,
            rename_all: false,
            started: Instant::now(),
        };
        let req = request;
        std::thread::Builder::new()
            .name(format!("job-{id}"))
            .spawn(move || worker.run(req))
            .expect("spawn job thread");
        Job { background: false, shared }
    }

    pub fn progress(&self) -> Progress {
        self.shared.progress.lock().unwrap().clone()
    }

    pub fn decide(&self, d: Decision) {
        *self.shared.decision.lock().unwrap() = Some(d);
        self.shared.cv.notify_all();
    }

    pub fn cancel(&self) {
        self.shared.cancel.store(true, Ordering::Relaxed);
        self.decide(Decision::Cancel);
    }
}

#[derive(PartialEq)]
enum Resolution {
    Write,
    Skip,
    RenameTo(PathBuf),
    Cancel,
}

struct Worker {
    sh: Arc<Shared>,
    ctx: egui::Context,
    overwrite_all: bool,
    skip_all: bool,
    older_all: bool,
    rename_all: bool,
    started: Instant,
}

type R<T> = Result<T, String>;

const CANCELLED: &str = "\u{0}cancelled";

impl Worker {
    fn cancelled(&self) -> bool {
        self.sh.cancel.load(Ordering::Relaxed)
    }

    fn update(&self, f: impl FnOnce(&mut Progress)) {
        f(&mut self.sh.progress.lock().unwrap());
        // Throttle repaints to ~30 per second.
        let now = self.started.elapsed().as_millis() as u64;
        let last = self.sh.last_repaint.load(Ordering::Relaxed);
        if now.saturating_sub(last) >= 33 {
            self.sh.last_repaint.store(now, Ordering::Relaxed);
            self.ctx.request_repaint();
        }
    }

    fn error(&self, msg: String) {
        self.update(|p| p.errors.push(msg));
    }

    fn run(mut self, req: OpRequest) {
        let result = match &req.kind {
            OpKind::Copy | OpKind::Move => self.run_transfer(&req, req.kind == OpKind::Move),
            OpKind::Delete { recycle } => self.run_delete(&req.sources, *recycle),
            OpKind::Pack { archive, move_files } => self.run_pack(&req.sources, archive, *move_files),
            OpKind::CopyPairs(pairs) => self.run_pairs(pairs),
        };
        let cancelled = self.cancelled() || matches!(&result, Err(e) if e == CANCELLED);
        if let Err(e) = result {
            if e != CANCELLED {
                self.error(e);
            }
        }
        self.update(|p| {
            p.finished = true;
            p.cancelled = cancelled;
            p.conflict = None;
        });
        self.ctx.request_repaint();
    }

    // ---- counting ------------------------------------------------------

    fn count_fs(&self, paths: &[PathBuf]) -> (u64, u64) {
        let (mut files, mut bytes) = (0u64, 0u64);
        let mut stack: Vec<PathBuf> = paths.to_vec();
        while let Some(p) = stack.pop() {
            if self.cancelled() {
                break;
            }
            let Ok(meta) = std::fs::symlink_metadata(&p) else { continue };
            if meta.is_dir() {
                if let Ok(rd) = std::fs::read_dir(&p) {
                    stack.extend(rd.flatten().map(|e| e.path()));
                }
            } else {
                files += 1;
                bytes += meta.len();
            }
        }
        (files, bytes)
    }

    // ---- conflicts -----------------------------------------------------

    fn resolve(&mut self, target: &Path, src_size: u64, src_time: Option<SystemTime>) -> Resolution {
        let Ok(dst) = std::fs::metadata(target) else { return Resolution::Write };
        if self.overwrite_all {
            return Resolution::Write;
        }
        if self.skip_all {
            return Resolution::Skip;
        }
        if self.older_all {
            let older = match (dst.modified().ok(), src_time) {
                (Some(d), Some(s)) => d < s,
                _ => false,
            };
            return if older { Resolution::Write } else { Resolution::Skip };
        }
        if self.rename_all {
            return Resolution::RenameTo(unique_name(target));
        }
        *self.sh.decision.lock().unwrap() = None;
        self.update(|p| {
            p.conflict = Some(Conflict {
                target: target.to_string_lossy().into_owned(),
                src_size,
                src_time,
                dst_size: dst.len(),
                dst_time: dst.modified().ok(),
            })
        });
        self.ctx.request_repaint();
        let decision = {
            let mut guard = self.sh.decision.lock().unwrap();
            while guard.is_none() {
                guard = self.sh.cv.wait(guard).unwrap();
            }
            guard.take().unwrap()
        };
        self.update(|p| p.conflict = None);
        match decision {
            Decision::Overwrite => Resolution::Write,
            Decision::OverwriteAll => {
                self.overwrite_all = true;
                Resolution::Write
            }
            Decision::OverwriteOlder => {
                self.older_all = true;
                self.resolve(target, src_size, src_time)
            }
            Decision::Skip => Resolution::Skip,
            Decision::SkipAll => {
                self.skip_all = true;
                Resolution::Skip
            }
            Decision::AutoRename => Resolution::RenameTo(unique_name(target)),
            Decision::AutoRenameAll => {
                self.rename_all = true;
                Resolution::RenameTo(unique_name(target))
            }
            Decision::Cancel => Resolution::Cancel,
        }
    }

    // ---- copy / move -----------------------------------------------------

    fn run_transfer(&mut self, req: &OpRequest, is_move: bool) -> R<()> {
        let dest = req.dest.clone().ok_or("No target")?;
        let fs_sources: Vec<PathBuf> =
            req.sources.iter().filter_map(|s| if let Source::Fs(p) = s { Some(p.clone()) } else { None }).collect();
        let zip_sources: Vec<&Source> = req.sources.iter().filter(|s| matches!(s, Source::Zip { .. })).collect();

        // Totals for the progress bars.
        let (mut files, mut bytes) = self.count_fs(&fs_sources);
        let mut zip_index = None;
        if let Some(Source::Zip { archive, .. }) = zip_sources.first() {
            let idx = ZipIndex::open(archive).map_err(|e| format!("{}: {e}", archive.display()))?;
            let paths: Vec<String> =
                zip_sources.iter().filter_map(|s| if let Source::Zip { inner, .. } = s { Some(inner.clone()) } else { None }).collect();
            for it in idx.files_under(&paths) {
                files += 1;
                bytes += it.size;
            }
            zip_index = Some(idx);
        }
        self.update(|p| {
            p.counting = false;
            p.files_total = files;
            p.bytes_total = bytes;
        });

        match dest {
            Dest::Dir(dir) => {
                if !dir.exists() {
                    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
                }
                for src in &fs_sources {
                    if self.cancelled() {
                        return Err(CANCELLED.into());
                    }
                    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let target = dir.join(apply_rename_mask(&req.mask, &name));
                    let ok = if is_move { self.move_item(src, &target)? } else { self.copy_item(src, &target)? };
                    if ok {
                        self.update(|p| p.done_sources.push(name));
                    }
                }
                if let Some(idx) = &zip_index {
                    self.extract(idx, &zip_sources, &dir, &req.mask)?;
                    if is_move && !self.cancelled() {
                        let removed: HashSet<String> = zip_sources
                            .iter()
                            .filter_map(|s| if let Source::Zip { inner, .. } = s { Some(inner.clone()) } else { None })
                            .collect();
                        self.delete_from_zip(&idx.path, &removed)?;
                    }
                }
            }
            Dest::Archive { archive, inner } => {
                if !zip_sources.is_empty() {
                    return Err("Copying between archives is not supported; unpack first.".into());
                }
                self.add_to_zip(&archive, &inner, &fs_sources, &req.mask)?;
                if is_move && !self.cancelled() && self.sh.progress.lock().unwrap().errors.is_empty() {
                    for s in &fs_sources {
                        self.remove_fs(s)?;
                    }
                }
                let names: Vec<String> = fs_sources
                    .iter()
                    .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                    .collect();
                self.update(|p| p.done_sources.extend(names));
            }
        }
        Ok(())
    }

    /// Copies a file or directory tree. Returns false if anything was skipped.
    fn copy_item(&mut self, src: &Path, target: &Path) -> R<bool> {
        let meta = std::fs::symlink_metadata(src).map_err(|e| format!("{}: {e}", src.display()))?;
        if same_path(src, target) {
            return Err(format!("Cannot copy {} onto itself", src.display()));
        }
        if meta.is_dir() {
            if is_inside(target, src) {
                return Err(format!("Cannot copy {} into its own subdirectory", src.display()));
            }
            if let Err(e) = std::fs::create_dir_all(target) {
                self.error(format!("{}: {e}", target.display()));
                return Ok(false);
            }
            let mut all_ok = true;
            let mut children: Vec<_> = match std::fs::read_dir(src) {
                Ok(rd) => rd.flatten().collect(),
                Err(e) => {
                    self.error(format!("{}: {e}", src.display()));
                    return Ok(false);
                }
            };
            children.sort_by_key(|e| e.file_name());
            for c in children {
                if self.cancelled() {
                    return Err(CANCELLED.into());
                }
                all_ok &= self.copy_item(&c.path(), &target.join(c.file_name()))?;
            }
            if let Ok(t) = meta.modified() {
                let _ = set_dir_mtime(target, t);
            }
            return Ok(all_ok);
        }
        let mut target = target.to_path_buf();
        match self.resolve(&target, meta.len(), meta.modified().ok()) {
            Resolution::Write => {}
            Resolution::Skip => {
                self.skip_file(meta.len());
                return Ok(false);
            }
            Resolution::RenameTo(p) => target = p,
            Resolution::Cancel => return Err(CANCELLED.into()),
        }
        self.copy_file(src, &target, meta.len())
    }

    fn skip_file(&self, size: u64) {
        self.update(|p| {
            p.files_done += 1;
            p.bytes_done += size;
        });
    }

    fn copy_file(&mut self, src: &Path, target: &Path, size: u64) -> R<bool> {
        let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let base = self.sh.progress.lock().unwrap().bytes_done;
        self.update(|p| {
            p.current = src.to_string_lossy().into_owned();
            p.target = target.to_string_lossy().into_owned();
            p.cur_done = 0;
            p.cur_total = size;
        });
        // A read-only target would make CopyFileEx fail with access denied.
        clear_readonly(target);
        match copy_file_with_progress(src, target, &self.sh.cancel, |done| {
            self.update(|p| {
                p.cur_done = done;
                p.bytes_done = base + done;
            });
        }) {
            Ok(()) => {
                self.update(|p| {
                    p.files_done += 1;
                    p.bytes_done = base + size;
                });
                Ok(true)
            }
            Err(_) if self.cancelled() => {
                let _ = std::fs::remove_file(target);
                Err(CANCELLED.into())
            }
            Err(e) => {
                self.error(format!("{name}: {e}"));
                self.skip_file(0);
                Ok(false)
            }
        }
    }

    fn move_item(&mut self, src: &Path, target: &Path) -> R<bool> {
        let meta = std::fs::symlink_metadata(src).map_err(|e| format!("{}: {e}", src.display()))?;
        if same_path(src, target) {
            return Ok(true);
        }
        if meta.is_dir() && is_inside(target, src) {
            return Err(format!("Cannot move {} into its own subdirectory", src.display()));
        }
        let case_rename = src.to_string_lossy().eq_ignore_ascii_case(&target.to_string_lossy());
        let mut target = target.to_path_buf();
        if target.exists() && !case_rename {
            if meta.is_dir() && target.is_dir() {
                // Merge into the existing directory.
                let mut all_ok = true;
                let children: Vec<_> = std::fs::read_dir(src).map_err(|e| e.to_string())?.flatten().collect();
                for c in children {
                    if self.cancelled() {
                        return Err(CANCELLED.into());
                    }
                    all_ok &= self.move_item(&c.path(), &target.join(c.file_name()))?;
                }
                if all_ok {
                    let _ = std::fs::remove_dir(src);
                }
                return Ok(all_ok);
            }
            match self.resolve(&target, meta.len(), meta.modified().ok()) {
                Resolution::Write => {
                    clear_readonly(&target);
                    if target.is_dir() {
                        self.error(format!("{}: a directory with this name exists", target.display()));
                        return Ok(false);
                    }
                }
                Resolution::Skip => {
                    self.skip_file(meta.len());
                    return Ok(false);
                }
                Resolution::RenameTo(p) => target = p,
                Resolution::Cancel => return Err(CANCELLED.into()),
            }
        }
        self.update(|p| {
            p.current = src.to_string_lossy().into_owned();
            p.target = target.to_string_lossy().into_owned();
        });
        match std::fs::rename(src, &target) {
            Ok(()) => {
                let (files, bytes) = if meta.is_dir() { self.count_fs(&[target.clone()]) } else { (1, meta.len()) };
                self.update(|p| {
                    p.files_done += files;
                    p.bytes_done += bytes;
                });
                Ok(true)
            }
            // ERROR_NOT_SAME_DEVICE: copy, then delete the source.
            Err(e) if e.raw_os_error() == Some(17) => {
                let ok = self.copy_item(src, &target)?;
                if ok {
                    self.remove_fs(src)?;
                }
                Ok(ok)
            }
            Err(e) => {
                self.error(format!("{}: {e}", src.display()));
                Ok(false)
            }
        }
    }

    // ---- archives --------------------------------------------------------

    fn extract(&mut self, idx: &ZipIndex, sources: &[&Source], dest: &Path, mask: &str) -> R<()> {
        let mut zip = archive::open_archive(&idx.path).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 256 * 1024];
        for src in sources {
            let Source::Zip { inner, is_dir, .. } = src else { continue };
            let base = match inner.rfind('/') {
                Some(i) => &inner[..i + 1],
                None => "",
            };
            let mut items: Vec<&archive::ZipItem> = idx
                .items
                .iter()
                .filter(|it| it.path == *inner || it.path.starts_with(&format!("{inner}/")))
                .collect();
            items.sort_by(|a, b| a.path.cmp(&b.path));
            if *is_dir {
                if let Some(t) = archive::safe_join(dest, &inner[base.len()..]) {
                    let _ = std::fs::create_dir_all(t);
                }
            }
            for it in items {
                if self.cancelled() {
                    return Err(CANCELLED.into());
                }
                let mut rel = it.path[base.len()..].to_string();
                if !is_dir {
                    rel = apply_rename_mask(mask, &rel);
                }
                let Some(mut target) = archive::safe_join(dest, &rel) else {
                    self.error(format!("Unsafe path in archive skipped: {}", it.path));
                    continue;
                };
                if it.is_dir {
                    let _ = std::fs::create_dir_all(&target);
                    continue;
                }
                match self.resolve(&target, it.size, it.modified) {
                    Resolution::Write => clear_readonly(&target),
                    Resolution::Skip => {
                        self.skip_file(it.size);
                        continue;
                    }
                    Resolution::RenameTo(p) => target = p,
                    Resolution::Cancel => return Err(CANCELLED.into()),
                }
                if let Some(parent) = target.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                self.update(|p| {
                    p.current = it.path.clone();
                    p.target = target.to_string_lossy().into_owned();
                    p.cur_done = 0;
                    p.cur_total = it.size;
                });
                let res = (|| -> std::io::Result<()> {
                    let mut entry = zip.by_index(it.index).map_err(std::io::Error::other)?;
                    let mut out = std::fs::File::create(&target)?;
                    let mut done = 0u64;
                    loop {
                        let n = entry.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        out.write_all(&buf[..n])?;
                        done += n as u64;
                        self.update(|p| {
                            p.cur_done = done;
                            p.bytes_done += n as u64;
                        });
                        if self.cancelled() {
                            return Err(std::io::Error::new(std::io::ErrorKind::Interrupted, "cancelled"));
                        }
                    }
                    if let Some(t) = it.modified {
                        let _ = out.set_modified(t);
                    }
                    Ok(())
                })();
                match res {
                    Ok(()) => self.update(|p| p.files_done += 1),
                    Err(_) if self.cancelled() => {
                        let _ = std::fs::remove_file(&target);
                        return Err(CANCELLED.into());
                    }
                    Err(e) => self.error(format!("{}: {e}", it.path)),
                }
            }
            let name = src.name();
            self.update(|p| p.done_sources.push(name));
        }
        Ok(())
    }

    fn add_to_zip(&mut self, archive_path: &Path, inner: &str, sources: &[PathBuf], mask: &str) -> R<()> {
        let mut add = archive::expand_sources(sources, inner);
        if mask != "*.*" && !mask.is_empty() {
            // The mask renames top-level files only.
            for item in add.iter_mut().filter(|a| a.source.is_file() && sources.contains(&a.source)) {
                let name = item.source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let renamed = apply_rename_mask(mask, &name);
                item.name = if inner.is_empty() { renamed } else { format!("{inner}/{renamed}") };
            }
        }
        if archive_path.exists() {
            let idx = ZipIndex::open(archive_path).map_err(|e| e.to_string())?;
            let existing: HashSet<&str> = idx.items.iter().filter(|i| !i.is_dir).map(|i| i.path.as_str()).collect();
            let mut keep = Vec::new();
            for item in add {
                if existing.contains(item.name.as_str()) {
                    let meta = std::fs::metadata(&item.source).ok();
                    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                    // Show the conflict against the archive member.
                    let pseudo = archive_path.join(item.name.replace('/', "\\"));
                    match self.resolve_archive(&pseudo, size, meta.and_then(|m| m.modified().ok())) {
                        Resolution::Write => keep.push(item),
                        Resolution::Skip => self.skip_file(size),
                        Resolution::RenameTo(_) => keep.push(AddItem { name: auto_rename_inner(&item.name, &existing), ..item }),
                        Resolution::Cancel => return Err(CANCELLED.into()),
                    }
                } else {
                    keep.push(item);
                }
            }
            add = keep;
        }
        let total: u64 = add.iter().filter_map(|a| std::fs::metadata(&a.source).ok()).filter(|m| m.is_file()).map(|m| m.len()).sum();
        self.update(|p| {
            p.bytes_total = total;
            p.target = archive_path.to_string_lossy().into_owned();
        });
        let cancel = &self.sh.cancel;
        let sh = self.sh.clone();
        let ctx = self.ctx.clone();
        let mut last = String::new();
        archive::rebuild(
            archive_path,
            &|_| true,
            &add,
            &mut |name, n| {
                let mut p = sh.progress.lock().unwrap();
                if name != last {
                    last = name.to_string();
                    p.current = name.to_string();
                    p.files_done += 1;
                }
                p.bytes_done += n;
                drop(p);
                ctx.request_repaint();
                true
            },
            cancel,
        )
        .map_err(|e| if self.cancelled() { CANCELLED.to_string() } else { format!("{}: {e}", archive_path.display()) })
    }

    /// Like [`Worker::resolve`] but for a member that only exists inside an archive.
    fn resolve_archive(&mut self, pseudo: &Path, size: u64, time: Option<SystemTime>) -> Resolution {
        if self.overwrite_all {
            return Resolution::Write;
        }
        if self.skip_all {
            return Resolution::Skip;
        }
        if self.rename_all {
            return Resolution::RenameTo(pseudo.to_path_buf());
        }
        *self.sh.decision.lock().unwrap() = None;
        self.update(|p| {
            p.conflict = Some(Conflict {
                target: pseudo.to_string_lossy().into_owned(),
                src_size: size,
                src_time: time,
                dst_size: 0,
                dst_time: None,
            })
        });
        let decision = {
            let mut guard = self.sh.decision.lock().unwrap();
            while guard.is_none() {
                guard = self.sh.cv.wait(guard).unwrap();
            }
            guard.take().unwrap()
        };
        self.update(|p| p.conflict = None);
        match decision {
            Decision::Overwrite | Decision::OverwriteOlder => Resolution::Write,
            Decision::OverwriteAll => {
                self.overwrite_all = true;
                Resolution::Write
            }
            Decision::Skip => Resolution::Skip,
            Decision::SkipAll => {
                self.skip_all = true;
                Resolution::Skip
            }
            Decision::AutoRename => Resolution::RenameTo(pseudo.to_path_buf()),
            Decision::AutoRenameAll => {
                self.rename_all = true;
                Resolution::RenameTo(pseudo.to_path_buf())
            }
            Decision::Cancel => Resolution::Cancel,
        }
    }

    fn delete_from_zip(&mut self, archive_path: &Path, removed: &HashSet<String>) -> R<()> {
        self.update(|p| p.current = archive_path.to_string_lossy().into_owned());
        let keep = |name: &str| !removed.iter().any(|r| name == r || name.starts_with(&format!("{r}/")));
        archive::rebuild(archive_path, &keep, &[], &mut |_, _| true, &self.sh.cancel)
            .map_err(|e| format!("{}: {e}", archive_path.display()))
    }

    fn run_pack(&mut self, sources: &[Source], archive_path: &Path, move_files: bool) -> R<()> {
        let fs: Vec<PathBuf> = sources.iter().filter_map(|s| if let Source::Fs(p) = s { Some(p.clone()) } else { None }).collect();
        let (files, bytes) = self.count_fs(&fs);
        self.update(|p| {
            p.counting = false;
            p.files_total = files;
            p.bytes_total = bytes;
        });
        self.add_to_zip(archive_path, "", &fs, "*.*")?;
        if move_files && !self.cancelled() && self.sh.progress.lock().unwrap().errors.is_empty() {
            for s in &fs {
                self.remove_fs(s)?;
            }
        }
        let names: Vec<String> = sources.iter().map(Source::name).collect();
        self.update(|p| p.done_sources.extend(names));
        Ok(())
    }

    fn run_pairs(&mut self, pairs: &[(PathBuf, PathBuf)]) -> R<()> {
        let bytes: u64 = pairs.iter().filter_map(|(s, _)| std::fs::metadata(s).ok()).map(|m| m.len()).sum();
        self.update(|p| {
            p.counting = false;
            p.files_total = pairs.len() as u64;
            p.bytes_total = bytes;
        });
        for (src, dst) in pairs {
            if self.cancelled() {
                return Err(CANCELLED.into());
            }
            if let Some(parent) = dst.parent() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    self.error(format!("{}: {e}", parent.display()));
                    continue;
                }
            }
            let size = std::fs::metadata(src).map(|m| m.len()).unwrap_or(0);
            self.copy_file(src, dst, size)?;
        }
        Ok(())
    }

    // ---- delete ------------------------------------------------------------

    fn run_delete(&mut self, sources: &[Source], recycle: bool) -> R<()> {
        let fs: Vec<PathBuf> = sources.iter().filter_map(|s| if let Source::Fs(p) = s { Some(p.clone()) } else { None }).collect();
        let zip: HashSet<String> =
            sources.iter().filter_map(|s| if let Source::Zip { inner, .. } = s { Some(inner.clone()) } else { None }).collect();
        if !zip.is_empty() {
            let Some(Source::Zip { archive, .. }) = sources.iter().find(|s| matches!(s, Source::Zip { .. })) else {
                return Ok(());
            };
            self.update(|p| {
                p.counting = false;
                p.files_total = zip.len() as u64;
            });
            self.delete_from_zip(archive, &zip)?;
            let names: Vec<String> = sources.iter().map(Source::name).collect();
            self.update(|p| {
                p.files_done = p.files_total;
                p.done_sources.extend(names);
            });
            return Ok(());
        }
        if recycle {
            self.update(|p| {
                p.counting = false;
                p.files_total = fs.len() as u64;
            });
            for src in &fs {
                if self.cancelled() {
                    return Err(CANCELLED.into());
                }
                let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                self.update(|p| p.current = src.to_string_lossy().into_owned());
                match crate::win::recycle(std::slice::from_ref(src)) {
                    Ok(()) => self.update(|p| {
                        p.files_done += 1;
                        p.done_sources.push(name);
                    }),
                    Err(e) => self.error(format!("{}: {e}", src.display())),
                }
            }
            return Ok(());
        }
        let (files, _) = self.count_fs(&fs);
        self.update(|p| {
            p.counting = false;
            p.files_total = files;
        });
        for src in &fs {
            if self.cancelled() {
                return Err(CANCELLED.into());
            }
            let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if self.remove_fs(src)? {
                self.update(|p| p.done_sources.push(name));
            }
        }
        Ok(())
    }

    /// Permanently deletes a file or tree, clearing read-only attributes.
    fn remove_fs(&mut self, path: &Path) -> R<bool> {
        let Ok(meta) = std::fs::symlink_metadata(path) else { return Ok(true) };
        self.update(|p| p.current = path.to_string_lossy().into_owned());
        if meta.is_dir() && !meta.file_type().is_symlink() {
            let mut ok = true;
            if let Ok(rd) = std::fs::read_dir(path) {
                for c in rd.flatten() {
                    if self.cancelled() {
                        return Err(CANCELLED.into());
                    }
                    ok &= self.remove_fs(&c.path())?;
                }
            }
            clear_readonly(path);
            if let Err(e) = std::fs::remove_dir(path) {
                self.error(format!("{}: {e}", path.display()));
                return Ok(false);
            }
            return Ok(ok);
        }
        clear_readonly(path);
        let res = if meta.is_dir() { std::fs::remove_dir(path) } else { std::fs::remove_file(path) };
        match res {
            Ok(()) => {
                self.update(|p| p.files_done += 1);
                Ok(true)
            }
            Err(e) => {
                self.error(format!("{}: {e}", path.display()));
                Ok(false)
            }
        }
    }
}

// ---- helpers -----------------------------------------------------------------

fn same_path(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

/// True when `inner` is `outer` or below it.
fn is_inside(inner: &Path, outer: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().to_lowercase().trim_end_matches('\\').to_string();
    let (i, o) = (norm(inner), norm(outer));
    i == o || i.starts_with(&format!("{o}\\"))
}

fn clear_readonly(p: &Path) {
    if let Ok(meta) = std::fs::metadata(p) {
        let mut perm = meta.permissions();
        if perm.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            let _ = std::fs::set_permissions(p, perm);
        }
    }
}

fn set_dir_mtime(dir: &Path, t: SystemTime) -> std::io::Result<()> {
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let f = std::fs::OpenOptions::new().write(true).custom_flags(FILE_FLAG_BACKUP_SEMANTICS).open(dir)?;
    f.set_modified(t)
}

/// `name.ext` -> `name(1).ext`, `name(2).ext`, ... (TC's auto-rename scheme).
pub fn unique_name(target: &Path) -> PathBuf {
    let parent = target.parent().unwrap_or(Path::new(""));
    let name = target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name.as_str(), ""),
    };
    (1..)
        .map(|i| parent.join(format!("{stem}({i}){ext}")))
        .find(|p| !p.exists())
        .unwrap()
}

fn auto_rename_inner(name: &str, existing: &HashSet<&str>) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 && !name[i..].contains('/') => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..).map(|i| format!("{stem}({i}){ext}")).find(|n| !existing.contains(n.as_str())).unwrap()
}

/// CopyFileExW with a progress callback and cancellation.
fn copy_file_with_progress(src: &Path, dst: &Path, cancel: &AtomicBool, mut progress: impl FnMut(u64)) -> std::io::Result<()> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Storage::FileSystem::{
        COPY_FILE_ALLOW_DECRYPTED_DESTINATION, COPYPROGRESSROUTINE_PROGRESS, CopyFileExW, LPPROGRESS_ROUTINE_CALLBACK_REASON,
        PROGRESS_CANCEL, PROGRESS_CONTINUE,
    };
    struct Ctx<'a> {
        progress: &'a mut dyn FnMut(u64),
        cancel: &'a AtomicBool,
    }
    unsafe extern "system" fn cb(
        _total: i64,
        transferred: i64,
        _stream_size: i64,
        _stream_transferred: i64,
        _stream: u32,
        _reason: LPPROGRESS_ROUTINE_CALLBACK_REASON,
        _src: HANDLE,
        _dst: HANDLE,
        data: *const core::ffi::c_void,
    ) -> COPYPROGRESSROUTINE_PROGRESS {
        let ctx = unsafe { &mut *(data as *mut Ctx) };
        (ctx.progress)(transferred as u64);
        if ctx.cancel.load(Ordering::Relaxed) { PROGRESS_CANCEL } else { PROGRESS_CONTINUE }
    }
    let s = crate::win::wide_path(src);
    let d = crate::win::wide_path(dst);
    let mut ctx = Ctx { progress: &mut progress, cancel };
    unsafe {
        CopyFileExW(
            windows::core::PCWSTR(s.as_ptr()),
            windows::core::PCWSTR(d.as_ptr()),
            Some(cb),
            Some(&mut ctx as *mut Ctx as *const core::ffi::c_void),
            None,
            COPY_FILE_ALLOW_DECRYPTED_DESTINATION,
        )
    }
    .map_err(|e| std::io::Error::from_raw_os_error((e.code().0 & 0xFFFF) as i32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn run(req: OpRequest) -> Progress {
        let job = Job::start(1, req, egui::Context::default());
        let t0 = Instant::now();
        loop {
            let p = job.progress();
            if p.conflict.is_some() {
                job.decide(Decision::OverwriteAll);
            }
            if p.finished {
                return p;
            }
            assert!(t0.elapsed() < Duration::from_secs(20), "job timed out");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn copy_move_delete_tree() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), b"hello").unwrap();
        std::fs::write(src.join("sub").join("b.txt"), b"world!").unwrap();
        let dst = dir.path().join("dst");

        let p = run(OpRequest {
            kind: OpKind::Copy,
            sources: vec![Source::Fs(src.clone())],
            dest: Some(Dest::Dir(dst.clone())),
            mask: "*.*".into(),
        });
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.files_total, 2);
        assert_eq!(p.bytes_done, 11);
        assert_eq!(std::fs::read(dst.join("src").join("sub").join("b.txt")).unwrap(), b"world!");

        // Copy again: conflicts are answered with "overwrite all".
        let p = run(OpRequest {
            kind: OpKind::Copy,
            sources: vec![Source::Fs(src.join("a.txt"))],
            dest: Some(Dest::Dir(dst.join("src"))),
            mask: "*.bak".into(),
        });
        assert!(p.errors.is_empty());
        assert!(dst.join("src").join("a.bak").exists());

        let moved = dir.path().join("moved");
        let p = run(OpRequest {
            kind: OpKind::Move,
            sources: vec![Source::Fs(dst.join("src"))],
            dest: Some(Dest::Dir(moved.clone())),
            mask: "*.*".into(),
        });
        assert!(p.errors.is_empty());
        assert!(!dst.join("src").exists());
        assert!(moved.join("src").join("a.bak").exists());

        let p = run(OpRequest {
            kind: OpKind::Delete { recycle: false },
            sources: vec![Source::Fs(moved.join("src"))],
            dest: None,
            mask: String::new(),
        });
        assert!(p.errors.is_empty());
        assert!(!moved.join("src").exists());
    }

    #[test]
    fn copy_into_itself_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("x");
        std::fs::create_dir_all(&src).unwrap();
        let p = run(OpRequest {
            kind: OpKind::Copy,
            sources: vec![Source::Fs(src.clone())],
            dest: Some(Dest::Dir(src.clone())),
            mask: "*.*".into(),
        });
        assert!(!p.errors.is_empty());
    }

    #[test]
    fn pack_then_extract_and_delete_inside_zip() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("data");
        std::fs::create_dir_all(src.join("inner")).unwrap();
        std::fs::write(src.join("one.txt"), b"1").unwrap();
        std::fs::write(src.join("inner").join("two.txt"), b"22").unwrap();
        let zip = dir.path().join("pack.zip");
        let p = run(OpRequest {
            kind: OpKind::Pack { archive: zip.clone(), move_files: false },
            sources: vec![Source::Fs(src.clone())],
            dest: None,
            mask: "*.*".into(),
        });
        assert!(p.errors.is_empty(), "{:?}", p.errors);

        let out = dir.path().join("out");
        let p = run(OpRequest {
            kind: OpKind::Copy,
            sources: vec![Source::Zip { archive: zip.clone(), inner: "data/inner".into(), is_dir: true }],
            dest: Some(Dest::Dir(out.clone())),
            mask: "*.*".into(),
        });
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(std::fs::read(out.join("inner").join("two.txt")).unwrap(), b"22");

        let p = run(OpRequest {
            kind: OpKind::Delete { recycle: false },
            sources: vec![Source::Zip { archive: zip.clone(), inner: "data/one.txt".into(), is_dir: false }],
            dest: None,
            mask: String::new(),
        });
        assert!(p.errors.is_empty());
        let idx = ZipIndex::open(&zip).unwrap();
        assert!(idx.items.iter().all(|i| i.path != "data/one.txt"));
        assert!(idx.items.iter().any(|i| i.path == "data/inner/two.txt"));

        // Copy a file into the archive, under a subfolder.
        std::fs::write(dir.path().join("new.md"), b"n").unwrap();
        let p = run(OpRequest {
            kind: OpKind::Copy,
            sources: vec![Source::Fs(dir.path().join("new.md"))],
            dest: Some(Dest::Archive { archive: zip.clone(), inner: "data".into() }),
            mask: "*.*".into(),
        });
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        let idx = ZipIndex::open(&zip).unwrap();
        assert!(idx.items.iter().any(|i| i.path == "data/new.md"));
    }

    #[test]
    fn unique_names() {
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path().join("a.txt");
        std::fs::write(&t, b"").unwrap();
        assert_eq!(unique_name(&t), dir.path().join("a(1).txt"));
        std::fs::write(dir.path().join("a(1).txt"), b"").unwrap();
        assert_eq!(unique_name(&t), dir.path().join("a(2).txt"));
    }
}
