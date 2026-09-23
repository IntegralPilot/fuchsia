// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::error::FfxFastbootError;
use crate::file_resolver::FileResolver;
use crate::file_resolver::resolvers::{Resolver, TarResolver};
type Result<T> = std::result::Result<T, FfxFastbootError>;
use async_trait::async_trait;
use std::fs::{File, create_dir_all};
use std::io::copy;
use std::path::{Path, PathBuf};
use tempfile::{TempDir, tempdir};
use walkdir::WalkDir;
use zip::ZipArchive;

/// Trait that allows the given Resolvers to find out where their flash manifest paths are
#[async_trait]
pub trait ManifestResolver {
    async fn get_manifest_path(&self) -> &Path;
}

#[async_trait]
impl ManifestResolver for ArchiveResolver {
    async fn get_manifest_path(&self) -> &Path {
        self.manifest()
    }
}

/// Specific resolver that embeds the base Resolver type. Ensures the root path is a file
pub struct FlashManifestResolver(Resolver);

impl FlashManifestResolver {
    pub fn new(path: PathBuf) -> Result<Self> {
        match path.is_file() {
            false => {
                return Err(FfxFastbootError::ManifestNotAFile { path: path.clone() });
            }
            true => Ok(Self(Resolver::new(path)?)),
        }
    }
}

#[async_trait]
impl FileResolver for FlashManifestResolver {
    async fn get_file(&mut self, file: &str) -> Result<String> {
        self.0.get_file(file).await
    }
}

#[async_trait]
impl ManifestResolver for FlashManifestResolver {
    async fn get_manifest_path(&self) -> &Path {
        self.0.root_path()
    }
}

pub struct ArchiveResolver {
    temp_dir: TempDir,
    manifest_path: PathBuf,
    internal_manifest_path: PathBuf,
    archive: ZipArchive<File>,
}

impl ArchiveResolver {
    pub fn new(path: PathBuf) -> Result<Self> {
        let temp_dir = tempdir()?;
        let file = File::open(path.clone())
            .map_err(|e| FfxFastbootError::FileOpen { path: path.clone(), source: e })?;
        let mut archive = ZipArchive::new(file).map_err(FfxFastbootError::ZipArchiveOpen)?;
        let mut internal_manifest_path = None;
        let mut manifest_path = None;

        for i in 0..archive.len() {
            let mut archive_file = archive.by_index(i).map_err(FfxFastbootError::ZipArchiveRead)?;
            let outpath = archive_file.mangled_name();
            if (&*archive_file.name()).ends_with("flash.json")
                || (&*archive_file.name()).ends_with("flash-manifest.manifest")
            {
                let mut internal_path = PathBuf::new();
                internal_path.push(outpath.clone());
                internal_manifest_path.replace(internal_path.clone());
                let mut manifest = PathBuf::new();
                manifest.push(temp_dir.path());
                manifest.push(outpath);
                if let Some(p) = manifest.parent() {
                    if !p.exists() {
                        create_dir_all(&p)?;
                    }
                }
                let mut outfile = File::create(&manifest)?;
                copy(&mut archive_file, &mut outfile)?;
                manifest_path.replace(manifest);
                break;
            }
        }

        match (internal_manifest_path, manifest_path) {
            (Some(i), Some(m)) => {
                Ok(Self { temp_dir, manifest_path: m, internal_manifest_path: i, archive })
            }
            _ => Err(FfxFastbootError::FlashManifestNotFoundInArchive { path }),
        }
    }

    pub fn manifest(&self) -> &Path {
        self.manifest_path.as_path()
    }
}

#[async_trait]
impl FileResolver for ArchiveResolver {
    async fn get_file(&mut self, file: &str) -> Result<String> {
        let mut file = match self.internal_manifest_path.parent() {
            Some(p) => {
                let mut path = PathBuf::new();
                path.push(p);
                path.push(file);
                self.archive
                    .by_name(path.to_str().ok_or_else(|| FfxFastbootError::InvalidArchiveFileName)?)
                    .map_err(|_| FfxFastbootError::FileNotFoundInArchive {
                        file: file.to_string(),
                    })?
            }
            None => self
                .archive
                .by_name(file)
                .map_err(|_| FfxFastbootError::FileNotFoundInArchive { file: file.to_string() })?,
        };
        let mut outpath = PathBuf::new();
        outpath.push(self.temp_dir.path());
        outpath.push(file.mangled_name());
        if let Some(p) = outpath.parent() {
            if !p.exists() {
                create_dir_all(&p)?;
            }
        }
        let mut outfile = File::create(&outpath)?;
        copy(&mut file, &mut outfile)?;
        Ok(outpath.to_str().ok_or_else(|| FfxFastbootError::InvalidTempFileName)?.to_owned())
    }
}

pub struct FlashManifestTarResolver(TarResolver, PathBuf);

impl FlashManifestTarResolver {
    pub fn new(path: PathBuf) -> Result<Self> {
        let resolver_inner = TarResolver::new(path.clone())?;

        let manifest_path = WalkDir::new(resolver_inner.root_path())
            .into_iter()
            .filter_map(|e| e.ok())
            .find(|e| e.file_name() == "flash.json" || e.file_name() == "flash-manifest.manifest");

        match manifest_path {
            Some(m) => Ok(Self(resolver_inner, m.into_path())),
            _ => Err(FfxFastbootError::FlashManifestNotFoundInArchive { path }),
        }
    }

    pub fn manifest(&self) -> &Path {
        self.1.as_path()
    }
}

#[async_trait]
impl FileResolver for FlashManifestTarResolver {
    async fn get_file(&mut self, file: &str) -> Result<String> {
        self.0.get_file(file).await
    }
}

#[async_trait]
impl ManifestResolver for FlashManifestTarResolver {
    async fn get_manifest_path(&self) -> &Path {
        self.1.as_path()
    }
}
