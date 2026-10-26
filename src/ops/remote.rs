//! Transfers between local folders and FTP/SFTP servers, between servers,
//! and operations on servers (delete, server-side move).

use super::*;
use crate::remote::{self, Connection, JobSession, RemoteEntry};
use std::collections::HashMap;
use std::io::{Seek, SeekFrom};

/// One file or directory of a planned transfer, in depth-first order
/// (a directory comes before its contents).
struct Item {
    /// Source path: remote `/a/b` or a local path.
    src: String,
    /// Target path relative to the destination directory, `/`-separated.
    rel: String,
    is_dir: bool,
    is_link: bool,
    size: u64,
    modified: Option<SystemTime>,
    /// Index of the top-level source this item belongs to.
    top: usize,
}

/// Directories nested deeper than this are not followed (symlink loops).
const MAX_DEPTH: usize = 40;

/// Makes a server-supplied name safe as a single Windows path component.
fn local_name(name: &str) -> String {
    let mut s: String = name.chars().map(|c| if c.is_control() || "<>:\"/\\|?*".contains(c) { '_' } else { c }).collect();
    while s.ends_with(['.', ' ']) {
        s.pop();
    }
    if s.is_empty() {
        return "_".into();
    }
    // Device names (CON, NUL, COM1, ...) can't be file names on Windows.
    let stem = s.split('.').next().unwrap_or("").trim_end().to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4 && (stem.starts_with("COM") || stem.starts_with("LPT")) && stem.as_bytes()[3].is_ascii_digit());
    if device { format!("_{s}") } else { s }
}

/// Joins a `/`-separated relative path below `dir`, component by component.
fn local_target(dir: &Path, rel: &str) -> PathBuf {
    rel.split('/').filter(|c| !c.is_empty()).fold(dir.to_path_buf(), |p, c| p.join(local_name(c)))
}

/// A remote source: connection, path, is_dir, is_link, size, modified.
type RemoteSrc<'a> = (&'a Arc<Connection>, &'a str, bool, bool, u64, Option<SystemTime>);

fn remote_sources(sources: &[Source]) -> Vec<RemoteSrc<'_>> {
    sources
        .iter()
        .filter_map(|s| match s {
            Source::Remote { conn, path, is_dir, is_link, size, modified } => Some((conn, path.as_str(), *is_dir, *is_link, *size, *modified)),
            _ => None,
        })
        .collect()
}

impl Worker {
    fn open_session(&self, conn: &Arc<Connection>) -> R<JobSession> {
        self.update(|p| p.current = format!("Connecting to {} ...", conn.url_prefix()));
        JobSession::open(conn).map_err(|e| format!("{}: {e}", conn.url_prefix()))
    }

    fn add_totals(&self, plan: &[Item]) {
        let files = plan.iter().filter(|i| !i.is_dir).count() as u64;
        let bytes = plan.iter().filter(|i| !i.is_dir).map(|i| i.size).sum::<u64>();
        self.update(|p| {
            p.counting = false;
            p.files_total += files;
            p.bytes_total += bytes;
        });
    }

    /// Walks remote sources. With `follow_links`, links to directories are
    /// entered (downloads); without, a link is an item of its own (deletes
    /// must remove the link, never what it points to).
    fn plan_remote(&mut self, sess: &mut JobSession, sources: &[Source], mask: &str, follow_links: bool) -> R<Vec<Item>> {
        let mut plan = Vec::new();
        for (top, (_, path, is_dir, is_link, size, modified)) in remote_sources(sources).into_iter().enumerate() {
            let name = remote::split(path).map(|(_, n)| n).unwrap_or_default();
            let rel = apply_rename_mask(mask, &name);
            if is_link && !follow_links {
                plan.push(Item { src: path.to_string(), rel, is_dir: false, is_link: true, size, modified, top });
                continue;
            }
            let mut stack = vec![(path.to_string(), rel, is_dir, 0usize)];
            let mut first = true;
            while let Some((src, rel, is_dir, depth)) = stack.pop() {
                if self.cancelled() {
                    return Err(CANCELLED.into());
                }
                self.update(|p| p.current = src.clone());
                if !is_dir {
                    if first {
                        plan.push(Item { src, rel, is_dir: false, is_link: false, size, modified, top });
                    }
                    first = false;
                    continue;
                }
                let listed = if depth < MAX_DEPTH { sess.call(|fs| fs.list(&src)) } else { Ok(Vec::new()) };
                let mut children = match listed {
                    Ok(list) => list,
                    // A link we guessed to be a directory may point to a file.
                    Err(_) if first && !follow_links => Vec::new(),
                    Err(e) => {
                        self.error(format!("{src}: {e}"));
                        first = false;
                        continue;
                    }
                };
                if first {
                    plan.push(Item { src: src.clone(), rel: rel.clone(), is_dir: true, is_link: false, size: 0, modified, top });
                }
                first = false;
                children.retain(|e| e.name != "." && e.name != "..");
                children.sort_by(|a, b| b.name.cmp(&a.name));
                for e in children {
                    let child = remote::join(&src, &e.name);
                    let child_rel = format!("{rel}/{}", e.name);
                    if e.is_dir && (!e.is_link || follow_links) {
                        plan.push(Item { src: child.clone(), rel: child_rel.clone(), is_dir: true, is_link: e.is_link, size: 0, modified: e.modified, top });
                        stack.push((child, child_rel, true, depth + 1));
                    } else {
                        plan.push(Item { src: child, rel: child_rel, is_dir: false, is_link: e.is_link, size: e.size, modified: e.modified, top });
                    }
                }
            }
        }
        // The stack walk pushed directories ahead of their contents but in
        // the wrong order between siblings; a stable sort on the path fixes it.
        plan.sort_by(|a, b| (a.top, a.rel.as_str()).cmp(&(b.top, b.rel.as_str())));
        Ok(plan)
    }

    fn plan_local(&mut self, sources: &[PathBuf], mask: &str) -> R<Vec<Item>> {
        let mut plan = Vec::new();
        for (top, src) in sources.iter().enumerate() {
            let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let mut stack = vec![(src.clone(), apply_rename_mask(mask, &name))];
            while let Some((path, rel)) = stack.pop() {
                if self.cancelled() {
                    return Err(CANCELLED.into());
                }
                let meta = match std::fs::metadata(&path) {
                    Ok(m) => m,
                    Err(e) => {
                        self.error(format!("{}: {e}", path.display()));
                        continue;
                    }
                };
                let src_str = path.to_string_lossy().into_owned();
                if meta.is_dir() {
                    plan.push(Item { src: src_str, rel: rel.clone(), is_dir: true, is_link: false, size: 0, modified: meta.modified().ok(), top });
                    let mut children: Vec<_> = std::fs::read_dir(&path).map(|rd| rd.flatten().collect()).unwrap_or_default();
                    children.sort_by_key(|e| std::cmp::Reverse(e.file_name()));
                    for c in children {
                        stack.push((c.path(), format!("{rel}/{}", c.file_name().to_string_lossy())));
                    }
                } else {
                    plan.push(Item { src: src_str, rel, is_dir: false, is_link: false, size: meta.len(), modified: meta.modified().ok(), top });
                }
            }
        }
        plan.sort_by(|a, b| (a.top, a.rel.as_str()).cmp(&(b.top, b.rel.as_str())));
        Ok(plan)
    }

    /// Progress callback for one file starting at `base` bytes.
    fn file_progress(&self, base: u64) -> impl FnMut(u64) -> bool + '_ {
        move |done| {
            self.update(|p| {
                p.cur_done = done;
                p.bytes_done = base + done;
            });
            !self.cancelled()
        }
    }

    fn start_file(&self, src: &str, target: &str, size: u64) -> u64 {
        let mut base = 0;
        self.update(|p| {
            p.current = src.to_string();
            p.target = target.to_string();
            p.cur_done = 0;
            p.cur_total = size;
            base = p.bytes_done;
        });
        base
    }

    fn finish_file(&self, base: u64, size: u64) {
        self.update(|p| {
            p.files_done += 1;
            p.bytes_done = base + size;
        });
    }

    pub(super) fn run_remote_transfer(&mut self, req: &OpRequest, dest: Dest, is_move: bool) -> R<()> {
        if req.sources.iter().any(|s| matches!(s, Source::Archive { .. })) {
            return Err("Copying between archives and servers is not supported; unpack first.".into());
        }
        let local: Vec<PathBuf> = req.sources.iter().filter_map(|s| if let Source::Fs(p) = s { Some(p.clone()) } else { None }).collect();
        let remote_src = remote_sources(&req.sources);
        match dest {
            Dest::Dir(dir) => self.download(&req.sources, &dir, &req.mask, is_move),
            Dest::Archive { .. } => Err("Files on a server cannot be copied into an archive directly; download them first.".into()),
            Dest::Remote { conn, dir } if remote_src.is_empty() => self.upload(&conn, &local, &dir, &req.mask, is_move),
            Dest::Remote { conn, dir } => {
                if !local.is_empty() {
                    return Err("Select either local files or files on the server, not both.".into());
                }
                if is_move && remote_src.iter().all(|(c, ..)| c.id == conn.id) {
                    self.move_on_server(&conn, &req.sources, &dir, &req.mask)
                } else {
                    self.server_to_server(&req.sources, &conn, &dir, &req.mask, is_move)
                }
            }
        }
    }

    // ---- download ------------------------------------------------------------------

    fn download(&mut self, sources: &[Source], dir: &Path, mask: &str, is_move: bool) -> R<()> {
        let Some((conn, ..)) = remote_sources(sources).first().copied() else { return Ok(()) };
        let conn = conn.clone();
        let mut sess = self.open_session(&conn)?;
        let plan = self.plan_remote(&mut sess, sources, mask, true)?;
        self.add_totals(&plan);
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut failed = HashSet::new();
        for item in &plan {
            if self.cancelled() {
                return Err(CANCELLED.into());
            }
            let target = local_target(dir, &item.rel);
            if item.is_dir {
                if let Err(e) = std::fs::create_dir_all(&target) {
                    self.error(format!("{}: {e}", target.display()));
                    failed.insert(item.top);
                }
                continue;
            }
            if !self.download_file(&mut sess, &conn, item, target)? {
                failed.insert(item.top);
            }
        }
        // Directory times last: writing files into them updates them.
        for item in plan.iter().filter(|i| i.is_dir) {
            if let Some(t) = item.modified {
                let _ = set_dir_mtime(&local_target(dir, &item.rel), t);
            }
        }
        self.finish_tops(&mut sess, sources, &failed, is_move);
        Ok(())
    }

    fn download_file(&mut self, sess: &mut JobSession, conn: &Connection, item: &Item, target: PathBuf) -> R<bool> {
        let mut target = target;
        match self.resolve(&target, item.size, item.modified) {
            Resolution::Write => clear_readonly(&target),
            Resolution::Skip => {
                self.skip_file(item.size);
                return Ok(false);
            }
            Resolution::RenameTo(p) => target = p,
            Resolution::Cancel => return Err(CANCELLED.into()),
        }
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let base = self.start_file(&format!("{}{}", conn.url_prefix(), item.src), &target.to_string_lossy(), item.size);
        let mut progress = self.file_progress(base);
        let result = sess.call(|fs| {
            let mut file = std::fs::File::create(&target)?;
            fs.download(&item.src, &mut file, &mut progress)?;
            if let Some(t) = item.modified {
                let _ = file.set_modified(t);
            }
            Ok(())
        });
        drop(progress);
        match result {
            Ok(()) => {
                self.finish_file(base, item.size);
                Ok(true)
            }
            Err(_) if self.cancelled() => {
                let _ = std::fs::remove_file(&target);
                Err(CANCELLED.into())
            }
            Err(e) => {
                let _ = std::fs::remove_file(&target);
                self.error(format!("{}: {e}", item.src));
                self.skip_file(0);
                Ok(false)
            }
        }
    }

    /// Marks fully transferred top-level sources as done; for moves, deletes
    /// them on the server.
    fn finish_tops(&mut self, sess: &mut JobSession, sources: &[Source], failed: &HashSet<usize>, is_move: bool) {
        for (top, src) in sources.iter().enumerate() {
            if failed.contains(&top) || self.cancelled() {
                continue;
            }
            if is_move && !self.remove_remote_tree(sess, src) {
                continue;
            }
            let name = src.name();
            self.update(|p| p.done_sources.push(name));
        }
    }

    // ---- upload --------------------------------------------------------------------

    /// Entries of a remote directory, listed once per transfer.
    fn listing<'a>(sess: &mut JobSession, cache: &'a mut HashMap<String, HashMap<String, RemoteEntry>>, dir: &str) -> &'a mut HashMap<String, RemoteEntry> {
        cache.entry(dir.to_string()).or_insert_with(|| {
            sess.call(|fs| fs.list(dir)).map(|l| l.into_iter().map(|e| (e.name.clone(), e)).collect()).unwrap_or_default()
        })
    }

    /// Creates `dir` and its missing parents on the server.
    fn ensure_remote_dir(&mut self, sess: &mut JobSession, cache: &mut HashMap<String, HashMap<String, RemoteEntry>>, dir: &str) -> R<()> {
        if dir == "/" || sess.call(|fs| fs.list(dir)).is_ok() {
            return Ok(());
        }
        let mut path = String::new();
        for part in dir.split('/').filter(|p| !p.is_empty()) {
            let parent = if path.is_empty() { "/".to_string() } else { path.clone() };
            path = remote::join(&parent, part);
            if !Self::listing(sess, cache, &parent).contains_key(part) {
                sess.call(|fs| fs.mkdir(&path)).map_err(|e| format!("{path}: {e}"))?;
                cache.insert(path.clone(), HashMap::new());
            }
        }
        Ok(())
    }

    fn upload(&mut self, conn: &Arc<Connection>, sources: &[PathBuf], dir: &str, mask: &str, is_move: bool) -> R<()> {
        let plan = self.plan_local(sources, mask)?;
        self.add_totals(&plan);
        let mut sess = self.open_session(conn)?;
        let mut cache = HashMap::new();
        self.ensure_remote_dir(&mut sess, &mut cache, dir)?;
        let mut failed = HashSet::new();
        for item in &plan {
            if self.cancelled() {
                return Err(CANCELLED.into());
            }
            let target = format!("{}/{}", dir.trim_end_matches('/'), item.rel);
            let (parent, name) = remote::split(&target).unwrap_or(("/".into(), item.rel.clone()));
            let existing = Self::listing(&mut sess, &mut cache, &parent).get(&name).cloned();
            if item.is_dir {
                if existing.as_ref().is_some_and(|e| e.is_dir) {
                    continue;
                }
                match sess.call(|fs| fs.mkdir(&target)) {
                    Ok(()) => {
                        let entry = RemoteEntry { name: name.clone(), is_dir: true, is_link: false, size: 0, modified: None, mode: None };
                        Self::listing(&mut sess, &mut cache, &parent).insert(name, entry);
                        cache.insert(target.clone(), HashMap::new());
                    }
                    Err(e) => {
                        self.error(format!("{target}: {e}"));
                        failed.insert(item.top);
                    }
                }
                continue;
            }
            if !self.upload_file(&mut sess, conn, &mut cache, item, &parent, name, existing)? {
                failed.insert(item.top);
            }
        }
        drop(sess);
        for (top, src) in sources.iter().enumerate() {
            if failed.contains(&top) || self.cancelled() {
                continue;
            }
            if is_move && !self.remove_fs(src)? {
                continue;
            }
            let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            self.update(|p| p.done_sources.push(name));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn upload_file(
        &mut self,
        sess: &mut JobSession,
        conn: &Connection,
        cache: &mut HashMap<String, HashMap<String, RemoteEntry>>,
        item: &Item,
        parent: &str,
        mut name: String,
        existing: Option<RemoteEntry>,
    ) -> R<bool> {
        if let Some(e) = existing {
            let shown = format!("{}{}", conn.url_prefix(), remote::join(parent, &name));
            if e.is_dir {
                self.error(format!("{shown}: a directory with this name exists"));
                self.skip_file(item.size);
                return Ok(false);
            }
            match self.ask(&shown, item.size, item.modified, e.size, e.modified) {
                Choice::Write => {}
                Choice::Skip => {
                    self.skip_file(item.size);
                    return Ok(false);
                }
                Choice::Rename => {
                    let names = Self::listing(sess, cache, parent);
                    name = unique_remote_name(&name, |n| names.contains_key(n));
                }
                Choice::Cancel => return Err(CANCELLED.into()),
            }
        }
        let target = remote::join(parent, &name);
        let base = self.start_file(&item.src, &format!("{}{target}", conn.url_prefix()), item.size);
        let mut file = match std::fs::File::open(&item.src) {
            Ok(f) => f,
            Err(e) => {
                self.error(format!("{}: {e}", item.src));
                self.skip_file(item.size);
                return Ok(false);
            }
        };
        let mut progress = self.file_progress(base);
        let result = sess.call(|fs| {
            file.seek(SeekFrom::Start(0))?;
            fs.upload(&target, &mut file, &mut progress)
        });
        drop(progress);
        match result {
            Ok(()) => {
                if let Some(t) = item.modified {
                    let _ = sess.call(|fs| fs.set_modified(&target, t));
                }
                let entry = RemoteEntry { name: name.clone(), is_dir: false, is_link: false, size: item.size, modified: item.modified, mode: None };
                Self::listing(sess, cache, parent).insert(name, entry);
                self.finish_file(base, item.size);
                Ok(true)
            }
            Err(e) => {
                // Don't leave a truncated file on the server.
                let _ = sess.call(|fs| fs.remove_file(&target));
                if self.cancelled() {
                    return Err(CANCELLED.into());
                }
                self.error(format!("{target}: {e}"));
                self.skip_file(0);
                Ok(false)
            }
        }
    }

    // ---- server side -----------------------------------------------------------------

    /// Move/rename within one server.
    fn move_on_server(&mut self, conn: &Arc<Connection>, sources: &[Source], dir: &str, mask: &str) -> R<()> {
        let mut sess = self.open_session(conn)?;
        let srcs = remote_sources(sources);
        self.update(|p| {
            p.counting = false;
            p.files_total = srcs.len() as u64;
        });
        let mut cache = HashMap::new();
        for (_, path, is_dir, _, size, modified) in srcs {
            if self.cancelled() {
                return Err(CANCELLED.into());
            }
            let name = remote::split(path).map(|(_, n)| n).unwrap_or_default();
            let new_name = apply_rename_mask(mask, &name);
            let target = remote::join(dir, &new_name);
            if target == path {
                self.update(|p| p.files_done += 1);
                continue;
            }
            self.update(|p| {
                p.current = path.to_string();
                p.target = target.clone();
            });
            let existing = Self::listing(&mut sess, &mut cache, dir).get(&new_name).cloned();
            // Renaming only the case of a name is not a conflict.
            let case_only = existing.is_some() && new_name.eq_ignore_ascii_case(&name) && remote::split(path).is_some_and(|(p, _)| p == dir);
            if let (Some(e), false) = (existing, case_only) {
                if e.is_dir || is_dir {
                    self.error(format!("{target}: already exists"));
                    continue;
                }
                match self.ask(&format!("{}{target}", conn.url_prefix()), size, modified, e.size, e.modified) {
                    Choice::Write => {
                        if let Err(err) = sess.call(|fs| fs.remove_file(&target)) {
                            self.error(format!("{target}: {err}"));
                            continue;
                        }
                    }
                    Choice::Skip => {
                        self.update(|p| p.files_done += 1);
                        continue;
                    }
                    Choice::Rename => {
                        let names = Self::listing(&mut sess, &mut cache, dir);
                        let unique = unique_remote_name(&new_name, |n| names.contains_key(n));
                        if let Err(err) = sess.call(|fs| fs.rename(path, &remote::join(dir, &unique))) {
                            self.error(format!("{path}: {err}"));
                        }
                        self.update(|p| p.files_done += 1);
                        continue;
                    }
                    Choice::Cancel => return Err(CANCELLED.into()),
                }
            }
            match sess.call(|fs| fs.rename(path, &target)) {
                Ok(()) => self.update(|p| {
                    p.files_done += 1;
                    p.done_sources.push(name);
                }),
                Err(e) => self.error(format!("{path}: {e}")),
            }
        }
        Ok(())
    }

    /// Copy (or move) between servers, or within one server, through a
    /// temporary local file.
    fn server_to_server(&mut self, sources: &[Source], dest: &Arc<Connection>, dir: &str, mask: &str, is_move: bool) -> R<()> {
        let Some((src_conn, ..)) = remote_sources(sources).first().copied() else { return Ok(()) };
        let src_conn = src_conn.clone();
        let mut src = self.open_session(&src_conn)?;
        let plan = self.plan_remote(&mut src, sources, mask, true)?;
        self.add_totals(&plan);
        let mut dst = self.open_session(dest)?;
        let mut cache = HashMap::new();
        self.ensure_remote_dir(&mut dst, &mut cache, dir)?;
        let tmp_dir = std::env::temp_dir().join("MCommander").join("transfer");
        std::fs::create_dir_all(&tmp_dir).map_err(|e| format!("{}: {e}", tmp_dir.display()))?;
        let tmp = tmp_dir.join(format!("{}-{}", std::process::id(), self.started.elapsed().as_nanos()));
        let mut failed = HashSet::new();
        for item in &plan {
            if self.cancelled() {
                let _ = std::fs::remove_file(&tmp);
                return Err(CANCELLED.into());
            }
            let target = format!("{}/{}", dir.trim_end_matches('/'), item.rel);
            let (parent, name) = remote::split(&target).unwrap_or(("/".into(), item.rel.clone()));
            let existing = Self::listing(&mut dst, &mut cache, &parent).get(&name).cloned();
            if item.is_dir {
                if !existing.as_ref().is_some_and(|e| e.is_dir) {
                    if let Err(e) = dst.call(|fs| fs.mkdir(&target)) {
                        self.error(format!("{target}: {e}"));
                        failed.insert(item.top);
                    }
                    cache.insert(target.clone(), HashMap::new());
                }
                continue;
            }
            // Download to the temporary file, then upload it.
            self.start_file(&format!("{}{}", src_conn.url_prefix(), item.src), &tmp.to_string_lossy(), item.size);
            let fetched = src.call(|fs| {
                let mut f = std::fs::File::create(&tmp)?;
                fs.download(&item.src, &mut f, &mut |_| !self.cancelled())
            });
            if let Err(e) = fetched {
                if self.cancelled() {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(CANCELLED.into());
                }
                self.error(format!("{}: {e}", item.src));
                self.skip_file(item.size);
                failed.insert(item.top);
                continue;
            }
            let staged = Item { src: tmp.to_string_lossy().into_owned(), rel: item.rel.clone(), is_dir: false, is_link: false, size: item.size, modified: item.modified, top: item.top };
            let ok = self.upload_file(&mut dst, dest, &mut cache, &staged, &parent, name, existing);
            let _ = std::fs::remove_file(&tmp);
            if !ok? {
                failed.insert(item.top);
            }
        }
        drop(dst);
        self.finish_tops(&mut src, sources, &failed, is_move);
        Ok(())
    }

    // ---- delete ------------------------------------------------------------------------

    pub(super) fn delete_remote(&mut self, sources: &[Source]) -> R<()> {
        let Some((conn, ..)) = remote_sources(sources).first().copied() else { return Ok(()) };
        let conn = conn.clone();
        let mut sess = self.open_session(&conn)?;
        let plan = self.plan_remote(&mut sess, sources, "*.*", false)?;
        self.update(|p| {
            p.counting = false;
            p.files_total = plan.len() as u64;
        });
        let mut failed = HashSet::new();
        // Contents before their directories.
        for item in plan.iter().rev() {
            if self.cancelled() {
                return Err(CANCELLED.into());
            }
            if !self.remove_remote_item(&mut sess, item) {
                failed.insert(item.top);
            }
        }
        for (top, src) in sources.iter().enumerate() {
            if !failed.contains(&top) {
                let name = src.name();
                self.update(|p| p.done_sources.push(name));
            }
        }
        Ok(())
    }

    fn remove_remote_item(&mut self, sess: &mut JobSession, item: &Item) -> bool {
        self.update(|p| p.current = item.src.clone());
        let res = if item.is_dir && !item.is_link { sess.call(|fs| fs.remove_dir(&item.src)) } else { sess.call(|fs| fs.remove_file(&item.src)) };
        match res {
            Ok(()) => {
                self.update(|p| p.files_done += 1);
                true
            }
            Err(e) => {
                self.error(format!("{}: {e}", item.src));
                false
            }
        }
    }

    /// Deletes one top-level source on the server (after a move).
    fn remove_remote_tree(&mut self, sess: &mut JobSession, src: &Source) -> bool {
        let Ok(plan) = self.plan_remote(sess, std::slice::from_ref(src), "*.*", false) else { return false };
        let mut ok = true;
        for item in plan.iter().rev() {
            let before = self.sh.progress.lock().unwrap().files_done;
            ok &= self.remove_remote_item(sess, item);
            // Deleting is part of the move, not extra files.
            self.update(|p| p.files_done = before);
        }
        ok
    }
}

/// `name(1).ext`, `name(2).ext`, ... not taken yet.
fn unique_remote_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (1..).map(|i| format!("{stem}({i}){ext}")).find(|n| !taken(n)).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::live_tests::{fixture, servers, trust_in};
    use crate::remote::{ConnParams, Protocol, Secret};

    #[test]
    fn server_names_cannot_escape_the_target() {
        let base = Path::new(r"C:\dl");
        assert_eq!(local_target(base, "a/b.txt"), PathBuf::from(r"C:\dl\a\b.txt"));
        assert_eq!(local_target(base, r"..\..\evil.exe"), PathBuf::from(r"C:\dl\.._.._evil.exe"));
        assert_eq!(local_target(base, "x/../y"), PathBuf::from(r"C:\dl\x\_\y"));
        assert_eq!(local_target(base, "c:/Windows"), PathBuf::from(r"C:\dl\c_\Windows"));
        assert_eq!(local_name("con?.txt "), "con_.txt");
        assert_eq!(local_name("nul.txt"), "_nul.txt");
        assert_eq!(local_name("COM1"), "_COM1");
        assert_eq!(local_name("COMPUTER.txt"), "COMPUTER.txt");
        assert_eq!(unique_remote_name("a.txt", |n| n == "a(1).txt"), "a(2).txt");
    }

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
            assert!(t0.elapsed() < std::time::Duration::from_secs(60), "job timed out");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    fn remote_src(conn: &Arc<Connection>, path: &str, is_dir: bool) -> Source {
        Source::Remote { conn: conn.clone(), path: path.into(), is_dir, is_link: false, size: 0, modified: None }
    }

    /// Upload, download, move on the server, copy between servers and
    /// delete, against an SFTP and an FTP server.
    #[test]
    fn transfers_with_servers() {
        let local = tempfile::tempdir().unwrap();
        let tree = local.path().join("tree");
        std::fs::create_dir_all(tree.join("deep").join("er")).unwrap();
        std::fs::create_dir_all(tree.join("empty")).unwrap();
        std::fs::write(tree.join("a.txt"), b"alpha").unwrap();
        std::fs::write(tree.join("deep").join("er").join("b.bin"), vec![7u8; 100_000]).unwrap();

        let sftp_root = fixture();
        let server = servers::sftp(sftp_root.path(), "u", "p");
        let state = tempfile::tempdir().unwrap();
        let trust = trust_in(state.path());
        trust.store.save_host_key("127.0.0.1", server.port, &server.host_key.to_openssh().unwrap()).unwrap();
        let mut p = ConnParams::new(Protocol::Sftp, "127.0.0.1");
        p.port = server.port;
        p.user = "u".into();
        p.password = Some(Secret("p".into()));
        p.key_file = Some(PathBuf::from("does-not-exist"));
        let sftp = Connection::new(p.clone(), "sftp".into(), trust.clone(), remote::connect(&p, &trust).unwrap());

        // Upload a tree.
        let r = run(OpRequest { kind: OpKind::Copy, sources: vec![Source::Fs(tree.clone())], dest: Some(Dest::Remote { conn: sftp.clone(), dir: "/up".into() }), mask: "*.*".into() });
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!((r.files_total, r.files_done, r.bytes_done), (2, 2, 100_005));
        assert_eq!(std::fs::read(sftp_root.path().join("up").join("tree").join("deep").join("er").join("b.bin")).unwrap().len(), 100_000);
        assert!(sftp_root.path().join("up").join("tree").join("empty").is_dir());

        // Uploading again asks (answered "overwrite all" by the test).
        let r = run(OpRequest { kind: OpKind::Copy, sources: vec![Source::Fs(tree.join("a.txt"))], dest: Some(Dest::Remote { conn: sftp.clone(), dir: "/up/tree".into() }), mask: "*.*".into() });
        assert!(r.errors.is_empty(), "{:?}", r.errors);

        // Download it again.
        let down = local.path().join("down");
        let r = run(OpRequest { kind: OpKind::Copy, sources: vec![remote_src(&sftp, "/up/tree", true)], dest: Some(Dest::Dir(down.clone())), mask: "*.*".into() });
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(std::fs::read(down.join("tree").join("a.txt")).unwrap(), b"alpha");
        assert_eq!(std::fs::read(down.join("tree").join("deep").join("er").join("b.bin")).unwrap(), vec![7u8; 100_000]);
        assert!(down.join("tree").join("empty").is_dir());

        // Rename on the server (move within it, with a mask).
        let r = run(OpRequest { kind: OpKind::Move, sources: vec![remote_src(&sftp, "/hello.txt", false)], dest: Some(Dest::Remote { conn: sftp.clone(), dir: "/up".into() }), mask: "*.md".into() });
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert!(sftp_root.path().join("up").join("hello.md").is_file());
        assert!(!sftp_root.path().join("hello.txt").exists());

        // Copy from the SFTP server to an FTP server.
        let ftp_root = tempfile::tempdir().unwrap();
        let port = servers::ftp(ftp_root.path(), None);
        let mut fp = ConnParams::new(Protocol::Ftp, "127.0.0.1");
        fp.port = port;
        let ftp = Connection::new(fp.clone(), "ftp".into(), trust.clone(), remote::connect(&fp, &trust).unwrap());
        let r = run(OpRequest { kind: OpKind::Copy, sources: vec![remote_src(&sftp, "/up/tree", true)], dest: Some(Dest::Remote { conn: ftp.clone(), dir: "/".into() }), mask: "*.*".into() });
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(std::fs::read(ftp_root.path().join("tree").join("a.txt")).unwrap(), b"alpha");

        // Move from FTP to local: downloaded, then deleted on the server.
        let moved = local.path().join("moved");
        let r = run(OpRequest { kind: OpKind::Move, sources: vec![remote_src(&ftp, "/tree", true)], dest: Some(Dest::Dir(moved.clone())), mask: "*.*".into() });
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert!(moved.join("tree").join("deep").join("er").join("b.bin").is_file());
        assert!(!ftp_root.path().join("tree").exists());
        assert_eq!(r.done_sources, vec!["tree".to_string()]);

        // Delete a tree on the SFTP server.
        let r = run(OpRequest { kind: OpKind::Delete { recycle: false }, sources: vec![remote_src(&sftp, "/up", true)], dest: None, mask: String::new() });
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert!(!sftp_root.path().join("up").exists());
        assert!(sftp_root.path().join("sub dir").exists());
    }
}
