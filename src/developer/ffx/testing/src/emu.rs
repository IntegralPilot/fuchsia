// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::anyhow;
use make_fuchsia_vol::make_empty_disk_with_uefi;
use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use tempfile::TempDir;
use tokio::net::UnixStream;

pub struct Emu {
    dir: TempDir,
    child: Child,
}

impl Emu {
    pub fn product_bundle_dir() -> &'static Path {
        static PRODUCT_BUNDLE_DIR: std::sync::LazyLock<PathBuf> =
            std::sync::LazyLock::new(|| crate::ROOT_BUILD_DIR.join(env!("PRODUCT_BUNDLE")));
        &*PRODUCT_BUNDLE_DIR
    }

    pub fn check_is_running(&mut self) -> anyhow::Result<()> {
        match self.child.try_wait() {
            Ok(Some(status)) => Err(anyhow!("Emulator exited with status: {}", status)),
            Ok(None) => Ok(()),
            Err(e) => Err(anyhow!("Error getting emulator running satus: {}", e)),
        }
    }

    /// Starts an emulator. Panics if the TestContext disallows emulator usage.
    pub fn start(ctx: &crate::TestContext) -> Emu {
        if !ctx.emulator_allowed() {
            panic!(
                "Attempted to start an emulator, but the passed Test Context disallows emulators"
            );
        }

        let emu_dir = TempDir::new_in(&*crate::TEMP_DIR).expect("could not create emu temp dir");

        let esp_blk_path = crate::ROOT_BUILD_DIR.join(env!("BOOTLOADER"));
        let esp_blk = std::fs::read(esp_blk_path.clone()).unwrap_or_else(|_| {
            panic!("failed to read bootloader from: {}", esp_blk_path.display())
        });
        let disk_path = emu_dir.path().join("disk.img");
        make_empty_disk_with_uefi(&disk_path, &esp_blk).expect("failed to make empty disk");

        static RUN_ZIRCON_PATH: std::sync::LazyLock<PathBuf> =
            std::sync::LazyLock::new(|| crate::ROOT_BUILD_DIR.join(env!("RUN_ZIRCON")));

        static QEMU_PATH: std::sync::LazyLock<PathBuf> = std::sync::LazyLock::new(|| {
            crate::ROOT_BUILD_DIR.join(concat!(env!("QEMU_PATH"), "/bin"))
        });

        let emu_serial = emu_dir.path().join("serial");
        let emu_serial_log = ctx.isolate().log_dir().join("emulator.serial.log");

        // run-zircon -a x64 -N --uefi --disktype=nvme -D <disk> -S <serial> -q <QEMU path>

        let mut command = Command::new(&*RUN_ZIRCON_PATH);

        command
            .args(["-a", "x64", "-N", "--uefi", "--disktype=nvme", "-M", "null"])
            .arg("-D")
            .arg(&disk_path)
            .arg("-S")
            .arg({
                let mut arg = OsString::from("unix:");
                arg.push(&emu_serial);
                arg.push(",server,nowait,logfile=");
                arg.push(&emu_serial_log);
                arg
            })
            .arg("-q")
            .arg(&*QEMU_PATH)
            // QEMU rewrites the system TMPDIR from /tmp to /var/tmp.
            // Instead use the directory the emulator is running in.
            // https://gitlab.com/qemu-project/qemu/-/issues/1626
            .env("TMPDIR", emu_dir.path());

        let mut emu = Emu { dir: emu_dir, child: command.spawn().unwrap() };

        emu.wait_for_spawn();

        emu
    }

    pub fn nodename(&self) -> &str {
        "fuchsia-5254-0063-5e7a"
    }

    fn serial_path(&self) -> PathBuf {
        self.dir.path().join("serial")
    }

    // Wait for the emulator serial socket to appear, while making sure the emulator process hasn't
    // exited abruptly.
    fn wait_for_spawn(&mut self) {
        use std::time::{Duration, Instant};

        const LOG_INTERVAL: Duration = Duration::from_secs(5);
        const TIMEOUT: Duration = Duration::from_secs(30);
        const BACKOFF: Duration = Duration::from_millis(500);

        let begun_at = Instant::now();
        let mut last_logged_at = begun_at;
        loop {
            let exists = self
                .serial_path()
                .try_exists()
                .expect("could not check existence of emulator serial");

            // Check if the emulator is still running
            let status = self.child.try_wait().expect("could not check emulator process status");
            assert!(
                status.is_none(),
                "emulator process exited while waiting for it to spawn, exit code {status:?}"
            );

            if !exists {
                if begun_at.elapsed() > TIMEOUT {
                    panic!("timed out waiting for emulator to spawn")
                }

                if last_logged_at.elapsed() > LOG_INTERVAL {
                    eprintln!("still waiting for emulator to spawn...");
                    last_logged_at = last_logged_at + LOG_INTERVAL;
                }

                std::thread::sleep(BACKOFF);
                continue;
            }

            return;
        }
    }

    pub async fn serial(&self) -> UnixStream {
        UnixStream::connect(self.serial_path()).await.expect("failed to connect to emulator socket")
    }
}

impl Drop for Emu {
    fn drop(&mut self) {
        while let None = self.child.try_wait().unwrap() {
            match self.child.kill() {
                Ok(()) => continue,
                Err(err) if err.kind() == ErrorKind::InvalidInput => continue,
                res @ _ => res.expect("could not kill qemu"),
            };
        }
        // TempDir's Drop impl deletes the directory, so it needs to live as long as Emu.
        // This silences unused variable warnings.
        let _ = self.dir;
    }
}
