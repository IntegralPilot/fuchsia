// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use std::fs::{File, create_dir_all};
use std::io::{Error, copy};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::time::Instant;
use std::{env, fs};
use tempfile::TempDir;
use zip::ZipArchive;

const PYTHON_RUNTIME_NAME: &str = "python3";
const LACEWING_TEST_NAME: &str = "test.pyz";

fn unpack_artifacts(current_exe: &Path, dir: &TempDir) -> Result<(), Error> {
    println!("[HermeticWrapper] Extracting Lacewing artifacts");
    let t_before_unpack = Instant::now();

    // The current executable file is assumed to have a ZIP file appended to it.
    let current_exe_file = File::open(current_exe)?;

    // Extract archive contents.
    let mut zip = ZipArchive::new(&current_exe_file).expect("Unable to read archive.");
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).expect("Unable to get file by index from archive.");
        let outpath = file.mangled_name();
        let dest = dir.path().join(outpath);
        // Ensure the paths we extract do not escape the temp dir.
        // mangled_name() already removes `..`, but we want to explicitly ensure that this behavior does not regress.
        assert_eq!(dest.starts_with(dir), true);
        if file.is_dir() {
            create_dir_all(&dest)?;
        } else {
            let mut outfile = File::create(&dest).expect("Unable to extract file.");
            copy(&mut file, &mut outfile)?;
        }
    }

    fs::set_permissions(dir.path().join(PYTHON_RUNTIME_NAME), fs::Permissions::from_mode(0o555))?;

    // TODO(https://fxbug.dev/346856480): Report metrics to performance backend.
    let unpack_duration = t_before_unpack.elapsed();
    println!("[HermeticWrapper] Time spent preparing environment: {:?}", unpack_duration);
    Ok(())
}

fn main() -> Result<(), Error> {
    // Create temp dir to unpack data resources
    let tempdir = TempDir::new().expect("failed to create tmp dir");

    let current_exe = env::current_exe().expect("failed to obtain current executable path");

    unpack_artifacts(&current_exe, &tempdir)?;

    // Execute the Lacewing test in an external process.
    let mut command = Command::new(tempdir.path().join(PYTHON_RUNTIME_NAME));
    command
        .current_dir(tempdir.path()) // Fuchsia Controller import hooks require its shared libs to be present in CWD.
        .arg(tempdir.path().join(LACEWING_TEST_NAME))
        .env("PYTHONPATH", tempdir.path()) // Ensure ambient Python libraries are not used.
        .env("PYTHONUNBUFFERED", "1"); // Set line-buffering for Mobly tests to flush output immediately.

    for test_arg in env::args().skip(1) {
        // Plumb Mobly test args from the caller through (e.g. ["-c", "config.yaml", "-v"])
        command.arg(test_arg);
    }

    // [`CommandExt::exec`] doesn't return if successful, the current process is
    // replaced, so this just always returns an error if anything at all.
    println!("[HermeticWrapper] Executing Python: {:?}", command);
    Err(command.exec())
}
