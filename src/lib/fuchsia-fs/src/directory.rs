// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! Utility functions for fuchsia.io directories.

use crate::PERM_READABLE;
use crate::node::{self, CloneError, CloseError, OpenError, RenameError};
use flex_fuchsia_io as fio;
use fuchsia_async::{DurationExt, MonotonicDuration, TimeoutExt};
use futures::future::BoxFuture;
use futures::stream::{self, BoxStream, StreamExt};
use std::collections::VecDeque;
use std::str::Utf8Error;
use thiserror::Error;
use zerocopy::{FromBytes, Immutable, KnownLayout, Ref, Unaligned};

use flex_client::ProxyHasDomain;
use flex_client::fidl::{ClientEnd, ProtocolMarker, ServerEnd};

mod watcher;
pub use watcher::{WatchEvent, WatchMessage, Watcher, WatcherCreateError, WatcherStreamError};

#[cfg(target_os = "fuchsia")]
#[cfg(not(feature = "fdomain"))]
pub use fuchsia::*;

#[cfg(not(target_os = "fuchsia"))]
pub use host::*;

#[cfg(target_os = "fuchsia")]
#[cfg(not(feature = "fdomain"))]
mod fuchsia {
    use super::*;
    use crate::file::ReadError;

    /// Opens the given `path` from the current namespace as a [`DirectoryProxy`].
    ///
    /// To connect to a filesystem node which doesn't implement fuchsia.io.Directory, use the
    /// functions in [`fuchsia_component::client`] instead.
    ///
    /// If the namespace path doesn't exist, or we fail to make the channel pair, this returns an
    /// error. However, if incorrect flags are sent, or if the rest of the path sent to the
    /// filesystem server doesn't exist, this will still return success. Instead, the returned
    /// DirectoryProxy channel pair will be closed with an epitaph.
    pub fn open_in_namespace(
        path: &str,
        flags: fio::Flags,
    ) -> Result<fio::DirectoryProxy, OpenError> {
        let (node, request) = fidl::endpoints::create_proxy();
        open_channel_in_namespace(path, flags, request)?;
        Ok(node)
    }

    /// Asynchronously opens the given [`path`] in the current namespace, serving the connection
    /// over [`request`]. Once the channel is connected, any calls made prior are serviced.
    ///
    /// To connect to a filesystem node which doesn't implement fuchsia.io.Directory, use the
    /// functions in [`fuchsia_component::client`] instead.
    ///
    /// If the namespace path doesn't exist, this returns an error. However, if incorrect flags are
    /// sent, or if the rest of the path sent to the filesystem server doesn't exist, this will
    /// still return success. Instead, the [`request`] channel will be closed with an epitaph.
    pub fn open_channel_in_namespace(
        path: &str,
        flags: fio::Flags,
        request: fidl::endpoints::ServerEnd<fio::DirectoryMarker>,
    ) -> Result<(), OpenError> {
        let mut flags = flags | fio::Flags::PROTOCOL_DIRECTORY;
        if flags.intersection(fio::MASK_KNOWN_PERMISSIONS).is_empty() {
            flags |= PERM_READABLE;
        }
        let namespace = fdio::Namespace::installed().map_err(OpenError::Namespace)?;
        namespace.open(path, flags, request.into_channel()).map_err(OpenError::Namespace)
    }

    /// Opens `path` from the `parent` directory as a file and reads the file contents into a Vec.
    pub async fn read_file(parent: &fio::DirectoryProxy, path: &str) -> Result<Vec<u8>, ReadError> {
        let flags = fio::Flags::FLAG_SEND_REPRESENTATION | PERM_READABLE;
        let file = open_file_async(parent, path, flags).map_err(ReadError::Open)?;
        crate::file::read_file_with_on_open_event(file).await
    }

    /// Atomically creates `path` with `contents`.
    ///
    /// This is done by opening an unnamed temporary file, writing the contents into it, and linking
    /// to `$(path).__tmp`, and then renaming the temporary file to `path`.
    ///
    /// Note that power failure after the temporary file was created, but before it was renamed,
    /// will result in the temporary file existing in the filesystem.  This function will always try
    /// to remove that file first, but if this function is never called, the temp file will never be
    /// removed.
    ///
    /// TODO(https://fxbug.dev/444270977): It would be preferable if we could avoid creating the
    /// named temporary file, but a limitation in Node/LinkInto prevents this.  Once that limitation
    /// is resolved, directly LinkInto over the target file, avoiding the need to clean up on poewr
    /// failure.
    ///
    /// Note that not all filesystems support this feature.
    ///
    /// Available at HEAD since FLAG_CREATE_AS_UNNAMED_TEMPORARY is available at HEAD
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    pub async fn atomic_write_file(
        parent: &fio::DirectoryProxy,
        path: &str,
        contents: &[u8],
    ) -> Result<(), crate::file::WriteError> {
        use crate::file::WriteError;
        let (dir, filename) = split_path(parent, path).await?;
        let dir = dir.as_ref().unwrap_or(parent);
        let tmp_filename = format!("{filename}.__tmp");
        // Clean up `tmp_filename` in case it still existed from a previous attempt.  Safe to ignore
        // errors.
        let _ = dir.unlink(&tmp_filename, &fio::UnlinkOptions::default()).await?;
        {
            let flags = fio::Flags::FLAG_SEND_REPRESENTATION
                | fio::Flags::PROTOCOL_FILE
                | fio::PERM_READABLE
                | fio::PERM_WRITABLE
                | fio::Flags::FLAG_CREATE_AS_UNNAMED_TEMPORARY;
            let tmp = open_file_async(dir, ".", flags).map_err(WriteError::Open)?;
            crate::file::write_file_with_on_open_event(&tmp, contents).await?;
            tmp.sync()
                .await
                .map_err(WriteError::Fidl)?
                .map_err(|s| WriteError::WriteError(zx::Status::err_from_raw(s)))?;
            let (status, token) = dir.get_token().await.map_err(WriteError::Fidl)?;
            zx::ok(status).map_err(WriteError::Link)?;
            let token = zx::Event::from(token.unwrap());
            tmp.link_into(token, &tmp_filename)
                .await
                .map_err(WriteError::Fidl)?
                .map_err(|s| WriteError::Link(zx::Status::err_from_raw(s)))?;
        }
        rename(dir, &tmp_filename, filename).await.map_err(|err| WriteError::Rename(err))
    }
}

#[cfg(not(target_os = "fuchsia"))]
mod host {
    use super::*;
    use crate::file::ReadError;

    /// Opens `path` from the `parent` directory as a file and reads the file contents into a Vec.
    pub async fn read_file(parent: &fio::DirectoryProxy, path: &str) -> Result<Vec<u8>, ReadError> {
        let file = open_file_async(parent, path, PERM_READABLE)?;
        crate::file::read(&file).await
    }
}

/// Error returned by readdir_recursive.
#[derive(Debug, Clone, Error)]
pub enum RecursiveEnumerateError {
    #[error("fidl error during {}: {:?}", _0, _1)]
    Fidl(&'static str, fidl::Error),

    #[error("Failed to read directory {}: {:?}", name, err)]
    ReadDir { name: String, err: EnumerateError },

    #[error("Failed to open directory {}: {:?}", name, err)]
    Open { name: String, err: OpenError },

    #[error("timeout")]
    Timeout,
}

/// Error returned by readdir.
#[derive(Debug, Clone, Error)]
pub enum EnumerateError {
    #[error("a directory entry could not be decoded: {:?}", _0)]
    DecodeDirent(DecodeDirentError),

    #[error("fidl error during {}: {:?}", _0, _1)]
    Fidl(&'static str, fidl::Error),

    #[error("`read_dirents` failed with status {:?}", _0)]
    ReadDirents(zx_status::Status),

    #[error("timeout")]
    Timeout,

    #[error("`rewind` failed with status {:?}", _0)]
    Rewind(zx_status::Status),

    #[error("`unlink` failed with status {:?}", _0)]
    Unlink(zx_status::Status),
}

/// An error encountered while decoding a single directory entry.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DecodeDirentError {
    #[error("an entry extended past the end of the buffer")]
    BufferOverrun,

    #[error("name is not valid utf-8: {}", _0)]
    InvalidUtf8(Utf8Error),
}

/// Opens the given `path` from the given `parent` directory as a [`DirectoryProxy`]. If open fails,
/// the returned `DirectoryProxy` will be closed with an epitaph.
pub fn open_directory_async(
    parent: &fio::DirectoryProxy,
    path: &str,
    flags: fio::Flags,
) -> Result<fio::DirectoryProxy, OpenError> {
    let (dir, server_end) = parent.domain().create_proxy::<fio::DirectoryMarker>();

    let mut flags = flags | fio::Flags::PROTOCOL_DIRECTORY;
    if flags.intersection(fio::MASK_KNOWN_PERMISSIONS).is_empty() {
        flags |= PERM_READABLE;
    }

    #[cfg(fuchsia_api_level_at_least = "27")]
    parent
        .open(path, flags, &fio::Options::default(), server_end.into_channel())
        .map_err(OpenError::SendOpenRequest)?;
    #[cfg(not(fuchsia_api_level_at_least = "27"))]
    parent
        .open3(path, flags, &fio::Options::default(), server_end.into_channel())
        .map_err(OpenError::SendOpenRequest)?;

    Ok(dir)
}

/// Opens the given `path` from given `parent` directory as a [`DirectoryProxy`], verifying that
/// the target implements the fuchsia.io.Directory protocol.
pub async fn open_directory(
    parent: &fio::DirectoryProxy,
    path: &str,
    flags: fio::Flags,
) -> Result<fio::DirectoryProxy, OpenError> {
    let (dir, server_end) = parent.domain().create_proxy::<fio::DirectoryMarker>();

    let mut flags = flags | fio::Flags::PROTOCOL_DIRECTORY | fio::Flags::FLAG_SEND_REPRESENTATION;
    if flags.intersection(fio::MASK_KNOWN_PERMISSIONS).is_empty() {
        flags |= PERM_READABLE;
    }

    #[cfg(fuchsia_api_level_at_least = "27")]
    parent
        .open(path, flags, &fio::Options::default(), server_end.into_channel())
        .map_err(OpenError::SendOpenRequest)?;
    #[cfg(not(fuchsia_api_level_at_least = "27"))]
    parent
        .open3(path, flags, &fio::Options::default(), server_end.into_channel())
        .map_err(OpenError::SendOpenRequest)?;

    // wait for the directory to open and report success.
    node::verify_directory_describe_event(dir).await
}

/// Creates a directory named `path` within the `parent` directory if it doesn't exist.
pub async fn create_directory(
    parent: &fio::DirectoryProxy,
    path: &str,
    flags: fio::Flags,
) -> Result<fio::DirectoryProxy, OpenError> {
    let (dir, server_end) = parent.domain().create_proxy::<fio::DirectoryMarker>();

    let flags = flags
        | fio::Flags::FLAG_MAYBE_CREATE
        | fio::Flags::PROTOCOL_DIRECTORY
        | fio::Flags::FLAG_SEND_REPRESENTATION;

    #[cfg(fuchsia_api_level_at_least = "27")]
    parent
        .open(path, flags, &fio::Options::default(), server_end.into_channel())
        .map_err(OpenError::SendOpenRequest)?;
    #[cfg(not(fuchsia_api_level_at_least = "27"))]
    parent
        .open3(path, flags, &fio::Options::default(), server_end.into_channel())
        .map_err(OpenError::SendOpenRequest)?;

    // wait for the directory to open and report success.
    node::verify_directory_describe_event(dir).await
}

/// Creates a directory named `path` (including all segments leading up to the terminal segment)
/// within the `parent` directory.  Returns a connection to the terminal directory.
pub async fn create_directory_recursive(
    parent: &fio::DirectoryProxy,
    path: &str,
    flags: fio::Flags,
) -> Result<fio::DirectoryProxy, OpenError> {
    let components = path.split('/');
    let mut dir = None;
    for part in components {
        dir = Some({
            let dir_ref = match dir.as_ref() {
                Some(r) => r,
                None => parent,
            };
            create_directory(dir_ref, part, flags).await?
        })
    }
    dir.ok_or(OpenError::OpenError(zx_status::Status::INVALID_ARGS))
}

/// Opens the given `path` from the given `parent` directory as a [`FileProxy`]. If open fails,
/// the returned `FileProxy` will be closed with an epitaph.
pub fn open_file_async(
    parent: &fio::DirectoryProxy,
    path: &str,
    flags: fio::Flags,
) -> Result<fio::FileProxy, OpenError> {
    let (file, server_end) = parent.domain().create_proxy::<fio::FileMarker>();

    let mut flags = flags | fio::Flags::PROTOCOL_FILE;
    if flags.intersection(fio::MASK_KNOWN_PERMISSIONS).is_empty() {
        flags |= PERM_READABLE;
    }

    #[cfg(fuchsia_api_level_at_least = "27")]
    parent
        .open(path, flags, &fio::Options::default(), server_end.into_channel())
        .map_err(OpenError::SendOpenRequest)?;
    #[cfg(not(fuchsia_api_level_at_least = "27"))]
    parent
        .open3(path, flags, &fio::Options::default(), server_end.into_channel())
        .map_err(OpenError::SendOpenRequest)?;

    Ok(file)
}

/// Opens the given `path` from given `parent` directory as a [`FileProxy`], verifying that the
/// target implements the fuchsia.io.File protocol.
pub async fn open_file(
    parent: &fio::DirectoryProxy,
    path: &str,
    flags: fio::Flags,
) -> Result<fio::FileProxy, OpenError> {
    let (file, server_end) = parent.domain().create_proxy::<fio::FileMarker>();

    let mut flags = flags | fio::Flags::PROTOCOL_FILE | fio::Flags::FLAG_SEND_REPRESENTATION;
    if flags.intersection(fio::MASK_KNOWN_PERMISSIONS).is_empty() {
        flags |= PERM_READABLE;
    }

    #[cfg(fuchsia_api_level_at_least = "27")]
    parent
        .open(path, flags, &fio::Options::default(), server_end.into_channel())
        .map_err(OpenError::SendOpenRequest)?;
    #[cfg(not(fuchsia_api_level_at_least = "27"))]
    parent
        .open3(path, flags, &fio::Options::default(), server_end.into_channel())
        .map_err(OpenError::SendOpenRequest)?;

    // wait for the file to open and report success.
    node::verify_file_describe_event(file).await
}

/// Opens the given `path` from the given `parent` directory as a [`NodeProxy`], verifying that the
/// target implements the fuchsia.io.Node protocol.
pub async fn open_node(
    parent: &fio::DirectoryProxy,
    path: &str,
    flags: fio::Flags,
) -> Result<fio::NodeProxy, OpenError> {
    let (file, server_end) = parent.domain().create_proxy::<fio::NodeMarker>();

    let flags = flags | fio::Flags::FLAG_SEND_REPRESENTATION;

    #[cfg(fuchsia_api_level_at_least = "27")]
    parent
        .open(path, flags, &fio::Options::default(), server_end.into_channel())
        .map_err(OpenError::SendOpenRequest)?;
    #[cfg(not(fuchsia_api_level_at_least = "27"))]
    parent
        .open3(path, flags, &fio::Options::default(), server_end.into_channel())
        .map_err(OpenError::SendOpenRequest)?;

    // wait for the file to open and report success.
    node::verify_node_describe_event(file).await
}

/// Opens the given `path` from the given `parent` directory as a [`P::Proxy`]. The target is not
/// verified to be any particular type and may not implement the [`P`] protocol.
pub fn open_async<P: ProtocolMarker>(
    parent: &fio::DirectoryProxy,
    path: &str,
    flags: fio::Flags,
) -> Result<P::Proxy, OpenError> {
    let (client, server_end) = parent.domain().create_endpoints::<P>();

    #[cfg(fuchsia_api_level_at_least = "27")]
    let () = parent
        .open(path, flags, &fio::Options::default(), server_end.into_channel())
        .map_err(OpenError::SendOpenRequest)?;
    #[cfg(not(fuchsia_api_level_at_least = "27"))]
    let () = parent
        .open3(path, flags, &fio::Options::default(), server_end.into_channel())
        .map_err(OpenError::SendOpenRequest)?;

    Ok(ClientEnd::<P>::new(client.into_channel()).into_proxy())
}

/// Opens a new connection to the given `directory`. The cloned connection has the same permissions.
pub fn clone(dir: &fio::DirectoryProxy) -> Result<fio::DirectoryProxy, CloneError> {
    let (client_end, server_end) = dir.domain().create_proxy::<fio::DirectoryMarker>();
    dir.clone(server_end.into_channel().into()).map_err(CloneError::SendCloneRequest)?;
    Ok(client_end)
}

/// Opens a new connection to the given `directory` using `request`. The cloned connection has the
/// same permissions as `directory`.
pub fn clone_onto(
    directory: &fio::DirectoryProxy,
    request: ServerEnd<fio::DirectoryMarker>,
) -> Result<(), CloneError> {
    directory.clone(request.into_channel().into()).map_err(CloneError::SendCloneRequest)
}

/// Gracefully closes the directory proxy from the remote end.
pub async fn close(dir: fio::DirectoryProxy) -> Result<(), CloseError> {
    let result = dir.close().await.map_err(CloseError::SendCloseRequest)?;
    result.map_err(|s| CloseError::CloseError(zx_status::Status::err_from_raw(s)))
}

/// Create a randomly named file in the given directory with the given prefix, and return its path
/// and `FileProxy`. `prefix` may contain "/".
pub async fn create_randomly_named_file(
    dir: &fio::DirectoryProxy,
    prefix: &str,
    flags: fio::Flags,
) -> Result<(String, fio::FileProxy), OpenError> {
    use rand::distr::{Alphanumeric, SampleString as _};
    use rand::rngs::SmallRng;
    let mut rng: SmallRng = rand::make_rng();

    let flags = flags | fio::Flags::FLAG_MUST_CREATE;

    loop {
        let random_string = Alphanumeric.sample_string(&mut rng, 6);
        let path = prefix.to_string() + &random_string;

        match open_file(dir, &path, flags).await {
            Ok(file) => return Ok((path, file)),
            Err(OpenError::OpenError(zx_status::Status::ALREADY_EXISTS)) => {}
            Err(err) => return Err(err),
        }
    }
}

// Split the given path under the directory into parent and file name, and open the parent directory
// if the path contains "/".
async fn split_path<'a>(
    dir: &fio::DirectoryProxy,
    path: &'a str,
) -> Result<(Option<fio::DirectoryProxy>, &'a str), OpenError> {
    match path.rsplit_once('/') {
        Some((parent, name)) => {
            let proxy = open_directory(
                dir,
                parent,
                fio::Flags::from_bits(fio::RW_STAR_DIR.bits()).unwrap(),
            )
            .await?;
            Ok((Some(proxy), name))
        }
        None => Ok((None, path)),
    }
}

/// Rename `src` to `dst` under the given directory, `src` and `dst` may contain "/".
pub async fn rename(dir: &fio::DirectoryProxy, src: &str, dst: &str) -> Result<(), RenameError> {
    use flex_client::Event;
    let (src_parent, src_filename) = split_path(dir, src).await?;
    let src_parent = src_parent.as_ref().unwrap_or(dir);
    let (dst_parent, dst_filename) = split_path(dir, dst).await?;
    let dst_parent = dst_parent.as_ref().unwrap_or(dir);
    let (status, dst_parent_dir_token) =
        dst_parent.get_token().await.map_err(RenameError::SendGetTokenRequest)?;
    zx_status::Status::ok(status).map_err(RenameError::GetTokenError)?;
    let event = Event::from(dst_parent_dir_token.ok_or(RenameError::NoHandleError)?);
    src_parent
        .rename(src_filename, event, dst_filename)
        .await
        .map_err(RenameError::SendRenameRequest)?
        .map_err(|s| RenameError::RenameError(zx_status::Status::err_from_raw(s)))
}

pub use fio::DirentType as DirentKind;

/// A directory entry.
#[derive(Clone, Eq, Ord, PartialOrd, PartialEq, Debug)]
pub struct DirEntry {
    /// The name of this node.
    pub name: String,

    /// The type of this node, or [`DirentKind::Unknown`] if not known.
    pub kind: DirentKind,
}

impl DirEntry {
    fn root() -> Self {
        Self { name: "".to_string(), kind: DirentKind::Directory }
    }

    fn is_dir(&self) -> bool {
        self.kind == DirentKind::Directory
    }

    fn is_root(&self) -> bool {
        self.is_dir() && self.name.is_empty()
    }

    fn chain(&self, subentry: &DirEntry) -> DirEntry {
        if self.name.is_empty() {
            DirEntry { name: subentry.name.clone(), kind: subentry.kind }
        } else {
            DirEntry { name: format!("{}/{}", self.name, subentry.name), kind: subentry.kind }
        }
    }
}

/// Returns Stream of nodes in tree rooted at the given DirectoryProxy for which |results_filter|
/// returns `true` plus any leaf (empty) directories. The results filter receives the directory
/// entry for the node in question and if the node is a directory, a reference a Vec of the
/// directory's contents. The function recurses into sub-directories for which |recurse_filter|
/// returns true. The returned entries will not include ".". |timeout| can be provided optionally
/// to specify the maximum time to wait for a directory to be read.
pub fn readdir_recursive_filtered<'a, ResultFn, RecurseFn>(
    dir: &'a fio::DirectoryProxy,
    timeout: Option<MonotonicDuration>,
    results_filter: ResultFn,
    recurse_filter: RecurseFn,
) -> BoxStream<'a, Result<DirEntry, RecursiveEnumerateError>>
where
    ResultFn: Fn(&DirEntry, Option<&Vec<DirEntry>>) -> bool + Send + Sync + Copy + 'a,
    RecurseFn: Fn(&DirEntry) -> bool + Send + Sync + Copy + 'a,
{
    let mut pending = VecDeque::new();
    pending.push_back(DirEntry::root());
    let results: VecDeque<DirEntry> = VecDeque::new();

    stream::unfold((results, pending), move |(mut results, mut pending)| {
        async move {
            loop {
                // Pending results to stream from the last read directory.
                if !results.is_empty() {
                    let result = results.pop_front().unwrap();
                    return Some((Ok(result), (results, pending)));
                }

                // No pending directories to read and per the last condition no pending results to
                // stream so finish the stream.
                if pending.is_empty() {
                    return None;
                }

                // The directory that will be read now.
                let dir_entry = pending.pop_front().unwrap();

                let sub_dir;
                let dir_ref = if dir_entry.is_root() {
                    dir
                } else {
                    match open_directory_async(dir, &dir_entry.name, fio::Flags::empty()) {
                        Ok(dir) => {
                            sub_dir = dir;
                            &sub_dir
                        }
                        Err(err) => {
                            let error = RecursiveEnumerateError::Open { name: dir_entry.name, err };
                            return Some((Err(error), (results, pending)));
                        }
                    }
                };

                let readdir_result = match timeout {
                    Some(timeout_duration) => readdir_with_timeout(dir_ref, timeout_duration).await,
                    None => readdir(&dir_ref).await,
                };
                let subentries = match readdir_result {
                    Ok(subentries) => subentries,
                    // Promote timeout error.
                    Err(EnumerateError::Timeout) => {
                        return Some((Err(RecursiveEnumerateError::Timeout), (results, pending)));
                    }
                    Err(err) => {
                        let error =
                            Err(RecursiveEnumerateError::ReadDir { name: dir_entry.name, err });
                        return Some((error, (results, pending)));
                    }
                };

                // If this is an empty directory and the caller is interested
                // in empty directories, emit that result.
                if subentries.is_empty()
                    && results_filter(&dir_entry, Some(&subentries))
                    && !dir_entry.name.is_empty()
                {
                    return Some((Ok(dir_entry), (results, pending)));
                }

                for subentry in subentries.into_iter() {
                    let subentry = dir_entry.chain(&subentry);
                    if subentry.is_dir() && recurse_filter(&subentry) {
                        pending.push_back(subentry.clone());
                    }
                    if results_filter(&subentry, None) {
                        results.push_back(subentry);
                    }
                }
            }
        }
    })
    .boxed()
}

/// Returns a Vec of all non-directory nodes and all empty directory nodes in the given directory
/// proxy. The returned entries will not include ".".
/// |timeout| can be provided optionally to specify the maximum time to wait for a directory to be
/// read.
pub fn readdir_recursive(
    dir: &fio::DirectoryProxy,
    timeout: Option<MonotonicDuration>,
) -> BoxStream<'_, Result<DirEntry, RecursiveEnumerateError>> {
    readdir_recursive_filtered(
        dir,
        timeout,
        |entry: &DirEntry, contents: Option<&Vec<DirEntry>>| {
            // We're interested in results which are not directories and any
            // empty directories.
            !entry.is_dir() || (contents.is_some() && contents.unwrap().is_empty())
        },
        |_| true,
    )
}

async fn readdir_inner(
    dir: &fio::DirectoryProxy,
    include_dot: bool,
) -> Result<Vec<DirEntry>, EnumerateError> {
    let status = dir.rewind().await.map_err(|e| EnumerateError::Fidl("rewind", e))?;
    zx_status::Status::ok(status).map_err(EnumerateError::Rewind)?;

    let mut entries = vec![];

    loop {
        let (status, buf) = dir
            .read_dirents(fio::MAX_BUF)
            .await
            .map_err(|e| EnumerateError::Fidl("read_dirents", e))?;
        zx_status::Status::ok(status).map_err(EnumerateError::ReadDirents)?;

        if buf.is_empty() {
            break;
        }

        for entry in parse_dir_entries(&buf) {
            let entry = entry.map_err(EnumerateError::DecodeDirent)?;
            if include_dot || entry.name != "." {
                entries.push(entry);
            }
        }
    }

    entries.sort_unstable();

    Ok(entries)
}

/// Returns a sorted Vec of directory entries contained directly in the given directory proxy.
/// (Like `readdir`, but includes the dot path as well.)
pub async fn readdir_inclusive(dir: &fio::DirectoryProxy) -> Result<Vec<DirEntry>, EnumerateError> {
    readdir_inner(dir, /*include_dot=*/ true).await
}

/// Returns a sorted Vec of directory entries contained directly in the given directory proxy. The
/// returned entries will not include "." or nodes from any subdirectories.
pub async fn readdir(dir: &fio::DirectoryProxy) -> Result<Vec<DirEntry>, EnumerateError> {
    readdir_inner(dir, /*include_dot=*/ false).await
}

/// Returns a sorted Vec of directory entries contained directly in the given directory proxy. The
/// returned entries will not include "." or nodes from any subdirectories. Timeouts if the read
/// takes longer than the given `timeout` duration.
pub async fn readdir_with_timeout(
    dir: &fio::DirectoryProxy,
    timeout: MonotonicDuration,
) -> Result<Vec<DirEntry>, EnumerateError> {
    readdir(&dir).on_timeout(timeout.after_now(), || Err(EnumerateError::Timeout)).await
}

/// Returns `true` if an entry with the specified name exists in the given directory.
pub async fn dir_contains(dir: &fio::DirectoryProxy, name: &str) -> Result<bool, EnumerateError> {
    Ok(readdir(&dir).await?.iter().any(|e| e.name == name))
}

/// Returns `true` if an entry with the specified name exists in the given directory.
///
/// Timesout if reading the directory's entries takes longer than the given `timeout`
/// duration.
pub async fn dir_contains_with_timeout(
    dir: &fio::DirectoryProxy,
    name: &str,
    timeout: MonotonicDuration,
) -> Result<bool, EnumerateError> {
    Ok(readdir_with_timeout(&dir, timeout).await?.iter().any(|e| e.name == name))
}

/// Parses the buffer returned by a read_dirents FIDL call.
///
/// Returns either an error or a parsed entry for each entry in the supplied buffer (see
/// read_dirents for the format of this buffer).
pub fn parse_dir_entries(mut buf: &[u8]) -> Vec<Result<DirEntry, DecodeDirentError>> {
    #[derive(KnownLayout, FromBytes, Immutable, Unaligned)]
    #[repr(C, packed)]
    struct Dirent {
        /// The inode number of the entry.
        _ino: u64,
        /// The length of the filename located after this entry.
        size: u8,
        /// The type of the entry. One of the `fio::DIRENT_TYPE_*` constants.
        kind: u8,
        // The unterminated name of the entry.  Length is the `size` field above.
        // char name[0],
    }

    let mut entries = vec![];

    while !buf.is_empty() {
        let Ok((dirent, rest)) = Ref::<_, Dirent>::from_prefix(buf) else {
            entries.push(Err(DecodeDirentError::BufferOverrun));
            return entries;
        };

        let entry = {
            // Don't read past the end of the buffer.
            let size = usize::from(dirent.size);
            if size > rest.len() {
                entries.push(Err(DecodeDirentError::BufferOverrun));
                return entries;
            }

            // Advance to the next entry.
            buf = &rest[size..];
            match String::from_utf8(rest[..size].to_vec()) {
                Ok(name) => Ok(DirEntry {
                    name,
                    kind: DirentKind::from_primitive(dirent.kind).unwrap_or(DirentKind::Unknown),
                }),
                Err(err) => Err(DecodeDirentError::InvalidUtf8(err.utf8_error())),
            }
        };

        entries.push(entry);
    }

    entries
}

const DIR_FLAGS: fio::Flags = fio::Flags::empty()
    .union(fio::Flags::PROTOCOL_DIRECTORY)
    .union(PERM_READABLE)
    .union(fio::Flags::PERM_INHERIT_WRITE);

/// Removes a directory and all of its children. `name` must be a subdirectory of `root_dir`.
///
/// The async analogue of `std::fs::remove_dir_all`.
pub async fn remove_dir_recursive(
    root_dir: &fio::DirectoryProxy,
    name: &str,
) -> Result<(), EnumerateError> {
    let (dir, dir_server) = root_dir.domain().create_proxy::<fio::DirectoryMarker>();

    #[cfg(fuchsia_api_level_at_least = "27")]
    root_dir
        .open(name, DIR_FLAGS, &fio::Options::default(), dir_server.into_channel())
        .map_err(|e| EnumerateError::Fidl("open", e))?;
    #[cfg(not(fuchsia_api_level_at_least = "27"))]
    root_dir
        .open3(name, DIR_FLAGS, &fio::Options::default(), dir_server.into_channel())
        .map_err(|e| EnumerateError::Fidl("open", e))?;
    remove_dir_contents(dir).await?;
    root_dir
        .unlink(
            name,
            &fio::UnlinkOptions {
                flags: Some(fio::UnlinkFlags::MUST_BE_DIRECTORY),
                ..Default::default()
            },
        )
        .await
        .map_err(|e| EnumerateError::Fidl("unlink", e))?
        .map_err(|s| EnumerateError::Unlink(zx_status::Status::err_from_raw(s)))
}

// Returns a `BoxFuture` instead of being async because async doesn't support recursion.
fn remove_dir_contents(dir: fio::DirectoryProxy) -> BoxFuture<'static, Result<(), EnumerateError>> {
    let fut = async move {
        for dirent in readdir(&dir).await? {
            match dirent.kind {
                DirentKind::Directory => {
                    let (subdir, subdir_server) =
                        dir.domain().create_proxy::<fio::DirectoryMarker>();
                    #[cfg(fuchsia_api_level_at_least = "27")]
                    dir.open(
                        &dirent.name,
                        DIR_FLAGS,
                        &fio::Options::default(),
                        subdir_server.into_channel(),
                    )
                    .map_err(|e| EnumerateError::Fidl("open", e))?;
                    #[cfg(not(fuchsia_api_level_at_least = "27"))]
                    dir.open3(
                        &dirent.name,
                        DIR_FLAGS,
                        &fio::Options::default(),
                        subdir_server.into_channel(),
                    )
                    .map_err(|e| EnumerateError::Fidl("open", e))?;
                    remove_dir_contents(subdir).await?;
                }
                _ => {}
            }
            dir.unlink(&dirent.name, &fio::UnlinkOptions::default())
                .await
                .map_err(|e| EnumerateError::Fidl("unlink", e))?
                .map_err(|s| EnumerateError::Unlink(zx_status::Status::err_from_raw(s)))?;
        }
        Ok(())
    };
    Box::pin(fut)
}

/// Opens `path` from the `parent` directory as a file and reads the file contents as a utf-8
/// encoded string.
#[cfg(not(feature = "fdomain"))]
pub async fn read_file_to_string(
    parent: &fio::DirectoryProxy,
    path: &str,
) -> Result<String, crate::file::ReadError> {
    let contents = read_file(parent, path).await?;
    Ok(String::from_utf8(contents)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::directory::OpenError;
    use crate::file::{ReadError, WriteError, write};
    use assert_matches::assert_matches;
    use fuchsia_async as fasync;
    use futures::channel::oneshot;
    use proptest::prelude::*;
    use tempfile::TempDir;
    use vfs::file::vmo::read_only;
    use vfs::pseudo_directory;
    use vfs::remote::remote_dir;

    const DATA_FILE_CONTENTS: &str = "Hello World!\n";

    #[cfg(target_os = "fuchsia")]
    const LONG_DURATION: MonotonicDuration = MonotonicDuration::from_seconds(30);

    #[cfg(not(target_os = "fuchsia"))]
    const LONG_DURATION: MonotonicDuration = MonotonicDuration::from_secs(30);

    proptest! {
        #[test]
        fn test_parse_dir_entries_does_not_crash(buf in prop::collection::vec(any::<u8>(), 0..200)) {
            parse_dir_entries(&buf);
        }
    }

    fn open_pkg() -> fio::DirectoryProxy {
        open_in_namespace("/pkg", fio::PERM_READABLE).unwrap()
    }

    fn open_tmp() -> (TempDir, fio::DirectoryProxy) {
        let tempdir = TempDir::new().expect("failed to create tmp dir");
        let proxy = open_in_namespace(
            tempdir.path().to_str().unwrap(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .unwrap();
        (tempdir, proxy)
    }

    fn open_data() -> (TempDir, fio::DirectoryProxy) {
        let tempdir = TempDir::new_in("/data").expect("failed to create tmp dir in /data");
        let proxy = open_in_namespace(
            tempdir.path().to_str().unwrap(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .unwrap();
        (tempdir, proxy)
    }

    // open_in_namespace

    #[fasync::run_singlethreaded(test)]
    async fn open_in_namespace_opens_real_dir() {
        let exists = open_in_namespace("/pkg", fio::PERM_READABLE).unwrap();
        assert_matches!(close(exists).await, Ok(()));
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_in_namespace_opens_fake_subdir_of_root_namespace_entry() {
        let notfound = open_in_namespace("/pkg/fake", fio::PERM_READABLE).unwrap();
        // The open error is not detected until the proxy is interacted with.
        assert_matches!(close(notfound).await, Err(_));
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_in_namespace_rejects_fake_root_namespace_entry() {
        let result = open_in_namespace("/fake", fio::PERM_READABLE);
        assert_matches!(result, Err(OpenError::Namespace(zx_status::Status::NOT_FOUND)));
        assert_matches!(result, Err(e) if e.is_not_found_error());
    }

    // open_directory_async

    #[fasync::run_singlethreaded(test)]
    async fn open_directory_async_opens_real_dir() {
        let pkg = open_pkg();
        let data = open_directory_async(&pkg, "data", fio::PERM_READABLE).unwrap();
        close(data).await.unwrap();
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_directory_async_opens_fake_dir() {
        let pkg = open_pkg();
        let fake = open_directory_async(&pkg, "fake", fio::PERM_READABLE).unwrap();
        // The open error is not detected until the proxy is interacted with.
        assert_matches!(close(fake).await, Err(_));
    }

    // open_directory

    #[fasync::run_singlethreaded(test)]
    async fn open_directory_opens_real_dir() {
        let pkg = open_pkg();
        let data = open_directory(&pkg, "data", fio::PERM_READABLE).await.unwrap();
        close(data).await.unwrap();
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_directory_rejects_fake_dir() {
        let pkg = open_pkg();

        let result = open_directory(&pkg, "fake", fio::PERM_READABLE).await;
        assert_matches!(result, Err(OpenError::OpenError(zx_status::Status::NOT_FOUND)));
        assert_matches!(result, Err(e) if e.is_not_found_error());
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_directory_rejects_file() {
        let pkg = open_pkg();

        assert_matches!(
            open_directory(&pkg, "data/file", fio::PERM_READABLE).await,
            Err(OpenError::OpenError(zx_status::Status::NOT_DIR))
        );
    }

    // create_directory

    #[fasync::run_singlethreaded(test)]
    async fn create_directory_simple() {
        let (_tmp, proxy) = open_tmp();
        let dir = create_directory(&proxy, "dir", fio::PERM_READABLE).await.unwrap();
        crate::directory::close(dir).await.unwrap();
    }

    #[fasync::run_singlethreaded(test)]
    async fn create_directory_add_file() {
        let (_tmp, proxy) = open_tmp();
        let dir =
            create_directory(&proxy, "dir", fio::PERM_READABLE | fio::PERM_WRITABLE).await.unwrap();
        let file = open_file(&dir, "data", fio::Flags::FLAG_MUST_CREATE | fio::PERM_READABLE)
            .await
            .unwrap();
        crate::file::close(file).await.unwrap();
    }

    #[fasync::run_singlethreaded(test)]
    async fn create_directory_existing_dir_opens() {
        let (_tmp, proxy) = open_tmp();
        let dir = create_directory(&proxy, "dir", fio::PERM_READABLE).await.unwrap();
        crate::directory::close(dir).await.unwrap();
        create_directory(&proxy, "dir", fio::PERM_READABLE).await.unwrap();
    }

    #[fasync::run_singlethreaded(test)]
    async fn create_directory_existing_dir_fails_if_must_create() {
        let (_tmp, proxy) = open_tmp();
        let dir =
            create_directory(&proxy, "dir", fio::Flags::FLAG_MUST_CREATE | fio::PERM_READABLE)
                .await
                .unwrap();
        crate::directory::close(dir).await.unwrap();
        assert_matches!(
            create_directory(&proxy, "dir", fio::Flags::FLAG_MUST_CREATE | fio::PERM_READABLE)
                .await,
            Err(_)
        );
    }

    // open_file_async

    #[fasync::run_singlethreaded(test)]
    async fn open_file_no_describe_opens_real_file() {
        let pkg = open_pkg();
        let file = open_file_async(&pkg, "data/file", fio::PERM_READABLE).unwrap();
        crate::file::close(file).await.unwrap();
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_file_no_describe_opens_fake_file() {
        let pkg = open_pkg();
        let fake = open_file_async(&pkg, "data/fake", fio::PERM_READABLE).unwrap();
        // The open error is not detected until the proxy is interacted with.
        assert_matches!(crate::file::close(fake).await, Err(_));
    }

    // open_file

    #[fasync::run_singlethreaded(test)]
    async fn open_file_opens_real_file() {
        let pkg = open_pkg();
        let file = open_file(&pkg, "data/file", fio::PERM_READABLE).await.unwrap();
        assert_eq!(
            file.seek(fio::SeekOrigin::End, 0).await.unwrap(),
            Ok(DATA_FILE_CONTENTS.len() as u64),
        );
        crate::file::close(file).await.unwrap();
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_file_rejects_fake_file() {
        let pkg = open_pkg();

        let result = open_file(&pkg, "data/fake", fio::PERM_READABLE).await;
        assert_matches!(result, Err(OpenError::OpenError(zx_status::Status::NOT_FOUND)));
        assert_matches!(result, Err(e) if e.is_not_found_error());
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_file_rejects_dir() {
        let pkg = open_pkg();

        assert_matches!(
            open_file(&pkg, "data", fio::PERM_READABLE).await,
            Err(OpenError::UnexpectedNodeKind {
                expected: node::Kind::File,
                actual: node::Kind::Directory,
            } | node::OpenError::OpenError(zx_status::Status::NOT_FILE))
        );
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_file_flags() {
        let tempdir = TempDir::new().expect("failed to create tmp dir");
        std::fs::write(tempdir.path().join("read_write"), "rw/read_write")
            .expect("failed to write file");
        let dir = crate::directory::open_in_namespace(
            tempdir.path().to_str().unwrap(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .expect("could not open tmp dir");
        let example_dir = pseudo_directory! {
            "ro" => pseudo_directory! {
                "read_only" => read_only("ro/read_only"),
            },
            "rw" => remote_dir(dir)
        };
        let example_dir_proxy = vfs::directory::serve(
            example_dir,
            vfs::execution_scope::ExecutionScope::new(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        );

        for (file_name, flags, should_succeed) in vec![
            ("ro/read_only", fio::PERM_READABLE, true),
            ("ro/read_only", fio::PERM_READABLE | fio::PERM_WRITABLE, false),
            ("ro/read_only", fio::PERM_WRITABLE, false),
            ("rw/read_write", fio::PERM_READABLE, true),
            ("rw/read_write", fio::PERM_READABLE | fio::PERM_WRITABLE, true),
            ("rw/read_write", fio::PERM_WRITABLE, true),
        ] {
            // open_file_async

            let file = open_file_async(&example_dir_proxy, file_name, flags).unwrap();
            match (should_succeed, file.query().await) {
                (true, Ok(_)) => (),
                (false, Err(_)) => continue,
                (true, Err(e)) => {
                    panic!("failed to open when expected success, couldn't describe: {:?}", e)
                }
                (false, Ok(d)) => {
                    panic!("successfully opened when expected failure, could describe: {:?}", d)
                }
            }
            if flags.intersects(fio::Flags::PERM_READ_BYTES) {
                assert_eq!(crate::file::read_to_string(&file).await.unwrap(), file_name);
            }
            if flags.intersects(fio::Flags::PERM_WRITE_BYTES) {
                let _ = file.seek(fio::SeekOrigin::Start, 0).await.expect("Seek failed!");
                let _: u64 = file
                    .write(file_name.as_bytes())
                    .await
                    .unwrap()
                    .map_err(zx_status::Status::err_from_raw)
                    .unwrap();
            }
            crate::file::close(file).await.unwrap();

            // open_file

            match open_file(&example_dir_proxy, file_name, flags).await {
                Ok(file) if should_succeed => {
                    if flags.intersects(fio::Flags::PERM_READ_BYTES) {
                        assert_eq!(crate::file::read_to_string(&file).await.unwrap(), file_name);
                    }
                    if flags.intersects(fio::Flags::PERM_WRITE_BYTES) {
                        let _ = file.seek(fio::SeekOrigin::Start, 0).await.expect("Seek failed!");
                        let _: u64 = file
                            .write(file_name.as_bytes())
                            .await
                            .unwrap()
                            .map_err(zx_status::Status::err_from_raw)
                            .unwrap();
                    }
                    crate::file::close(file).await.unwrap();
                }
                Ok(_) => {
                    panic!("successfully opened when expected failure: {:?}", (file_name, flags))
                }
                Err(e) if should_succeed => {
                    panic!("failed to open when expected success: {:?}", (e, file_name, flags))
                }
                Err(_) => {}
            }
        }
    }

    // open_node

    #[fasync::run_singlethreaded(test)]
    async fn open_node_opens_real_node() {
        let pkg = open_pkg();
        let node = open_node(&pkg, "data", fio::PERM_READABLE).await.unwrap();
        crate::node::close(node).await.unwrap();
    }

    #[fasync::run_singlethreaded(test)]
    async fn open_node_opens_fake_node() {
        let pkg = open_pkg();
        // The open error should be detected immediately.
        assert_matches!(open_node(&pkg, "fake", fio::PERM_READABLE).await, Err(_));
    }

    // create_randomly_named_file

    #[fasync::run_singlethreaded(test)]
    async fn create_randomly_named_file_simple() {
        let (_tmp, proxy) = open_tmp();
        let (path, file) =
            create_randomly_named_file(&proxy, "prefix", fio::PERM_WRITABLE).await.unwrap();
        assert!(path.starts_with("prefix"));
        crate::file::close(file).await.unwrap();
    }

    #[fasync::run_singlethreaded(test)]
    async fn create_randomly_named_file_subdir() {
        let (_tmp, proxy) = open_tmp();
        let _subdir = create_directory(&proxy, "subdir", fio::PERM_WRITABLE).await.unwrap();
        let (path, file) =
            create_randomly_named_file(&proxy, "subdir/file", fio::PERM_WRITABLE).await.unwrap();
        assert!(path.starts_with("subdir/file"));
        crate::file::close(file).await.unwrap();
    }

    #[fasync::run_singlethreaded(test)]
    async fn create_randomly_named_file_no_prefix() {
        let (_tmp, proxy) = open_tmp();
        let (_path, file) =
            create_randomly_named_file(&proxy, "", fio::PERM_READABLE | fio::PERM_WRITABLE)
                .await
                .unwrap();
        crate::file::close(file).await.unwrap();
    }

    #[fasync::run_singlethreaded(test)]
    async fn create_randomly_named_file_error() {
        let pkg = open_pkg();
        assert_matches!(create_randomly_named_file(&pkg, "", fio::Flags::empty()).await, Err(_));
    }

    // rename

    #[fasync::run_singlethreaded(test)]
    async fn rename_simple() {
        let (tmp, proxy) = open_tmp();
        let (path, file) =
            create_randomly_named_file(&proxy, "", fio::PERM_WRITABLE).await.unwrap();
        crate::file::close(file).await.unwrap();
        rename(&proxy, &path, "new_path").await.unwrap();
        assert!(!tmp.path().join(path).exists());
        assert!(tmp.path().join("new_path").exists());
    }

    #[fasync::run_singlethreaded(test)]
    async fn rename_with_subdir() {
        let (tmp, proxy) = open_tmp();
        let _subdir1 = create_directory(&proxy, "subdir1", fio::PERM_WRITABLE).await.unwrap();
        let _subdir2 = create_directory(&proxy, "subdir2", fio::PERM_WRITABLE).await.unwrap();
        let (path, file) =
            create_randomly_named_file(&proxy, "subdir1/file", fio::PERM_WRITABLE).await.unwrap();
        crate::file::close(file).await.unwrap();
        rename(&proxy, &path, "subdir2/file").await.unwrap();
        assert!(!tmp.path().join(path).exists());
        assert!(tmp.path().join("subdir2/file").exists());
    }

    #[fasync::run_singlethreaded(test)]
    async fn rename_directory() {
        let (tmp, proxy) = open_tmp();
        let dir = create_directory(&proxy, "dir", fio::PERM_WRITABLE).await.unwrap();
        close(dir).await.unwrap();
        rename(&proxy, "dir", "dir2").await.unwrap();
        assert!(!tmp.path().join("dir").exists());
        assert!(tmp.path().join("dir2").exists());
    }

    #[fasync::run_singlethreaded(test)]
    async fn rename_overwrite_existing_file() {
        let (tmp, proxy) = open_tmp();
        std::fs::write(tmp.path().join("foo"), b"foo").unwrap();
        std::fs::write(tmp.path().join("bar"), b"bar").unwrap();
        rename(&proxy, "foo", "bar").await.unwrap();
        assert!(!tmp.path().join("foo").exists());
        assert_eq!(std::fs::read_to_string(tmp.path().join("bar")).unwrap(), "foo");
    }

    #[fasync::run_singlethreaded(test)]
    async fn rename_non_existing_src_fails() {
        let (tmp, proxy) = open_tmp();
        assert_matches!(
            rename(&proxy, "foo", "bar").await,
            Err(RenameError::RenameError(zx_status::Status::NOT_FOUND))
        );
        assert!(!tmp.path().join("foo").exists());
        assert!(!tmp.path().join("bar").exists());
    }

    #[fasync::run_singlethreaded(test)]
    async fn rename_to_non_existing_subdir_fails() {
        let (tmp, proxy) = open_tmp();
        std::fs::write(tmp.path().join("foo"), b"foo").unwrap();
        assert_matches!(
            rename(&proxy, "foo", "bar/foo").await,
            Err(RenameError::OpenError(OpenError::OpenError(zx_status::Status::NOT_FOUND)))
        );
        assert!(tmp.path().join("foo").exists());
        assert!(!tmp.path().join("bar/foo").exists());
    }

    #[fasync::run_singlethreaded(test)]
    async fn rename_root_path_fails() {
        let (tmp, proxy) = open_tmp();
        assert_matches!(
            rename(&proxy, "/foo", "bar").await,
            Err(RenameError::OpenError(OpenError::OpenError(zx_status::Status::INVALID_ARGS)))
        );
        assert!(!tmp.path().join("bar").exists());
    }

    // parse_dir_entries

    #[test]
    fn test_parse_dir_entries_rejects_invalid_utf8() {
        #[rustfmt::skip]
        let buf = &[
            // entry 0
            // ino
            1, 0, 0, 0, 0, 0, 0, 0,
            // name length
            1,
            // type
            fio::DirentType::File.into_primitive(),
            // name (a lonely continuation byte)
            0x80,
            // entry 1
            // ino
            2, 0, 0, 0, 0, 0, 0, 0,
            // name length
            4,
            // type
            fio::DirentType::File.into_primitive(),
            // name
            'o' as u8, 'k' as u8, 'a' as u8, 'y' as u8,
        ];

        #[allow(unknown_lints, invalid_from_utf8)]
        let expected_err = std::str::from_utf8(&[0x80]).unwrap_err();

        assert_eq!(
            parse_dir_entries(buf),
            vec![
                Err(DecodeDirentError::InvalidUtf8(expected_err)),
                Ok(DirEntry { name: "okay".to_string(), kind: DirentKind::File })
            ]
        );
    }

    #[test]
    fn test_parse_dir_entries_overrun() {
        #[rustfmt::skip]
        let buf = &[
            // ino
            0, 0, 0, 0, 0, 0, 0, 0,
            // name length
            5,
            // type
            fio::DirentType::File.into_primitive(),
            // name
            't' as u8, 'e' as u8, 's' as u8, 't' as u8,
        ];

        assert_eq!(parse_dir_entries(buf), vec![Err(DecodeDirentError::BufferOverrun)]);
    }

    // readdir

    #[fasync::run_singlethreaded(test)]
    async fn test_readdir() {
        let dir = pseudo_directory! {
            "afile" => read_only(""),
            "zzz" => read_only(""),
            "subdir" => pseudo_directory! {
                "ignored" => read_only(""),
            },
        };
        let dir_proxy =
            vfs::directory::serve_read_only(dir, vfs::execution_scope::ExecutionScope::new());

        // run twice to check that seek offset is properly reset before reading the directory
        for _ in 0..2 {
            let entries = readdir(&dir_proxy).await.expect("readdir failed");
            assert_eq!(
                entries,
                vec![
                    build_direntry("afile", DirentKind::File),
                    build_direntry("subdir", DirentKind::Directory),
                    build_direntry("zzz", DirentKind::File),
                ]
            );
        }
    }

    // dir_contains

    #[fasync::run_singlethreaded(test)]
    async fn test_dir_contains() {
        let dir = pseudo_directory! {
            "afile" => read_only(""),
            "zzz" => read_only(""),
            "subdir" => pseudo_directory! {
                "ignored" => read_only(""),
            },
        };
        let dir_proxy =
            vfs::directory::serve_read_only(dir, vfs::execution_scope::ExecutionScope::new());

        for file in &["afile", "zzz", "subdir"] {
            assert!(dir_contains(&dir_proxy, file).await.unwrap());
        }

        assert!(
            !dir_contains(&dir_proxy, "notin").await.expect("error checking if dir contains notin")
        );
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_dir_contains_with_timeout() {
        let tempdir = TempDir::new().expect("failed to create tmp dir");
        let dir = create_nested_dir(&tempdir).await;
        let first = dir_contains_with_timeout(&dir, "notin", LONG_DURATION)
            .await
            .expect("error checking dir contains notin");
        assert!(!first);
        let second = dir_contains_with_timeout(&dir, "a", LONG_DURATION)
            .await
            .expect("error checking dir contains a");
        assert!(second);
    }

    // readdir_recursive

    #[fasync::run_singlethreaded(test)]
    async fn test_readdir_recursive() {
        let tempdir = TempDir::new().expect("failed to create tmp dir");
        let dir = create_nested_dir(&tempdir).await;
        // run twice to check that seek offset is properly reset before reading the directory
        for _ in 0..2 {
            let (tx, rx) = oneshot::channel();
            let clone_dir = clone(&dir).expect("clone dir");
            fasync::Task::spawn(async move {
                let entries = readdir_recursive(&clone_dir, None)
                    .collect::<Vec<Result<DirEntry, RecursiveEnumerateError>>>()
                    .await
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()
                    .expect("readdir_recursive failed");
                tx.send(entries).expect("sending entries failed");
            })
            .detach();
            let entries = rx.await.expect("receiving entries failed");
            assert_eq!(
                entries,
                vec![
                    build_direntry("a", DirentKind::File),
                    build_direntry("b", DirentKind::File),
                    build_direntry("emptydir", DirentKind::Directory),
                    build_direntry("subdir/a", DirentKind::File),
                    build_direntry("subdir/subsubdir/a", DirentKind::File),
                    build_direntry("subdir/subsubdir/emptydir", DirentKind::Directory),
                ]
            );
        }
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_readdir_recursive_timeout_expired() {
        // This test must use a forever-pending server in order to ensure that the timeout
        // triggers before the function under test finishes, even if the timeout is
        // in the past.
        let (dir, _server) = fidl::endpoints::create_proxy::<fio::DirectoryMarker>();
        let result = readdir_recursive(&dir, Some(zx::MonotonicDuration::from_nanos(0)))
            .collect::<Vec<Result<DirEntry, RecursiveEnumerateError>>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>();
        assert!(result.is_err());
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_readdir_recursive_timeout() {
        let tempdir = TempDir::new().expect("failed to create tmp dir");
        let dir = create_nested_dir(&tempdir).await;
        let entries = readdir_recursive(&dir, Some(LONG_DURATION))
            .collect::<Vec<Result<DirEntry, RecursiveEnumerateError>>>()
            .await
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .expect("readdir_recursive failed");
        assert_eq!(
            entries,
            vec![
                build_direntry("a", DirentKind::File),
                build_direntry("b", DirentKind::File),
                build_direntry("emptydir", DirentKind::Directory),
                build_direntry("subdir/a", DirentKind::File),
                build_direntry("subdir/subsubdir/a", DirentKind::File),
                build_direntry("subdir/subsubdir/emptydir", DirentKind::Directory),
            ]
        );
    }

    // remove_dir

    #[fasync::run_singlethreaded(test)]
    async fn test_remove_dir_recursive() {
        {
            let tempdir = TempDir::new().expect("failed to create tmp dir");
            let dir = create_nested_dir(&tempdir).await;
            remove_dir_recursive(&dir, "emptydir").await.expect("remove_dir_recursive failed");
            let entries = readdir_recursive(&dir, None)
                .collect::<Vec<Result<DirEntry, RecursiveEnumerateError>>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .expect("readdir_recursive failed");
            assert_eq!(
                entries,
                vec![
                    build_direntry("a", DirentKind::File),
                    build_direntry("b", DirentKind::File),
                    build_direntry("subdir/a", DirentKind::File),
                    build_direntry("subdir/subsubdir/a", DirentKind::File),
                    build_direntry("subdir/subsubdir/emptydir", DirentKind::Directory),
                ]
            );
        }
        {
            let tempdir = TempDir::new().expect("failed to create tmp dir");
            let dir = create_nested_dir(&tempdir).await;
            remove_dir_recursive(&dir, "subdir").await.expect("remove_dir_recursive failed");
            let entries = readdir_recursive(&dir, None)
                .collect::<Vec<Result<DirEntry, RecursiveEnumerateError>>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .expect("readdir_recursive failed");
            assert_eq!(
                entries,
                vec![
                    build_direntry("a", DirentKind::File),
                    build_direntry("b", DirentKind::File),
                    build_direntry("emptydir", DirentKind::Directory),
                ]
            );
        }
        {
            let tempdir = TempDir::new().expect("failed to create tmp dir");
            let dir = create_nested_dir(&tempdir).await;
            let subdir = open_directory(&dir, "subdir", fio::PERM_READABLE | fio::PERM_WRITABLE)
                .await
                .expect("could not open subdir");
            remove_dir_recursive(&subdir, "subsubdir").await.expect("remove_dir_recursive failed");
            let entries = readdir_recursive(&dir, None)
                .collect::<Vec<Result<DirEntry, RecursiveEnumerateError>>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .expect("readdir_recursive failed");
            assert_eq!(
                entries,
                vec![
                    build_direntry("a", DirentKind::File),
                    build_direntry("b", DirentKind::File),
                    build_direntry("emptydir", DirentKind::Directory),
                    build_direntry("subdir/a", DirentKind::File),
                ]
            );
        }
        {
            let tempdir = TempDir::new().expect("failed to create tmp dir");
            let dir = create_nested_dir(&tempdir).await;
            let subsubdir =
                open_directory(&dir, "subdir/subsubdir", fio::PERM_READABLE | fio::PERM_WRITABLE)
                    .await
                    .expect("could not open subsubdir");
            remove_dir_recursive(&subsubdir, "emptydir")
                .await
                .expect("remove_dir_recursive failed");
            let entries = readdir_recursive(&dir, None)
                .collect::<Vec<Result<DirEntry, RecursiveEnumerateError>>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .expect("readdir_recursive failed");
            assert_eq!(
                entries,
                vec![
                    build_direntry("a", DirentKind::File),
                    build_direntry("b", DirentKind::File),
                    build_direntry("emptydir", DirentKind::Directory),
                    build_direntry("subdir/a", DirentKind::File),
                    build_direntry("subdir/subsubdir/a", DirentKind::File),
                ]
            );
        }
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_remove_dir_recursive_errors() {
        {
            let tempdir = TempDir::new().expect("failed to create tmp dir");
            let dir = create_nested_dir(&tempdir).await;
            let res = remove_dir_recursive(&dir, "baddir").await;
            let res = res.expect_err("remove_dir did not fail");
            match res {
                EnumerateError::Fidl("rewind", fidl_error) if fidl_error.is_closed() => {}
                _ => panic!("unexpected error {:?}", res),
            }
        }
        {
            let tempdir = TempDir::new().expect("failed to create tmp dir");
            let dir = create_nested_dir(&tempdir).await;
            let res = remove_dir_recursive(&dir, ".").await;
            let expected: Result<(), EnumerateError> =
                Err(EnumerateError::Unlink(zx_status::Status::INVALID_ARGS));
            assert_eq!(format!("{:?}", res), format!("{:?}", expected));
        }
    }

    // create_directory_recursive

    #[fasync::run_singlethreaded(test)]
    async fn create_directory_recursive_test() {
        let tempdir = TempDir::new().unwrap();

        let path = "path/to/example/dir";
        let file_name = "example_file_name";
        let data = "file contents";

        let root_dir = open_in_namespace(
            tempdir.path().to_str().unwrap(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .expect("open_in_namespace failed");

        let sub_dir =
            create_directory_recursive(&root_dir, &path, fio::PERM_READABLE | fio::PERM_WRITABLE)
                .await
                .expect("create_directory_recursive failed");
        let file = open_file(
            &sub_dir,
            &file_name,
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .expect("open_file failed");

        write(&file, &data).await.expect("writing to the file failed");

        let contents = std::fs::read_to_string(tempdir.path().join(path).join(file_name))
            .expect("read_to_string failed");
        assert_eq!(&contents, &data, "File contents did not match");
    }

    async fn create_nested_dir(tempdir: &TempDir) -> fio::DirectoryProxy {
        let dir = open_in_namespace(
            tempdir.path().to_str().unwrap(),
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .expect("could not open tmp dir");
        create_directory_recursive(&dir, "emptydir", fio::PERM_READABLE | fio::PERM_WRITABLE)
            .await
            .expect("failed to create emptydir");
        create_directory_recursive(
            &dir,
            "subdir/subsubdir/emptydir",
            fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .expect("failed to create subdir/subsubdir/emptydir");
        create_file(&dir, "a").await;
        create_file(&dir, "b").await;
        create_file(&dir, "subdir/a").await;
        create_file(&dir, "subdir/subsubdir/a").await;
        dir
    }

    async fn create_file(dir: &fio::DirectoryProxy, path: &str) {
        open_file(
            dir,
            path,
            fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE | fio::PERM_WRITABLE,
        )
        .await
        .unwrap_or_else(|e| panic!("failed to create {}: {:?}", path, e));
    }

    fn build_direntry(name: &str, kind: DirentKind) -> DirEntry {
        DirEntry { name: name.to_string(), kind }
    }

    // DirEntry

    #[test]
    fn test_direntry_is_dir() {
        assert!(build_direntry("foo", DirentKind::Directory).is_dir());

        // Negative test
        assert!(!build_direntry("foo", DirentKind::File).is_dir());
        assert!(!build_direntry("foo", DirentKind::Unknown).is_dir());
    }

    #[test]
    fn test_direntry_chaining() {
        let parent = build_direntry("foo", DirentKind::Directory);

        let child1 = build_direntry("bar", DirentKind::Directory);
        let chained1 = parent.chain(&child1);
        assert_eq!(&chained1.name, "foo/bar");
        assert_eq!(chained1.kind, DirentKind::Directory);

        let child2 = build_direntry("baz", DirentKind::File);
        let chained2 = parent.chain(&child2);
        assert_eq!(&chained2.name, "foo/baz");
        assert_eq!(chained2.kind, DirentKind::File);
    }

    // read_file

    #[fasync::run_singlethreaded(test)]
    async fn test_read_file() {
        let contents = read_file(&open_pkg(), "/data/file").await.unwrap();
        assert_eq!(&contents, DATA_FILE_CONTENTS.as_bytes());
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_read_file_to_string() {
        let contents = read_file_to_string(&open_pkg(), "/data/file").await.unwrap();
        assert_eq!(contents, DATA_FILE_CONTENTS);
    }

    #[fasync::run_singlethreaded(test)]
    async fn test_read_missing_file() {
        let result = read_file(&open_pkg(), "/data/missing").await;
        assert_matches!(
            result,
            Err(ReadError::Open(OpenError::OpenError(zx_status::Status::NOT_FOUND)))
        );
        assert_matches!(result, Err(e) if e.is_not_found_error());
    }

    #[fasync::run_singlethreaded(test)]
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    async fn atomic_write_file_writes_to_file() {
        let (_tmp, proxy) = open_data();
        let contents = b"atomic write contents";
        match atomic_write_file(&proxy, "atomic-file", contents).await {
            Ok(_) => (),
            // Don't assume that the filesystem running on the system supports unnamed temp files.
            Err(WriteError::Open(OpenError::OpenError(zx::Status::NOT_SUPPORTED))) => return,
            Err(err) => panic!("atomic_write_file failed: {err:?}"),
        };

        let file_contents = read_file(&proxy, "atomic-file").await.unwrap();
        assert_eq!(file_contents, contents);
    }

    #[fasync::run_singlethreaded(test)]
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    async fn atomic_write_file_overwrites_existing_file() {
        let (_tmp, proxy) = open_data();
        let initial_contents = b"initial contents";
        match atomic_write_file(&proxy, "atomic-file", initial_contents).await {
            Ok(_) => (),
            // Don't assume that the filesystem running on the system supports unnamed temp files.
            Err(WriteError::Open(OpenError::OpenError(zx::Status::NOT_SUPPORTED))) => return,
            Err(err) => panic!("atomic_write_file failed: {err:?}"),
        };

        let new_contents = b"new contents";
        atomic_write_file(&proxy, "atomic-file", new_contents).await.unwrap();

        let file_contents = read_file(&proxy, "atomic-file").await.unwrap();
        assert_eq!(file_contents, new_contents);
    }

    #[fasync::run_singlethreaded(test)]
    #[cfg(fuchsia_api_level_at_least = "HEAD")]
    async fn atomic_write_file_nested() {
        let (_tmp, proxy) = open_data();
        let contents = b"atomic write contents";
        create_directory(&proxy, "dir", fio::PERM_READABLE | fio::PERM_WRITABLE).await.unwrap();
        match atomic_write_file(&proxy, "dir/atomic-file", contents).await {
            Ok(_) => (),
            // Don't assume that the filesystem running on the system supports unnamed temp files.
            Err(WriteError::Open(OpenError::OpenError(zx::Status::NOT_SUPPORTED))) => return,
            Err(err) => panic!("atomic_write_file failed: {err:?}"),
        };

        let file_contents = read_file(&proxy, "dir/atomic-file").await.unwrap();
        assert_eq!(file_contents, contents);
    }
}
