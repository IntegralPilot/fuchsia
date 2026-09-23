// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Error;
use argh::FromArgs;
use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::Command;

/// Tool that run the aidl tool

/// Arguments for aidl_wrapper.
#[derive(Debug, FromArgs)]
pub struct AidlWrapperArgs {
    /// path to the aidl tool
    #[argh(option)]
    pub aidl_path: PathBuf,

    /// base paths for the library
    #[argh(option)]
    pub base: Vec<PathBuf>,

    /// directory into which the tool will generate the dependencies and package files
    #[argh(option)]
    dependency_dir: PathBuf,

    /// dependency dir of dependencies
    #[argh(option)]
    deps: Vec<PathBuf>,

    /// arguments of the aidl command
    #[argh(option)]
    pub args: Vec<String>,

    /// file containing the input of the aidl command
    #[argh(option)]
    pub inputs_path: PathBuf,

    /// the version of the interface and parcelable
    #[argh(option)]
    version: Option<String>,
}

fn read_file<T>(filename: PathBuf) -> Result<T, Error>
where
    T: FromIterator<String>,
{
    // Open the file in read-only mode.
    let file = File::open(filename)?;
    // Read the file line by line.
    Ok(BufReader::new(file).lines().collect::<Result<T, _>>()?)
}

fn read_path_bufs(filename: PathBuf) -> Result<BTreeSet<PathBuf>, Error> {
    Ok(read_file::<BTreeSet<String>>(filename)?.iter().map(PathBuf::from).collect())
}

fn write_file<'a, T>(filename: PathBuf, content: impl Iterator<Item = T>) -> Result<(), Error>
where
    String: From<T>,
{
    std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(filename)?
        .write_all(content.map(String::from).collect::<Vec<_>>().join("\n").as_bytes())?;
    Ok(())
}

const DEPS_FILE: &'static str = "deps.d";
const PACKAGE_FILE: &'static str = "aidl_package";
const BASES_FILE: &'static str = "aidl_bases";
const RUST_GLUE_ARGS: &'static str = "aidl_rust_glue_args";
const HASH_FILE: &'static str = ".hash";

fn main() -> Result<(), Error> {
    let opt: AidlWrapperArgs = argh::from_env();
    let mut packages = BTreeSet::<String>::new();
    let mut dependencies = BTreeSet::<PathBuf>::new();
    for base in &opt.base {
        dependencies.insert(base.clone());
    }
    let mut glue_args = vec![];
    for deps in opt.deps {
        for package in read_file::<BTreeSet<String>>(deps.join(PACKAGE_FILE))? {
            glue_args.push("-I".to_owned());
            glue_args.push(package.replace(".", "_"));
        }
        for base in read_file::<BTreeSet<String>>(deps.join(BASES_FILE))? {
            dependencies.insert(base.into());
        }
    }
    for path in read_path_bufs(opt.inputs_path)? {
        let mut hash = None;
        for base in &opt.base {
            if path.starts_with(base) {
                let package = pathdiff::diff_paths(&path, &base)
                    .unwrap()
                    .parent()
                    .unwrap()
                    .to_string_lossy()
                    .replace("/", ".");
                packages.insert(package);
                if opt.version.is_some() {
                    let mut hashes = read_file::<Vec<String>>(base.join(HASH_FILE))?;
                    hash = hashes.pop();
                }
            }
        }
        let mut command = Command::new(&opt.aidl_path);
        // Make aidl generate a dependencies file.
        command.arg("-a");
        for dep in &dependencies {
            command.arg("-I").arg(dep);
        }
        if let Some(version) = &opt.version {
            command.arg(format!("--version={version}"));
        }
        if let Some(hash) = hash {
            command.arg(format!("--hash={hash}"));
        }
        for arg in &opt.args {
            command.arg(arg);
        }
        command.arg(path);
        let output = command.output()?;
        if !output.status.success() {
            anyhow::bail!("aidl failed: {output:?}");
        }
    }

    // Find dependency files, aggregate it and delete the individual dependencies.
    let mut aggregated_dependency_file = vec![];
    for entry in walkdir::WalkDir::new(&opt.dependency_dir).into_iter() {
        let entry = entry?;
        let f_name = entry.file_name().to_string_lossy();

        if f_name.ends_with(".d") {
            let file = File::open(entry.path())?;
            aggregated_dependency_file
                .append(&mut BufReader::new(file).lines().collect::<Result<Vec<_>, _>>()?);
            std::fs::remove_file(entry.path())?;
        }
    }
    write_file(opt.dependency_dir.join(PACKAGE_FILE), packages.into_iter())?;
    write_file(opt.dependency_dir.join(RUST_GLUE_ARGS), glue_args.iter().map(|x| x.as_str()))?;
    write_file(opt.dependency_dir.join(DEPS_FILE), aggregated_dependency_file.into_iter())?;
    write_file(
        opt.dependency_dir.join(BASES_FILE),
        dependencies.iter().map(|x| x.to_string_lossy()),
    )?;
    Ok(())
}
