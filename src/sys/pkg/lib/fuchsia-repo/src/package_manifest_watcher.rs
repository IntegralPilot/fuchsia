// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::{Context as _, Result, anyhow, format_err};
use camino::{Utf8Path, Utf8PathBuf};
use fuchsia_hash::Hash;
use fuchsia_pkg::{PackageManifest, PackageManifestList};
use futures::channel::mpsc::{Receiver, channel};
use futures::executor::block_on;
use futures::{SinkExt, Stream};
use log::error;
use notify::{RecommendedWatcher, RecursiveMode};
use notify_batch_watcher::{BatchEvent, BatchWatcher, Error as BatchError};
use std::collections::{HashMap, HashSet, hash_map};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

type BatchResult = Result<BatchEvent, Vec<BatchError>>;

/// How long to buffer up all watcher events.
const DEFAULT_BATCH_WATCHER_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestEvent {
    /// When the first package manifest event occurred.
    pub start_time: Instant,

    /// When the last package manifest event occurred.
    pub end_time: Instant,

    /// Which package manifests were changed.
    pub changed_manifests: HashSet<Utf8PathBuf>,

    /// Paths we are no longer watching.
    pub unwatched_manifests: HashSet<Utf8PathBuf>,
}

#[derive(Default, Debug)]
pub struct PackageManifestWatcherBuilder {
    manifest_paths: HashSet<Utf8PathBuf>,
    list_paths: HashSet<Utf8PathBuf>,
    batch_timeout: Duration,
}

impl PackageManifestWatcherBuilder {
    pub fn new() -> Self {
        Self {
            manifest_paths: HashSet::new(),
            list_paths: HashSet::new(),
            batch_timeout: DEFAULT_BATCH_WATCHER_TIMEOUT,
        }
    }

    /// Watch a package manifest. The path is not required to exist, but its parent directory must
    /// exist.
    pub fn package_manifest(mut self, path: Utf8PathBuf) -> Self {
        self.manifest_paths.insert(path);
        self
    }

    /// Watch an iterator of package manifests. Each path is not required to exist, but its parent
    /// directory must exist.
    pub fn package_manifests(mut self, paths: impl IntoIterator<Item = Utf8PathBuf>) -> Self {
        for path in paths {
            self = self.package_manifest(path);
        }
        self
    }

    /// Watch a package manifest list. The path is not required to exist, but its parent directory
    /// must exist.
    pub fn package_list(mut self, path: Utf8PathBuf) -> Self {
        self.list_paths.insert(path);
        self
    }

    /// Watch an iterator of package manifest lists. Each path is not required to exist, but its
    /// parent directory must exist.
    pub fn package_lists(mut self, paths: impl IntoIterator<Item = Utf8PathBuf>) -> Self {
        for path in paths {
            self = self.package_list(path);
        }
        self
    }

    /// How long to batch up events before we will emit file changes. A larger value will avoid
    /// excess events if a file keeps getting modified, but will take longer before the caller
    /// observes changes.
    pub fn batch_timeout(mut self, batch_timeout: Duration) -> Self {
        self.batch_timeout = batch_timeout;
        self
    }

    /// Watch paths to package manifests and package manifests lists.
    pub fn watch(self) -> Result<PackageManifestWatcher> {
        let mut watcher = PackageManifestWatcher::new(self.batch_timeout)?;

        for path in self.manifest_paths {
            watcher.watch_package_manifest(&path)?;
        }

        for path in self.list_paths {
            watcher.watch_package_manifest_list(&path)?;
        }

        Ok(watcher)
    }
}

/// Watches changes to manifests and manifest list files.
#[derive(Debug)]
#[pin_project::pin_project]
pub struct PackageManifestWatcher {
    /// The individual package manifests we are watching.
    watched_manifest_paths: HashSet<Utf8PathBuf>,

    /// Mapping of the watched package manifest list path to the manifest paths in the list.
    watched_list_to_manifests: HashMap<Utf8PathBuf, HashSet<Utf8PathBuf>>,

    /// If we get a rescan event, we'll re-load all our manifest data. However, we might still have
    /// old messages in the queue. Ignore any events that are older than this time.
    last_event_time: Instant,

    /// File watcher.
    file_watcher: ManifestFileWatcher,

    #[pin]
    receiver: Receiver<BatchResult>,
}

impl PackageManifestWatcher {
    /// `PackageManifestWatcherBuilder` provides interface to set up the `PackageManifestWatcher`.
    pub fn builder() -> PackageManifestWatcherBuilder {
        PackageManifestWatcherBuilder::new()
    }

    fn new(batch_timeout: Duration) -> Result<Self> {
        let (file_watcher, receiver) = ManifestFileWatcher::new(batch_timeout)?;

        Ok(Self {
            watched_manifest_paths: HashSet::new(),
            watched_list_to_manifests: HashMap::new(),
            last_event_time: Instant::now(),
            file_watcher,
            receiver,
        })
    }

    /// Watch a package manifest path.
    pub fn watch_package_manifest(&mut self, manifest_path: &Utf8Path) -> Result<()> {
        // Canonicalize the paths, since that's what notify returns.
        let manifest_path = manifest_path.canonicalize_utf8()?;

        #[allow(clippy::set_contains_or_insert)] // TODO(https://fxbug.dev/379733655): Remove this
        if !self.watched_manifest_paths.contains(&manifest_path) {
            self.file_watcher.update_package_manifest(&manifest_path)?;
            self.file_watcher.watch_path(&manifest_path)?;

            self.watched_manifest_paths.insert(manifest_path);
        }

        Ok(())
    }

    /// Watch a package manifest list path, and each package manifest inside the file.
    pub fn watch_package_manifest_list(&mut self, list_path: &Utf8Path) -> Result<()> {
        // Canonicalize the paths, since that's what notify returns.
        let list_path = list_path.canonicalize_utf8()?;

        match self.watched_list_to_manifests.entry(list_path) {
            hash_map::Entry::Occupied(_) => Ok(()),
            hash_map::Entry::Vacant(entry) => {
                self.file_watcher.watch_path(entry.key())?;

                let list_contents = read_package_manifest_list(entry.key())?;
                let list_contents = entry.insert(list_contents);

                // Also watch all the manifest paths in the list.
                for manifest_path in list_contents.iter() {
                    self.file_watcher.update_package_manifest(manifest_path)?;
                    self.file_watcher.watch_path(manifest_path)?;
                }

                Ok(())
            }
        }
    }

    /// Rescan all the watched files to see if any have changed.
    fn rescan(&mut self) -> HashSet<Utf8PathBuf> {
        let mut changed_manifest_paths = HashSet::new();

        for manifest_path in &self.watched_manifest_paths {
            if let Ok(true) = self.file_watcher.update_package_manifest(manifest_path) {
                changed_manifest_paths.insert(manifest_path.to_owned());
            }
        }

        for (list_path, old_manifest_paths) in &mut self.watched_list_to_manifests {
            let new_manifest_paths = read_package_manifest_list(list_path).unwrap_or_default();

            for manifest_path in &new_manifest_paths {
                if changed_manifest_paths.contains(manifest_path) {
                    continue;
                }

                if let Ok(true) = self.file_watcher.update_package_manifest(manifest_path) {
                    changed_manifest_paths.insert(manifest_path.to_owned());
                }
            }

            *old_manifest_paths = new_manifest_paths;
        }

        changed_manifest_paths
    }

    fn handle_batch_event(&mut self, event: BatchEvent) -> Option<ManifestEvent> {
        // Ignore old events.
        //
        // The BatchWatcher guarantees that the end time of the previous event (tracked in
        // `last_event_time`) will always be less than or equal to the `end_time` of the next event.
        // When starting a rescan, however, we set `last_event_time` to the current time, which may
        // be larger than the end time of the event that triggered the rescan. This allows us to
        // skip any events that were in the queue when the rescan was triggered (since their changes
        // have already been addressed by the rescan).
        if event.end_time < self.last_event_time {
            return None;
        }

        let manifest_event = if event.rescan {
            // Our underlying watcher had to drop events, so rescan all our manifests.
            // Update our `last_event_time` to now so we'll ignore any events that happen to
            // come in before we started this scan.
            let now = Instant::now();

            self.last_event_time = now;

            ManifestEvent {
                start_time: now,
                end_time: now,
                changed_manifests: self.rescan(),
                unwatched_manifests: HashSet::new(),
            }
        } else {
            self.last_event_time = event.end_time;

            let ManifestEventPaths { changed_manifests, unwatched_manifests } =
                self.handle_changed_paths(event.paths);

            ManifestEvent {
                start_time: event.start_time,
                end_time: event.end_time,
                changed_manifests,
                unwatched_manifests,
            }
        };

        // Don't bother sending along an empty list.
        if manifest_event.changed_manifests.is_empty()
            && manifest_event.unwatched_manifests.is_empty()
        {
            None
        } else {
            Some(manifest_event)
        }
    }

    // Returns paths of the changed and unwatched package manifest files.
    fn handle_changed_paths(&mut self, paths: HashSet<PathBuf>) -> ManifestEventPaths {
        let mut changed_manifests = HashSet::new();
        let mut unwatched_manifests = HashSet::new();

        for path in paths {
            let path = match Utf8PathBuf::try_from(path) {
                Ok(path) => path,
                Err(err) => {
                    error!("path contains invalid UTF-8: {}", err.as_path().display());
                    continue;
                }
            };

            // If the changed file is a package manifest list, update the watched manifests inside
            // of it. Otherwise let the caller know which package manifests changed.
            if let Some(old_manifests) = self.watched_list_to_manifests.get_mut(&path) {
                match self.file_watcher.update_package_manifest_list(&path, old_manifests) {
                    Ok(manifest_paths) => {
                        changed_manifests.extend(manifest_paths.changed_manifests);
                        unwatched_manifests.extend(manifest_paths.unwatched_manifests);
                    }
                    Err(err) => {
                        error!("error processing list {}: {:?}", path, err);
                    }
                }
            } else if self.file_watcher.is_watching_manifest(&path) {
                // Otherwise, treat it as a package manifest.
                match self.file_watcher.update_package_manifest(&path) {
                    Ok(true) => {
                        changed_manifests.insert(path);
                    }
                    Ok(false) => {}
                    Err(err) => {
                        error!("error processing manifest {}: {:?}", path, err);
                    }
                }
            }
        }

        ManifestEventPaths { changed_manifests, unwatched_manifests }
    }
}

struct ManifestEventPaths {
    changed_manifests: HashSet<Utf8PathBuf>,
    unwatched_manifests: HashSet<Utf8PathBuf>,
}

impl Stream for PackageManifestWatcher {
    type Item = ManifestEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match futures::ready!(self.as_mut().project().receiver.poll_next(cx)) {
                Some(Ok(event)) => {
                    if let Some(changed_paths) = self.handle_batch_event(event) {
                        return Poll::Ready(Some(changed_paths));
                    }
                }
                Some(Err(errs)) => {
                    for err in errs {
                        error!("notify batch watcher error: {err:#?}");
                    }
                }
                None => {
                    return Poll::Ready(None);
                }
            }
        }
    }
}

/// Watches manifest files and provides methods for watching and unwatching them.
///
/// This is factored out of [PackageManifestWatcher] to make it easier to work with ownership.
#[derive(Debug)]
struct ManifestFileWatcher {
    batch_watcher: BatchWatcher<RecommendedWatcher>,

    /// Hashed by parent dir, and the hash entry is a `HashMap` of file paths to number of clients
    /// watching the file in that dir.
    watched_paths: HashMap<Utf8PathBuf, HashMap<Utf8PathBuf, usize>>,

    /// Mapping from a manifest path to the package manifest's hash, which will be used to see if
    /// watched manifests change over time.
    manifest_hashes: HashMap<Utf8PathBuf, Option<Hash>>,
}

impl ManifestFileWatcher {
    fn new(batch_timeout: Duration) -> Result<(Self, Receiver<BatchResult>)> {
        // We use a batch size of zero so we have back pressure to the batch watcher thread. If the
        // stream is taking longer to process the batch watcher will just keep batching more notify
        // events into a batch until we're ready to consume it.
        let (mut event_tx, event_rx) = channel::<BatchResult>(0);

        // Some notify backends can emit multiple events when manipulating files. To reduce churn,
        // we'll spawn a background thread to batch up multiple events.
        let batch_watcher = BatchWatcher::builder()
            // We only care if the event modifies a path.
            .event_filter(|event| {
                matches!(
                    event.kind,
                    notify::EventKind::Create(_)
                        | notify::EventKind::Modify(_)
                        | notify::EventKind::Remove(_)
                )
            })
            .build(batch_timeout, move |result: BatchResult| {
                // Send the events along to the receiver. `BatchWatcher` implements back pressure,
                // so it's okay if this blocks until the receiver receives it.
                if let Err(err) = block_on(event_tx.send(result))
                    && !err.is_disconnected()
                {
                    error!("error sending manifest event: {err}");
                }
            })
            .map_err(|e| anyhow!("notify error: {:?}", e))?;

        Ok((
            Self { batch_watcher, watched_paths: HashMap::new(), manifest_hashes: HashMap::new() },
            event_rx,
        ))
    }

    /// Returns true if we are currently watching this path.
    fn is_watching_manifest(&self, path: &Utf8Path) -> bool {
        self.manifest_hashes.contains_key(path)
    }

    fn watch_path(&mut self, path: &Utf8Path) -> Result<()> {
        let Some(parent) = path.parent() else {
            return Err(format_err!("path does not have a parent: {path:?}"));
        };

        // Add this file to our watch list.
        let children = self.watched_paths.entry(parent.into()).or_default();

        // We only need to watch this file if we haven't watched it already.
        let needs_watch = children.is_empty();

        // Increment the reference count.
        let count = children.entry(path.to_owned()).or_default();
        *count += 1;

        if needs_watch {
            // Watch the parent path instead of directly watching the file to avoid
            // https://github.com/notify-rs/notify/issues/165.
            self.batch_watcher.watch(parent.as_std_path(), RecursiveMode::NonRecursive)?;
        }

        Ok(())
    }

    /// Unwatch a path.
    fn unwatch_path(&mut self, path: &Utf8Path) -> Result<bool> {
        // Canonicalize the paths, since that's what notify returns.
        let path = path.canonicalize_utf8()?;

        let Some(parent) = path.parent() else {
            return Err(format_err!("path does not have a parent: {path:?}"));
        };

        if let Some(ref mut children) = self.watched_paths.get_mut(parent)
            && let Some(count) = children.get_mut(&path)
        {
            *count -= 1;

            let path_removed = if *count == 0 {
                children.remove(&path);
                true
            } else {
                false
            };

            if children.is_empty() {
                self.watched_paths.remove(parent);
                self.batch_watcher.unwatch(parent.as_std_path())?;
            }

            return Ok(path_removed);
        }

        Ok(false)
    }

    /// Update our tracking of the package manifest. Returns true if this file is new, or has
    /// changed since we last saw it.
    fn update_package_manifest(&mut self, path: &Utf8Path) -> Result<bool> {
        let new_hash = read_package_manifest_hash(path)?;

        Ok(if let Some(old_hash) = self.manifest_hashes.get_mut(path) {
            if *old_hash == new_hash {
                false
            } else {
                *old_hash = new_hash;
                true
            }
        } else {
            self.manifest_hashes.insert(path.to_owned(), new_hash);
            true
        })
    }

    /// Reads a package manifest list from `path`, watches any new manifests, unwatches any removed
    /// manifests, and returns any the new manifests. Returns any added
    /// manifests, and any manifests we are unwatching.
    fn update_package_manifest_list(
        &mut self,
        path: &Utf8Path,
        old_manifests: &mut HashSet<Utf8PathBuf>,
    ) -> Result<ManifestEventPaths> {
        let new_manifests = read_package_manifest_list(path)?;

        let added_manifests =
            new_manifests.difference(old_manifests).cloned().collect::<HashSet<Utf8PathBuf>>();

        for path in &added_manifests {
            self.watch_path(path)?;
            self.update_package_manifest(path)?;
        }

        let mut unwatched_manifests = HashSet::new();
        for path in old_manifests.difference(&new_manifests) {
            if self.unwatch_path(path)? {
                // If we actually unwatched the path, we can remove this path from our hashes map,
                // so we don't leak space over time.
                self.manifest_hashes.remove(path);

                unwatched_manifests.insert(path.clone());
            }
        }

        *old_manifests = new_manifests;

        Ok(ManifestEventPaths { changed_manifests: added_manifests, unwatched_manifests })
    }
}

/// Read a package manifest. Returns `None` if the manifest file does not exist, or
/// is malformed (which can happen if we read the file while it was being written).
/// If the package doesn't
fn read_package_manifest_hash(path: &Utf8Path) -> Result<Option<Hash>> {
    let file = match File::open(path) {
        Ok(file) => BufReader::new(file),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err.into());
        }
    };

    match PackageManifest::from_reader(path, file) {
        Ok(manifest) => Ok(Some(manifest.hash())),
        Err(_) => Ok(None),
    }
}

/// Reads `PackageManifestList` from path. Returns the canonical path for each entry, since this is
/// what the `BatchWatcher` returns. Returns an empty set if the file does not exist, or is
/// malformed.
fn read_package_manifest_list(path: &Utf8Path) -> Result<HashSet<Utf8PathBuf>> {
    let file = match File::open(path) {
        Ok(file) => BufReader::new(file),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(HashSet::new()),
        Err(err) => {
            return Err(err.into());
        }
    };

    let list = match PackageManifestList::from_reader(path, file) {
        Ok(list) => list,
        Err(_) => {
            return Ok(HashSet::new());
        }
    };

    let parent = path.parent().ok_or_else(|| format_err!("path does not have a parent {path}"))?;

    // Paths in the package manifest list are relative to the location of the list.
    let list = list
        .into_iter()
        .map(|path| {
            parent.join(&path).canonicalize_utf8().with_context(|| format!("canonicalizing {path}"))
        })
        .collect::<Result<_>>()?;

    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils;
    use fuchsia_async::TimeoutExt;
    use fuchsia_pkg::PackageBuilder;
    use futures::StreamExt as _;
    use pretty_assertions::assert_eq;

    const TEST_BATCH_WATCHER_TIMEOUT: Duration = Duration::from_millis(100);

    async fn create_watcher(builder: PackageManifestWatcherBuilder) -> PackageManifestWatcher {
        builder.batch_timeout(TEST_BATCH_WATCHER_TIMEOUT).watch().unwrap()
    }

    fn create_populated_manifest_list<'a>(
        dir: &Utf8Path,
        name: &str,
        manifests: impl IntoIterator<Item = &'a Utf8Path>,
    ) -> Utf8PathBuf {
        let manifest_list = dir.join(name);

        create_manifest_list(&manifest_list, manifests);

        manifest_list
    }

    fn create_package_manifest(dir: &Utf8Path, i: u32) -> Utf8PathBuf {
        let package_name = format!("pkg{i}");
        let pkg_dir = dir.join(&package_name);
        let (_meta_far_path, manifest) =
            test_utils::make_package_manifest(&package_name, pkg_dir.as_std_path(), Vec::new());

        let manifest_path = pkg_dir.join(format!("{package_name}.manifest"));

        serde_json::to_writer(std::fs::File::create(&manifest_path).unwrap(), &manifest).unwrap();

        manifest_path
    }

    fn create_package_manifest_with_content(dir: &Utf8Path, i: u32, content: &str) -> Utf8PathBuf {
        let package_name = format!("pkg{i}");
        let pkg_dir = dir.join(&package_name);

        let mut builder = PackageBuilder::new(
            &package_name,
            // ABI revision for API level 7
            0xECCEA2F70ACD6FC0.into(),
        );

        builder
            .add_contents_as_blob(format!("bin/{package_name}"), content.as_bytes(), &pkg_dir)
            .unwrap();

        let meta_far_path = pkg_dir.join("meta.far");
        let manifest = builder.build(&pkg_dir, meta_far_path).unwrap();

        let manifest_path = pkg_dir.join(format!("{package_name}.manifest"));
        serde_json::to_writer(std::fs::File::create(&manifest_path).unwrap(), &manifest).unwrap();

        manifest_path
    }

    fn create_manifest_list<'a>(
        file_path: &Utf8Path,
        manifests: impl IntoIterator<Item = &'a Utf8Path>,
    ) {
        let file = File::create(file_path).unwrap();

        let parent = file_path.parent().unwrap();
        PackageManifestList::from_iter(
            manifests.into_iter().map(|path| path.strip_prefix(parent).unwrap().into()),
        )
        .to_writer(file)
        .unwrap();
    }

    fn update_manifest_list<'a>(
        file_path: &Utf8Path,
        manifests: impl IntoIterator<Item = &'a Utf8Path>,
    ) {
        let file = std::fs::OpenOptions::new().write(true).truncate(true).open(file_path).unwrap();
        let parent = file_path.parent().unwrap();
        PackageManifestList::from_iter(
            manifests.into_iter().map(|path| path.strip_prefix(parent).unwrap()).map(Into::into),
        )
        .to_writer(file)
        .unwrap();
    }

    // TODO(https://github.com/rust-lang/rust/issues/87417): use #[track_caller] when stabilized.
    macro_rules! check_no_events_were_emitted {
        ($watcher:expr) => {{
            async {
                let result = $watcher.next().await;
                panic!("should not have received: {result:?}");
            }
            .on_timeout(TEST_BATCH_WATCHER_TIMEOUT * 10, || ())
            .await;
        }};
    }

    /// Read changed paths from the watcher. It's possible events can be distributed across batches,
    /// so try to coalesce them.
    async fn check_changed_manifests(
        watcher: &mut PackageManifestWatcher,
        expected: HashSet<Utf8PathBuf>,
    ) {
        let mut actual = HashSet::new();

        let deadline = Instant::now() + Duration::from_secs(10);
        while let Some(duration) = deadline.checked_duration_since(Instant::now()) {
            if let Some(event) = watcher
                .next()
                .on_timeout(duration, || {
                    panic!("timed out fetching event: got {actual:#?}, expected {expected:#?}")
                })
                .await
            {
                actual.extend(event.changed_manifests);

                if actual.len() >= expected.len() {
                    break;
                }
            } else {
                panic!("received unexpected end of stream")
            };
        }

        assert_eq!(actual, expected);
    }

    async fn wait_for_manifests_to_be_unwatched(
        watcher: &mut PackageManifestWatcher,
        mut expected: HashSet<Utf8PathBuf>,
    ) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while let Some(duration) = deadline.checked_duration_since(Instant::now()) {
            if let Some(event) = watcher
                .next()
                .on_timeout(duration, || {
                    panic!("timed out waiting for paths to be unwatched: {expected:#?}");
                })
                .await
            {
                for path in event.unwatched_manifests {
                    expected.remove(&path);
                }

                if expected.is_empty() {
                    break;
                }
            } else {
                panic!("received unexpected end of stream")
            };
        }
    }

    #[fuchsia::test]
    async fn test_should_not_observe_unwatched_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap().canonicalize_utf8().unwrap();

        // Initialize a watcher with some files.
        let manifest_path = create_package_manifest(&dir, 1);
        let list_path =
            create_populated_manifest_list(&dir, "pkg-manifest-list", [manifest_path.as_path()]);

        let mut watcher =
            create_watcher(PackageManifestWatcher::builder().package_list(list_path.clone())).await;

        // Creating files next to the watched files should not be observed.
        let unwatched_manifest_path = create_package_manifest(&dir, 2);

        let _unwatched_list_path = create_populated_manifest_list(
            &dir,
            "unwatched-pkg-manifest-list",
            [unwatched_manifest_path.as_path()],
        );

        // Make sure we don't read any events from the stream.
        check_no_events_were_emitted!(&mut watcher);
    }

    #[fuchsia::test]
    async fn test_should_not_observe_changes_with_identical_content() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap().canonicalize_utf8().unwrap();

        // Initialize a watcher with some files.
        let manifest_path = create_package_manifest_with_content(&dir, 1, "1");

        let list_path =
            create_populated_manifest_list(&dir, "pkg-manifest-list", [manifest_path.as_path()]);

        let mut watcher =
            create_watcher(PackageManifestWatcher::builder().package_list(list_path.clone())).await;

        // Overwrite the package with the same content. This should not be
        // observed because the contents were not changed.
        create_package_manifest_with_content(&dir, 1, "1");

        // Make sure we don't read any events from the stream.
        check_no_events_were_emitted!(&mut watcher);

        // Overwrite the package with the same content. This should be observed
        // because the contents changed.
        create_package_manifest_with_content(&dir, 1, "2");
        check_changed_manifests(&mut watcher, HashSet::from([manifest_path])).await;
    }

    #[fuchsia::test]
    async fn test_create_modify_remove_recreate_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap().canonicalize_utf8().unwrap();

        let manifest_path1 = create_package_manifest_with_content(&dir, 1, "1");
        let manifest_path2 = create_package_manifest_with_content(&dir, 2, "1");

        let mut watcher = create_watcher(
            PackageManifestWatcher::builder()
                .package_manifest(manifest_path1.clone())
                .package_manifest(manifest_path2.clone()),
        )
        .await;

        // Next, write to the file and make sure we observe an event
        create_package_manifest_with_content(&dir, 1, "2");
        check_changed_manifests(&mut watcher, HashSet::from([manifest_path1.clone()])).await;

        // Next, write to another file and make sure we observe an event.
        create_package_manifest_with_content(&dir, 2, "2");
        check_changed_manifests(&mut watcher, HashSet::from([manifest_path2.clone()])).await;

        // Next, delete a file and make sure we observe an event.
        std::fs::remove_file(&manifest_path2).unwrap();
        check_changed_manifests(&mut watcher, HashSet::from([manifest_path2.clone()])).await;

        // Next, re-create a file and make sure we observe an event.
        create_package_manifest_with_content(&dir, 2, "2");
        check_changed_manifests(&mut watcher, HashSet::from([manifest_path2.clone()])).await;
    }

    #[fuchsia::test]
    async fn test_updating_manifest_lists() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap().canonicalize_utf8().unwrap();

        // We'll start out watching some empty package manifest lists.
        let list_path1 = create_populated_manifest_list(&dir, "pkg-manifest-list1", []);
        let list_path2 = create_populated_manifest_list(&dir, "pkg-manifest-list2", []);

        let mut watcher = create_watcher(
            PackageManifestWatcher::builder()
                .package_list(list_path1.clone())
                .package_list(list_path2.clone()),
        )
        .await;

        //////////////////////////////////////////////////////////////////////////////////////////
        // Add the packages to one of the lists, which should be observed.

        let manifest_path1 = create_package_manifest_with_content(&dir, 1, "1");
        let manifest_path2 = create_package_manifest_with_content(&dir, 2, "1");

        update_manifest_list(&list_path1, [manifest_path1.as_path(), manifest_path2.as_path()]);

        check_changed_manifests(
            &mut watcher,
            HashSet::from([manifest_path1.clone(), manifest_path2.clone()]),
        )
        .await;

        //////////////////////////////////////////////////////////////////////////////////////////
        // Add files to both lists, which should be observed. pkg1 and pkg2 should not be observed
        // because they were removed.

        let manifest_path3 = create_package_manifest_with_content(&dir, 3, "1");
        let manifest_path4 = create_package_manifest_with_content(&dir, 4, "1");

        update_manifest_list(&list_path1, [manifest_path3.as_path()]);
        update_manifest_list(&list_path2, [manifest_path4.as_path()]);

        check_changed_manifests(
            &mut watcher,
            HashSet::from([manifest_path3.clone(), manifest_path4.clone()]),
        )
        .await;

        //////////////////////////////////////////////////////////////////////////////////////////
        // We should only observe a single change if we change one package.

        create_package_manifest_with_content(&dir, 3, "2");

        check_changed_manifests(&mut watcher, HashSet::from([manifest_path3.clone()])).await;

        ///////////////////////////////////////////////////////////////////////
        // Updating multiple manifests files should be observed.

        create_package_manifest_with_content(&dir, 3, "3");
        create_package_manifest_with_content(&dir, 4, "3");

        check_changed_manifests(&mut watcher, HashSet::from([manifest_path3, manifest_path4]))
            .await;
    }

    #[fuchsia::test]
    async fn test_remove_manifest_list_stops_watching_manifests() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap().canonicalize_utf8().unwrap();

        let manifest_path1 = create_package_manifest_with_content(&dir, 1, "1");
        let manifest_path2 = create_package_manifest_with_content(&dir, 2, "1");

        let list_path1 = create_populated_manifest_list(
            &dir,
            "pkg-manifest-list1",
            [manifest_path1.as_path(), manifest_path2.as_path()],
        );

        let mut watcher =
            create_watcher(PackageManifestWatcher::builder().package_list(list_path1.clone()))
                .await;

        // Remove the manifest list.
        std::fs::remove_file(&list_path1).unwrap();

        // We should observe the package manifests were unwatched.
        wait_for_manifests_to_be_unwatched(
            &mut watcher,
            HashSet::from([manifest_path1, manifest_path2.clone()]),
        )
        .await;

        // Next, write to manifest file which is no longer watched.
        create_package_manifest_with_content(&dir, 1, "2");

        // Next, remove manifest file which is no longer watched.
        std::fs::remove_file(&manifest_path2).unwrap();

        // Try to read from the stream. This should not return anything since there were no
        // updates to files watched.
        check_no_events_were_emitted!(&mut watcher);
    }

    #[fuchsia::test]
    async fn test_delete_and_recreate_manifest_list() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap().canonicalize_utf8().unwrap();

        let manifest_path1 = create_package_manifest_with_content(&dir, 1, "1");
        let manifest_path2 = create_package_manifest_with_content(&dir, 2, "1");

        // We'll start out watching some empty package manifest lists.
        let list_path =
            create_populated_manifest_list(&dir, "pkg-manifest-list1", [manifest_path1.as_path()]);

        let mut watcher =
            create_watcher(PackageManifestWatcher::builder().package_list(list_path.clone())).await;

        // Delete the manifest list.
        std::fs::remove_file(&list_path).unwrap();

        // We should observe the package manifest being unwatched.
        wait_for_manifests_to_be_unwatched(&mut watcher, HashSet::from([manifest_path1])).await;

        // Next, recreate manifest list and make sure we observe an event.
        create_manifest_list(&list_path, [manifest_path2.as_path()]);
        check_changed_manifests(&mut watcher, HashSet::from([manifest_path2.clone()])).await;

        // Next, write to now-watched file and make sure we observe an event.
        create_package_manifest_with_content(&dir, 2, "2");
        check_changed_manifests(&mut watcher, HashSet::from([manifest_path2.clone()])).await;
    }

    #[fuchsia::test]
    async fn test_same_package_manifest_in_multiple_lists() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = Utf8Path::from_path(tmp.path()).unwrap().canonicalize_utf8().unwrap();

        let manifest_path = create_package_manifest_with_content(&dir, 1, "1");

        // Write the manifest in both lists.
        let list_path1 =
            create_populated_manifest_list(&dir, "pkg-manifest-list1", [manifest_path.as_path()]);
        let list_path2 =
            create_populated_manifest_list(&dir, "pkg-manifest-list2", [manifest_path.as_path()]);

        let mut watcher = create_watcher(
            PackageManifestWatcher::builder()
                .package_list(list_path1.clone())
                .package_list(list_path2.clone()),
        )
        .await;

        // Delete first manifest list. We shouldn't observe an event because the
        // file is still watched in the second list.
        std::fs::remove_file(&list_path1).unwrap();
        check_no_events_were_emitted!(&mut watcher);

        // Next, write to shared file and make sure we observe an event.
        create_package_manifest_with_content(&dir, 1, "2");
        check_changed_manifests(&mut watcher, HashSet::from([manifest_path.clone()])).await;

        // Delete second manifest list and make sure we observe an event.
        std::fs::remove_file(&list_path2).unwrap();
        wait_for_manifests_to_be_unwatched(&mut watcher, HashSet::from([manifest_path])).await;

        // Next, write to shared file, which should not be observed.
        create_package_manifest_with_content(&dir, 1, "3");
        check_no_events_were_emitted!(&mut watcher);
    }
}
