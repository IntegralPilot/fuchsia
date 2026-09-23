// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO(https://fxbug.dev/42144933): Migrate to the new library that substitutes io_utils and fuchsia_fs::directory.
// Ask for host-side support on the new library (fxr/467217).

use anyhow::{Error, Result, anyhow, format_err};
use async_trait::async_trait;
use futures::lock::Mutex;
use std::path::{Path, PathBuf};
use std::{env, fs};
use zx_status::Status;

#[cfg(feature = "fdomain")]
use fuchsia_fs_fdomain as fuchsia_fs;

use flex_client::ProxyHasDomain;
use flex_fuchsia_io as fio;
use fuchsia_fs::directory::{DirEntry, open_directory_async, open_file_async, readdir};
use fuchsia_fs::file::{close, read, read_to_string, write};

pub enum DirentKind {
    File,
    Directory,
}

#[async_trait]
pub trait Directory: Sized {
    /// Attempts to open the directory at `relative_path` with read-only rights.
    fn open_dir_readonly<P: AsRef<Path> + Send>(&self, relative_path: P) -> Result<Self>;

    /// Attempts to open the directory at `relative_path` with read/write rights.
    fn open_dir_readwrite<P: AsRef<Path> + Send>(&self, relative_path: P) -> Result<Self>;

    /// Attempts to create a directory at `relative_path` with read right (if `readwrite` is false) or read/write rights (if `readwrite` is true).
    fn create_dir<P: AsRef<Path> + Send>(&self, relative_path: P, readwrite: bool) -> Result<Self>;

    /// Return a copy of self.
    fn clone(&self) -> Result<Self>;

    /// Returns the contents of the file at `relative_path` as a string.
    async fn read_file<P: AsRef<Path> + Send>(&self, relative_path: P) -> Result<String>;

    /// Returns the contents of the file at `relative_path` as bytes.
    async fn read_file_bytes<P: AsRef<Path> + Send>(&self, relative_path: P) -> Result<Vec<u8>>;

    /// Returns true if an entry called `filename` exists in this directory. `filename` must be
    /// a plain file name, not a relative path.
    async fn exists(&self, filename: &str) -> Result<bool>;

    /// Returns the type of entry specified by `filename`, or None if no entry by that name
    /// is found. `filename` must be a plan file name, not a relative path.
    async fn entry_type(&self, filename: &str) -> Result<Option<DirentKind>>;

    /// Deletes the file at `relative_path`.
    async fn remove(&self, relative_path: &str) -> Result<()>;

    /// Writes `data` to a file at `relative_path`. Overwrites if the file already
    /// exists.
    async fn write_file<P: AsRef<Path> + Send>(&self, relative_path: P, data: &[u8]) -> Result<()>;

    /// Returns the size of the file at `relative_path` in bytes.
    async fn get_file_size<P: AsRef<Path> + Send>(&self, relative_path: P) -> Result<u64>;

    /// Returns a list of directory entry names as strings.
    async fn entry_names(&self) -> Result<Vec<String>>;
}

/// A convenience wrapper over a local directory.
#[derive(Debug)]
pub struct LocalDirectory {
    path: PathBuf,
}

impl LocalDirectory {
    /// Returns a new local directory wrapper with no base path component. All operations will
    /// be on paths relative to the environment used by std::fs.
    pub fn new() -> Self {
        LocalDirectory { path: PathBuf::new() }
    }

    /// Returns a new local directory such that the methods on `Self` will
    /// operate as expected on `path`:
    ///
    ///     - if `path` is absolute, returns a directory at "/"
    ///     - if `path` is relative, returns a directory at `cwd`
    pub fn for_path(path: &PathBuf) -> Self {
        if path.is_absolute() {
            LocalDirectory { path: "/".into() }
        } else {
            LocalDirectory { path: env::current_dir().unwrap() }
        }
    }
}

#[async_trait]
impl Directory for LocalDirectory {
    fn open_dir_readonly<P: AsRef<Path> + Send>(&self, relative_path: P) -> Result<Self> {
        let full_path = self.path.join(relative_path);
        Ok(LocalDirectory { path: full_path })
    }

    fn open_dir_readwrite<P: AsRef<Path> + Send>(&self, relative_path: P) -> Result<Self> {
        self.open_dir_readonly(relative_path)
    }

    fn create_dir<P: AsRef<Path> + Send>(
        &self,
        relative_path: P,
        _readwrite: bool,
    ) -> Result<Self> {
        let full_path = self.path.join(relative_path);
        fs::create_dir(full_path.clone())?;
        Ok(LocalDirectory { path: full_path })
    }

    fn clone(&self) -> Result<Self> {
        Ok(LocalDirectory { path: self.path.clone() })
    }

    async fn read_file<P: AsRef<Path> + Send>(&self, relative_path: P) -> Result<String> {
        let full_path = self.path.join(relative_path);
        fs::read_to_string(full_path).map_err(Into::into)
    }

    async fn read_file_bytes<P: AsRef<Path> + Send>(&self, relative_path: P) -> Result<Vec<u8>> {
        let full_path = self.path.join(relative_path);
        fs::read(full_path).map_err(Into::into)
    }

    async fn exists(&self, filename: &str) -> Result<bool> {
        Ok(self.path.join(filename).exists())
    }

    async fn entry_type(&self, filename: &str) -> Result<Option<DirentKind>> {
        let full_path = self.path.join(filename);
        if !full_path.exists() {
            return Ok(None);
        }
        let metadata = fs::metadata(full_path)?;
        if metadata.is_file() {
            Ok(Some(DirentKind::File))
        } else if metadata.is_dir() {
            Ok(Some(DirentKind::Directory))
        } else {
            Err(anyhow!("Unsupported entry type: {:?}", metadata.file_type()))
        }
    }

    async fn remove(&self, relative_path: &str) -> Result<()> {
        let full_path = self.path.join(relative_path);
        fs::remove_file(full_path).map_err(Into::into)
    }

    async fn write_file<P: AsRef<Path> + Send>(&self, relative_path: P, data: &[u8]) -> Result<()> {
        let full_path = self.path.join(relative_path);
        fs::write(full_path, data).map_err(Into::into)
    }

    async fn get_file_size<P: AsRef<Path> + Send>(&self, relative_path: P) -> Result<u64> {
        let full_path = self.path.join(relative_path);
        let metadata = fs::metadata(full_path)?;
        if metadata.is_file() {
            Ok(metadata.len())
        } else {
            Err(anyhow!("Cannot get size of non-file"))
        }
    }

    async fn entry_names(&self) -> Result<Vec<String>> {
        let paths = fs::read_dir(self.path.clone())?;
        Ok(paths.into_iter().map(|p| p.unwrap().file_name().into_string().unwrap()).collect())
    }
}

/// A convenience wrapper over a FIDL DirectoryProxy.
#[derive(Debug)]
pub struct RemoteDirectory {
    path: PathBuf,
    proxy: fio::DirectoryProxy,
    // The `fuchsia.io.RemoteDirectory` protocol is stateful in readdir, and the associated `fuchsia_fs::directory`
    // library used for enumerating the directory has no mechanism for synchronization of readdir
    // operations, as such this mutex must be held throughout directory enumeration in order to
    // avoid race conditions from concurrent rewinds and reads.
    readdir_mutex: Mutex<()>,
}

impl RemoteDirectory {
    #[cfg(target_os = "fuchsia")]
    pub fn from_namespace<P: AsRef<Path> + Send>(path: P) -> Result<Self> {
        let path_str = path
            .as_ref()
            .as_os_str()
            .to_str()
            .ok_or_else(|| format_err!("could not convert path to string"))?;
        let proxy = fuchsia_fs::directory::open_in_namespace(path_str, fio::PERM_READABLE)?;
        let path = path.as_ref().to_path_buf();
        Ok(Self { path, proxy, readdir_mutex: Mutex::new(()) })
    }

    pub fn from_proxy(proxy: fio::DirectoryProxy) -> Self {
        let path = PathBuf::from(".");
        Self { path, proxy, readdir_mutex: Mutex::new(()) }
    }

    pub fn clone_proxy(&self) -> Result<fio::DirectoryProxy> {
        let (cloned_proxy, clone_server) =
            self.proxy.domain().create_proxy::<fio::DirectoryMarker>();
        self.proxy.clone(clone_server.into_channel().into())?;
        Ok(cloned_proxy)
    }

    async fn entries(&self) -> Result<Vec<DirEntry>, Error> {
        let _lock = self.readdir_mutex.lock().await;
        match readdir(&self.proxy).await {
            Ok(entries) => Ok(entries),
            Err(e) => Err(format_err!(
                "could not get entries of `{}`: {}",
                self.path.as_path().display(),
                e
            )),
        }
    }

    fn open_dir<P: AsRef<Path> + Send>(&self, relative_path: P, flags: fio::Flags) -> Result<Self> {
        let path = self.path.join(relative_path.as_ref());
        let relative_path = match relative_path.as_ref().to_str() {
            Some(relative_path) => relative_path,
            None => return Err(format_err!("could not convert relative path to &str")),
        };
        match open_directory_async(&self.proxy, relative_path, flags) {
            Ok(proxy) => Ok(Self { path, proxy, readdir_mutex: Mutex::new(()) }),
            Err(e) => Err(format_err!("could not open dir `{}`: {}", path.as_path().display(), e)),
        }
    }
}

#[async_trait]
impl Directory for RemoteDirectory {
    fn open_dir_readonly<P: AsRef<Path> + Send>(&self, relative_path: P) -> Result<Self> {
        self.open_dir(relative_path, fio::PERM_READABLE)
    }

    fn open_dir_readwrite<P: AsRef<Path> + Send>(&self, relative_path: P) -> Result<Self> {
        self.open_dir(relative_path, fio::PERM_READABLE | fio::PERM_WRITABLE)
    }

    fn create_dir<P: AsRef<Path> + Send>(&self, relative_path: P, readwrite: bool) -> Result<Self> {
        let mut flags = fio::Flags::FLAG_MAYBE_CREATE | fio::PERM_READABLE;
        if readwrite {
            flags = flags | fio::PERM_WRITABLE;
        }
        self.open_dir(relative_path, flags)
    }

    async fn read_file<P: AsRef<Path> + Send>(&self, relative_path: P) -> Result<String> {
        let path = self.path.join(relative_path.as_ref());
        let relative_path = match relative_path.as_ref().to_str() {
            Some(relative_path) => relative_path,
            None => return Err(format_err!("relative path is not valid unicode")),
        };

        let proxy = match open_file_async(&self.proxy, relative_path, fio::PERM_READABLE) {
            Ok(proxy) => proxy,
            Err(e) => {
                return Err(format_err!(
                    "could not open file `{}`: {}",
                    path.as_path().display(),
                    e
                ));
            }
        };

        match read_to_string(&proxy).await {
            Ok(data) => Ok(data),
            Err(e) => Err(format_err!(
                "could not read file `{}` as string: {}",
                path.as_path().display(),
                e
            )),
        }
    }

    async fn read_file_bytes<P: AsRef<Path> + Send>(&self, relative_path: P) -> Result<Vec<u8>> {
        let path = self.path.join(relative_path.as_ref());
        let relative_path = match relative_path.as_ref().to_str() {
            Some(relative_path) => relative_path,
            None => return Err(format_err!("relative path is not valid unicode")),
        };

        let proxy = match open_file_async(&self.proxy, relative_path, fio::PERM_READABLE) {
            Ok(proxy) => proxy,
            Err(e) => {
                return Err(format_err!(
                    "could not open file `{}`: {}",
                    path.as_path().display(),
                    e
                ));
            }
        };

        match read(&proxy).await {
            Ok(data) => Ok(data),
            Err(e) => Err(format_err!("could not read file `{}`: {}", path.as_path().display(), e)),
        }
    }

    async fn exists(&self, filename: &str) -> Result<bool> {
        let filename = PathBuf::from(filename);
        let mut path_iter = filename.iter().peekable();
        let path_element = path_iter.next().ok_or_else(|| format_err!("path is empty"))?;
        match path_iter.peek() {
            Some(_) => {
                let sub_directory = self.open_dir_readonly(path_element)?;
                let sub_path = path_iter.collect::<PathBuf>();
                let sub_path_str =
                    sub_path.to_str().ok_or_else(|| format_err!("path is not valid utf8"))?;
                sub_directory.exists(sub_path_str).await
            }
            None => match self.entry_names().await {
                Ok(entries) => Ok(entries.iter().any(|s| s.as_str() == path_element)),
                Err(e) => Err(format_err!(
                    "could not check if `{}` exists in `{}`: {}",
                    filename.display(),
                    self.path.as_path().display(),
                    e
                )),
            },
        }
    }

    async fn entry_type(&self, filename: &str) -> Result<Option<DirentKind>> {
        let entries = self.entries().await?;

        entries
            .into_iter()
            .find(|e| e.name == filename)
            .map(|e| {
                match e.kind {
                    // TODO(https://fxbug.dev/42077929): Update component_manager vfs to assign proper DirentType when installing the directory tree.
                    fio::DirentType::Directory | fio::DirentType::Unknown => {
                        Ok(Some(DirentKind::Directory))
                    }
                    fio::DirentType::File => Ok(Some(DirentKind::File)),
                    _ => {
                        return Err(anyhow!(
                            "Unsupported entry type for file {}: {:?}",
                            filename,
                            e.kind,
                        ));
                    }
                }
            })
            .unwrap_or(Ok(None))
    }

    async fn remove(&self, filename: &str) -> Result<()> {
        let filename = PathBuf::from(filename);
        let mut path_iter = filename.iter().peekable();
        let path_element = path_iter.next().ok_or_else(|| {
            format_err!(
                "could not delete `{}` from `{}: path is empty",
                filename.display(),
                self.path.as_path().display(),
            )
        })?;
        match path_iter.peek() {
            Some(_) => {
                let sub_directory = self.open_dir_readwrite(path_element)?;
                let sub_path = path_iter.collect::<PathBuf>();
                let sub_path_str = sub_path.to_str().ok_or_else(|| {
                    format_err!(
                        "could not delete `{}` from `{}: path is not valid utf8",
                        filename.display(),
                        self.path.as_path().display(),
                    )
                })?;
                sub_directory.remove(sub_path_str).await
            }
            None => {
                let options = fio::UnlinkOptions::default();
                let path_str =
                    path_element.to_str().ok_or_else(|| format_err!("path is not valid utf8"))?;
                match self.proxy.unlink(path_str, &options).await {
                    Ok(r) => match r {
                        Ok(()) => Ok(()),
                        Err(e) => Err(format_err!(
                            "could not delete `{}` from `{}`: {}",
                            path_element.display(),
                            self.path.as_path().display(),
                            e
                        )),
                    },
                    Err(e) => Err(format_err!(
                        "proxy error while deleting `{}` from `{}`: {}",
                        path_element.display(),
                        self.path.as_path().display(),
                        e
                    )),
                }
            }
        }
    }

    async fn write_file<P: AsRef<Path> + Send>(&self, relative_path: P, data: &[u8]) -> Result<()> {
        let path = self.path.join(relative_path.as_ref());
        let relative_path = match relative_path.as_ref().to_str() {
            Some(relative_path) => relative_path,
            None => return Err(format_err!("relative path is not valid unicode")),
        };

        let file = match open_file_async(
            &self.proxy,
            relative_path,
            fio::PERM_WRITABLE | fio::Flags::FLAG_MAYBE_CREATE,
        ) {
            Ok(proxy) => proxy,
            Err(e) => {
                return Err(format_err!(
                    "could not open file `{}`: {}",
                    path.as_path().display(),
                    e
                ));
            }
        };

        let () = file
            .resize(0)
            .await
            .map_err(|e| {
                format_err!("could not truncate file `{}`: {}", path.as_path().display(), e)
            })?
            .map_err(Status::err_from_raw)
            .map_err(|status| {
                format_err!("could not truncate file `{}`: {}", path.as_path().display(), status)
            })?;

        match write(&file, data).await {
            Ok(()) => {}
            Err(e) => {
                return Err(format_err!(
                    "could not write to file `{}`: {}",
                    path.as_path().display(),
                    e
                ));
            }
        }

        match close(file).await {
            Ok(()) => Ok(()),
            Err(e) => {
                Err(format_err!("could not close file `{}`: {}", path.as_path().display(), e))
            }
        }
    }

    async fn get_file_size<P: AsRef<Path> + Send>(&self, relative_path: P) -> Result<u64> {
        let path = self.path.join(relative_path.as_ref());
        let relative_path = match relative_path.as_ref().to_str() {
            Some(relative_path) => relative_path,
            None => return Err(format_err!("relative path is not valid unicode")),
        };

        let file = match open_file_async(&self.proxy, relative_path, fio::PERM_READABLE) {
            Ok(proxy) => proxy,
            Err(e) => {
                return Err(format_err!(
                    "could not open file `{}`: {}",
                    path.as_path().display(),
                    e
                ));
            }
        };
        let (_, immutable_attributes) = file
            .get_attributes(fio::NodeAttributesQuery::STORAGE_SIZE)
            .await
            .map_err(|e| {
                format_err!("Unexpected FIDL error during file attribute retrieval: {}", e)
            })?
            .unwrap();
        Ok(immutable_attributes.storage_size.unwrap_or_default())
    }

    async fn entry_names(&self) -> Result<Vec<String>> {
        match self.entries().await {
            Ok(entries) => Ok(entries.into_iter().map(|e| e.name).collect()),
            Err(e) => Err(format_err!(
                "could not get entry names of `{}`: {}",
                self.path.as_path().display(),
                e
            )),
        }
    }

    fn clone(&self) -> Result<Self> {
        let proxy = open_directory_async(&self.proxy, ".", fio::PERM_READABLE)?;
        Ok(Self { path: self.path.clone(), proxy, readdir_mutex: Mutex::new(()) })
    }
}
