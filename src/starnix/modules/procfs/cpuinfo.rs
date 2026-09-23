// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use fuchsia_component::client::connect_to_protocol_sync;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::FsNodeOps;
use starnix_core::vfs::pseudo::dynamic_file::{DynamicFile, DynamicFileBuf, DynamicFileSource};
use starnix_logging::log_error;
use starnix_uapi::errors::Errno;
use starnix_uapi::user_address::ArchSpecific as _;
use std::sync::LazyLock;

#[derive(Clone)]
pub struct CpuinfoFile {}
impl CpuinfoFile {
    pub fn new_node() -> impl FsNodeOps {
        DynamicFile::new_node(Self {})
    }
}

impl DynamicFileSource for CpuinfoFile {
    fn generate(&self, current_task: &CurrentTask, sink: &mut DynamicFileBuf) -> Result<(), Errno> {
        let is_qemu = SYSINFO.is_qemu();

        for i in 0..zx::system_get_num_cpus() {
            writeln!(sink, "processor\t: {}", i)?;

            // Report emulated CPU as "QEMU Virtual CPU". Some LTP tests rely on this to detect
            // that they running in a VM.
            if is_qemu {
                writeln!(sink, "model name\t: QEMU Virtual CPU")?;
            }
            if current_task.is_arch32() {
                #[cfg(target_arch = "aarch64")]
                {
                    arm32_write_features(sink, current_task.kernel().hwcaps.arch32)?;
                }
                #[cfg(not(target_arch = "aarch64"))]
                {
                    unreachable!("32-bit programs are only supported on ARM.")
                }
            }
            writeln!(sink)?;
        }
        Ok(())
    }
}

#[cfg(target_arch = "aarch64")]
fn arm32_write_features(
    sink: &mut DynamicFileBuf,
    hwcap: starnix_core::task::HwCap,
) -> Result<(), Errno> {
    write!(sink, "Features\t:")?;
    const HWCAP_STRINGS: [&str; 28] = [
        "swp",
        "half",
        "thumb",
        "26bit",
        "fastmult",
        "fpa",
        "vfp",
        "edsp",
        "java",
        "iwmmxt",
        "crunch",
        "thumbee",
        "neon",
        "vfpv3",
        "vfpv3d16",
        "tls",
        "vfpv4",
        "idiva",
        "idivt",
        "vfpd32",
        "lpae",
        "evtstrm",
        "fphp",
        "asimdhp",
        "asimddp",
        "asimdfhm",
        "asimdbf16",
        "i8mm",
    ];
    const HWCAP2_STRINGS: [&str; 7] = ["aes", "pmull", "sha1", "sha2", "crc32", "sb", "ssbs"];

    for i in 0..HWCAP_STRINGS.len() {
        if hwcap.hwcap & (1 << i) != 0 {
            write!(sink, " {}", HWCAP_STRINGS[i])?;
        }
    }
    for i in 0..HWCAP2_STRINGS.len() {
        if hwcap.hwcap2 & (1 << i) != 0 {
            write!(sink, " {}", HWCAP2_STRINGS[i])?;
        }
    }
    writeln!(sink)?;
    Ok(())
}

struct SysInfo {
    board_name: String,
}

impl SysInfo {
    fn is_qemu(&self) -> bool {
        matches!(
            self.board_name.as_str(),
            "Standard PC (Q35 + ICH9, 2009)" | "qemu-arm64" | "qemu-riscv64"
        )
    }

    fn fetch() -> Result<SysInfo, anyhow::Error> {
        let sysinfo = connect_to_protocol_sync::<fidl_fuchsia_sysinfo::SysInfoMarker>()?;
        let board_name = match sysinfo.get_board_name(zx::MonotonicInstant::INFINITE)? {
            (zx::sys::ZX_OK, Some(name)) => name,
            (_, _) => "Unknown".to_string(),
        };
        Ok(SysInfo { board_name })
    }
}

const SYSINFO: LazyLock<SysInfo> = LazyLock::new(|| {
    SysInfo::fetch().unwrap_or_else(|e| {
        log_error!("Failed to fetch sysinfo: {e}");
        SysInfo { board_name: "Unknown".to_string() }
    })
});
