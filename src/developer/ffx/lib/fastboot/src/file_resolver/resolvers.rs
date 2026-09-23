// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::FfxFastbootError;
use crate::file_resolver::FileResolver;
type Result<T> = std::result::Result<T, FfxFastbootError>;

use async_trait::async_trait;
use flate2::read::GzDecoder;
use std::fs::{File, create_dir_all};
use std::io::copy;
use std::path::{Path, PathBuf};
use tar::Archive;
use tempfile::{TempDir, tempdir};
use zip::read::ZipArchive;

/// EmptyResolver resolves paths directly from the host environment / CWD.
///
/// This resolver is only used for direct command-line arguments (such as
/// `ffx target bootloader boot --zbi <path>` and `unlock --cred <path>`)
/// where the user explicitly supplies file paths from the shell, and is not
/// used for parsing manifests or external archives.
pub struct EmptyResolver {
    fake: PathBuf,
}

impl EmptyResolver {
    pub fn new() -> Result<Self> {
        let mut fake = std::env::current_dir()?;
        fake.push("fake");
        Ok(Self { fake })
    }

    pub fn manifest(&self) -> &Path {
        self.fake.as_path() //should never get used
    }
}

#[async_trait]
impl FileResolver for EmptyResolver {
    async fn get_file(&mut self, file: &str) -> Result<String> {
        if PathBuf::from(file).is_absolute() {
            Ok(file.to_string())
        } else {
            let mut parent = std::env::current_dir()?;
            parent.push(file);
            if let Some(f) = parent.to_str() {
                Ok(f.to_string())
            } else {
                return Err(FfxFastbootError::NonUtf8Path);
            }
        }
    }
}

pub struct Resolver {
    root_path: PathBuf,
    base_dir: PathBuf,
}

impl Resolver {
    pub fn new(path: PathBuf) -> Result<Self> {
        let root_path = path
            .canonicalize()
            .map_err(|e| FfxFastbootError::CanonicalizePath { path: path.clone(), source: e })?;
        let base_dir = if root_path.is_dir() {
            root_path.clone()
        } else if let Some(parent) = root_path.parent() {
            parent.to_path_buf()
        } else {
            return Err(FfxFastbootError::NoParentDirectory);
        };
        Ok(Self { root_path, base_dir })
    }

    pub fn root_path(&self) -> &Path {
        self.root_path.as_path()
    }
}

#[async_trait]
impl FileResolver for Resolver {
    async fn get_file(&mut self, file: &str) -> Result<String> {
        let path = Path::new(file);
        let target = if path.is_absolute() { path.to_path_buf() } else { self.base_dir.join(path) };
        let canonical = target
            .canonicalize()
            .map_err(|e| FfxFastbootError::CanonicalizePath { path: target.clone(), source: e })?;
        if !canonical.starts_with(&self.base_dir) {
            return Err(FfxFastbootError::PathOutsideDirectory {
                path: canonical,
                root: self.base_dir.clone(),
            });
        }
        canonical.to_str().map(|s| s.to_string()).ok_or(FfxFastbootError::NonUtf8Path)
    }
}

#[derive(Debug)]
pub struct ZipArchiveResolver {
    temp_dir: TempDir,
    archive: ZipArchive<File>,
}

impl ZipArchiveResolver {
    pub fn new(path: PathBuf) -> Result<Self> {
        let temp_dir = tempdir()?;
        let file = File::open(path.clone())
            .map_err(|e| FfxFastbootError::FileOpen { path: path.clone(), source: e })?;
        let archive = ZipArchive::new(file).map_err(FfxFastbootError::ZipArchiveOpen)?;

        Ok(Self { temp_dir, archive })
    }
}

#[async_trait]
impl FileResolver for ZipArchiveResolver {
    async fn get_file(&mut self, file: &str) -> Result<String> {
        let mut file = self
            .archive
            .by_name(file)
            .map_err(|_| FfxFastbootError::FileNotFoundInArchive { file: file.to_string() })?;

        let mut outpath = PathBuf::new();
        outpath.push(self.temp_dir.path());
        outpath.push(file.mangled_name());
        if let Some(p) = outpath.parent() {
            if !p.exists() {
                create_dir_all(&p)?;
            }
        }
        log::debug!("Extracting to {}", self.temp_dir.path().display());
        let mut outfile = File::create(&outpath)?;
        copy(&mut file, &mut outfile)?;
        Ok(outpath.to_str().ok_or_else(|| FfxFastbootError::InvalidTempFileName)?.to_owned())
    }
}

pub struct TarResolver {
    temp_dir: TempDir,
    canonical_root: PathBuf,
}

impl TarResolver {
    pub fn new(path: PathBuf) -> Result<Self> {
        let temp_dir = tempdir()?;
        let file = File::open(path.clone())
            .map_err(|e| FfxFastbootError::FileOpen { path: path.clone(), source: e })?;
        log::debug!("Extracting to {}", temp_dir.path().display());
        let is_tar_gz = path.to_string_lossy().ends_with(".tar.gz")
            || path.extension().and_then(|e| e.to_str()) == Some("tgz");
        let is_tar = path.extension().and_then(|e| e.to_str()) == Some("tar");

        if is_tar_gz {
            let mut archive = Archive::new(GzDecoder::new(file));
            archive.unpack(temp_dir.path())?;
        } else if is_tar {
            let mut archive = Archive::new(file);
            archive.unpack(temp_dir.path())?;
        } else {
            return Err(FfxFastbootError::InvalidTarArchive);
        }

        let canonical_root = temp_dir.path().canonicalize().map_err(|e| {
            FfxFastbootError::CanonicalizePath { path: temp_dir.path().to_path_buf(), source: e }
        })?;

        Ok(Self { temp_dir, canonical_root })
    }

    pub fn root_path(&self) -> &Path {
        self.temp_dir.path()
    }
}

#[async_trait]
impl FileResolver for TarResolver {
    async fn get_file(&mut self, file: &str) -> Result<String> {
        let path = Path::new(file);
        if path.is_absolute() {
            return Err(FfxFastbootError::PathOutsideDirectory {
                path: path.to_path_buf(),
                root: self.canonical_root.clone(),
            });
        }
        let target = self.canonical_root.join(path);
        let canonical = target
            .canonicalize()
            .map_err(|e| FfxFastbootError::CanonicalizePath { path: target.clone(), source: e })?;
        if !canonical.starts_with(&self.canonical_root) {
            return Err(FfxFastbootError::PathOutsideDirectory {
                path: canonical,
                root: self.canonical_root.clone(),
            });
        }
        canonical.to_str().map(|s| s.to_string()).ok_or(FfxFastbootError::NonUtf8Path)
    }
}

////////////////////////////////////////////////////////////////////////////////
// tests

#[cfg(test)]
mod test {
    use super::*;
    type Result<T> = std::result::Result<T, anyhow::Error>;
    use std::io::{Read, Write};
    use std::str::FromStr;
    use zip::CompressionMethod;
    use zip::write::{SimpleFileOptions as FileOptions, ZipWriter};

    ////////////////////////////////////////////////////////////////////////////////
    // EmptyResolver

    #[fuchsia::test]
    async fn empty_resolver_resolves_cli_paths() -> Result<()> {
        let tmpdir = tempdir()?;
        let file_path = tmpdir.path().join("my_cred.bin");
        std::fs::write(&file_path, "cred_data")?;

        let mut resolver = EmptyResolver::new()?;

        let resolved = resolver.get_file(file_path.to_str().unwrap()).await?;
        assert_eq!(resolved, file_path.to_str().unwrap());

        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    // Resolver

    #[fuchsia::test]
    async fn resolver_manifest_file() -> Result<()> {
        let tmpdir = tempdir()?;
        let parent_dir = tmpdir.path().canonicalize()?;
        let manifest_dir = parent_dir.join("manifest_dir");
        std::fs::create_dir_all(&manifest_dir)?;
        let manifest_path = manifest_dir.join("flash.json");
        std::fs::write(&manifest_path, "{}")?;
        let image_path = manifest_dir.join("zircon_a.img");
        std::fs::write(&image_path, "zircon_data")?;

        let mut resolver = Resolver::new(manifest_path)?;

        // Relative path inside directory
        let file_path = resolver.get_file("zircon_a.img").await?;
        assert_eq!(file_path, image_path.to_str().unwrap());

        // Absolute path inside directory
        let file_path_abs = resolver.get_file(image_path.to_str().unwrap()).await?;
        assert_eq!(file_path_abs, image_path.to_str().unwrap());

        // Create an outside file in parent_dir so it exists on disk
        let outside_file = parent_dir.join("outside.img");
        std::fs::write(&outside_file, "outside")?;

        // Outside file (absolute)
        assert!(resolver.get_file(outside_file.to_str().unwrap()).await.is_err());

        // Outside file (relative traversal that resolves to an existing file outside base)
        assert!(resolver.get_file("../outside.img").await.is_err());

        Ok(())
    }

    #[fuchsia::test]
    async fn resolver_directory() -> Result<()> {
        let tmpdir = tempdir()?;
        let parent_dir = tmpdir.path().canonicalize()?;
        let pb_dir = parent_dir.join("pb");
        let image_dir = pb_dir.join("system_a");
        std::fs::create_dir_all(&image_dir)?;
        let image_path = image_dir.join("fuchsia.zbi");
        std::fs::write(&image_path, "zbi_data")?;

        let mut resolver = Resolver::new(pb_dir)?;

        // Relative path inside directory
        let file_path = resolver.get_file("system_a/fuchsia.zbi").await?;
        assert_eq!(file_path, image_path.to_str().unwrap());

        // Absolute path inside directory
        let file_path_abs = resolver.get_file(image_path.to_str().unwrap()).await?;
        assert_eq!(file_path_abs, image_path.to_str().unwrap());

        // Create an outside file in parent_dir so it exists on disk
        let outside_file = parent_dir.join("outside.img");
        std::fs::write(&outside_file, "outside")?;

        // Outside file (absolute)
        assert!(resolver.get_file(outside_file.to_str().unwrap()).await.is_err());

        // Outside file (relative traversal)
        assert!(resolver.get_file("../outside.img").await.is_err());

        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    // TarResolver

    #[fuchsia::test]
    async fn tar_resolver_get_file() -> Result<()> {
        let tmpdir = tempdir()?;
        let tar_path = tmpdir.path().join("test.tar.gz");
        let file = File::create(&tar_path)?;
        let enc = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut tar = tar::Builder::new(enc);

        let mut header = tar::Header::new_gnu();
        header.set_path("hello.txt")?;
        header.set_size(13);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append(&header, "Hello, World!".as_bytes())?;
        let enc = tar.into_inner()?;
        enc.finish()?;

        let mut resolver = TarResolver::new(tar_path)?;
        let file_path = resolver.get_file("hello.txt").await?;
        let content = std::fs::read_to_string(file_path)?;
        assert_eq!(content, "Hello, World!");

        // Create an outside file in tmpdir so that relative traversal points to an existing file
        let outside_file = tmpdir.path().join("outside.txt");
        std::fs::write(&outside_file, "outside")?;

        // Absolute path should be rejected
        assert!(resolver.get_file("/etc/shadow").await.is_err());

        // Path traversal should be rejected even when target exists
        assert!(resolver.get_file("../../outside.txt").await.is_err());

        Ok(())
    }

    #[test]
    fn tar_resolver_invalid_extension() -> Result<()> {
        let tmpdir = tempdir()?;
        let gz_path = tmpdir.path().join("test.gz");
        File::create(&gz_path)?;
        assert!(TarResolver::new(gz_path).is_err());
        Ok(())
    }

    ////////////////////////////////////////////////////////////////////////////////
    // ZipArchiveResolver

    #[test]
    fn zip_archive_resolver_new_errors() -> Result<()> {
        let non_existant_path = PathBuf::from_str("./not-exists.zip")?;
        assert!(ZipArchiveResolver::new(non_existant_path).is_err());
        Ok(())
    }

    #[fuchsia::test]
    async fn zip_archive_resolver_get_file() -> Result<()> {
        // Make a temporary zip file
        let tmpdir = tempdir()?;

        let mut pbuff = PathBuf::new();
        pbuff.push(tmpdir.path());
        pbuff.push("test_zip.zip");

        let file = File::create(pbuff.as_path())?;

        // We use a buffer here, though you'd normally use a `File`
        let mut zip = ZipWriter::new(file);
        let options = FileOptions::default().compression_method(CompressionMethod::Stored);

        zip.start_file("hello_world.txt", options)?;
        let _ = zip.write(b"Hello, World!")?;
        zip.start_file("foo/hello_world.txt", options)?;
        let _ = zip.write(b"Hello, nested World!")?;

        zip.flush()?;
        zip.finish()?;

        let mut resolver = ZipArchiveResolver::new(pbuff)?;

        // Test standard file
        {
            let file_path = resolver.get_file("hello_world.txt").await?;
            let mut hello_file = File::open(file_path)?;
            let mut hello_buf = vec![];
            hello_file.read_to_end(&mut hello_buf)?;
            assert_eq!(hello_buf, b"Hello, World!");
        }

        // Test nested file
        {
            let file_path = resolver.get_file("foo/hello_world.txt").await?;
            let mut hello_file = File::open(file_path)?;
            let mut hello_buf = vec![];
            hello_file.read_to_end(&mut hello_buf)?;
            assert_eq!(hello_buf, b"Hello, nested World!");
        }

        // Test standard file with leading slash
        {
            assert!(resolver.get_file("/hello_world.txt").await.is_err());
        }

        // Test non-existent file
        {
            assert!(resolver.get_file("this-shouldnt-exist.txt").await.is_err());
        }

        Ok(())
    }
}
