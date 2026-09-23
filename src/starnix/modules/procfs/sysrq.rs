// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

//! See https://www.kernel.org/doc/html/latest/admin-guide/sysrq.html.

use fidl_fuchsia_hardware_power_statecontrol::{
    AdminMarker, ShutdownAction, ShutdownOptions, ShutdownReason,
};
use fuchsia_component::client::connect_to_protocol_sync;
use starnix_core::task::CurrentTask;
use starnix_core::vfs::{
    AppendLockWriteGuard, FileObject, FileOps, FsNode, FsNodeHandle, FsNodeOps, FsStr, InputBuffer,
    OutputBuffer, SeekTarget, fileops_impl_noop_sync,
};
use starnix_logging::{log_info, log_warn, track_stub};

use starnix_uapi::auth::FsCred;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::file_mode::FileMode;
use starnix_uapi::open_flags::OpenFlags;
use starnix_uapi::{error, off_t};

pub struct SysRqNode {}

impl SysRqNode {
    pub fn new() -> Self {
        Self {}
    }
}

impl FsNodeOps for SysRqNode {
    fn create_file_ops(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _flags: OpenFlags,
    ) -> Result<Box<dyn FileOps>, Errno> {
        Ok(Box::new(SysRqFile {}))
    }

    fn mknod(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        _mode: FileMode,
        _dev: DeviceId,
        _owner: FsCred,
    ) -> Result<FsNodeHandle, Errno> {
        error!(EINVAL)
    }

    fn mkdir(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        _mode: FileMode,
        _owner: FsCred,
    ) -> Result<FsNodeHandle, Errno> {
        error!(EINVAL)
    }

    fn create_symlink(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        _target: &FsStr,
        _owner: FsCred,
    ) -> Result<FsNodeHandle, Errno> {
        error!(EINVAL)
    }

    fn unlink(
        &self,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _name: &FsStr,
        _child: &FsNodeHandle,
    ) -> Result<(), Errno> {
        error!(EINVAL)
    }

    fn truncate(
        &self,
        _guard: &AppendLockWriteGuard<'_>,
        _node: &FsNode,
        _current_task: &CurrentTask,
        _length: u64,
    ) -> Result<(), Errno> {
        // This file doesn't store any contents, but userspace expects to truncate it on open.
        Ok(())
    }
}

pub struct SysRqFile {}

impl FileOps for SysRqFile {
    fileops_impl_noop_sync!();

    fn is_seekable(&self) -> bool {
        false
    }

    fn read(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _offset: usize,
        _data: &mut dyn OutputBuffer,
    ) -> Result<usize, Errno> {
        error!(EINVAL)
    }

    fn write(
        &self,
        _file: &FileObject,
        current_task: &CurrentTask,
        _offset: usize,
        data: &mut dyn InputBuffer,
    ) -> Result<usize, Errno> {
        let commands = data.read_all()?;
        for command in &commands {
            match *command {
                b'b' => {
                    log_warn!("SysRq reboot request.");

                    // Attempt to reboot the device.
                    // When this call succeeds with a production implementation it should never
                    // return. If it returns at all it is a sign the kernel either doesn't have the
                    // capability or there was a problem with the shutdown request.
                    let reboot_res = connect_to_protocol_sync::<AdminMarker>().unwrap().shutdown(
                        &ShutdownOptions {
                            action: Some(ShutdownAction::Reboot),
                            reasons: Some(vec![ShutdownReason::CriticalComponentFailure]),
                            ..Default::default()
                        },
                        zx::MonotonicInstant::INFINITE,
                    );

                    panic!(
                        "reboot call returned unexpectedly ({:?}), crashing from SysRq",
                        reboot_res
                    );
                }
                b'c' => {
                    // LINT.IfChange
                    panic!("SysRq kernel crash request",);
                    // LINT.ThenChange(/src/starnix/tests/sysrq/src/lib.rs)
                }
                b'd' => track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqDumpLocksHeld"),
                b'e' => {
                    track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqSigtermAllButInit")
                }
                b'f' => track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqOomKiller"),
                b'h' => track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqPrintHelp"),
                b'i' => {
                    track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqSigkillAllButInit")
                }
                b'j' => track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqJustThawIt"),
                b'k' => {
                    track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqSecureAccessKey")
                }
                b'l' => {
                    track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqBacktraceActiveCpus",)
                }
                b'm' => {
                    track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqDumpMemoryInfo")
                }
                b'n' => track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqRealtimeNice"),
                b'o' => {
                    log_info!("SysRq kernel shutdown request.");
                    current_task.kernel().shut_down();
                }
                b'p' => {
                    track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqDumpRegistersAndFlags",)
                }
                b'q' => track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqDumpHrTimers"),
                b'r' => {
                    track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqDisableKeyboardRawMode",)
                }
                b's' => {
                    track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqSyncMountedFilesystems",)
                }
                b't' => {
                    track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqDumpCurrentTasks")
                }
                b'u' => {
                    track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqRemountAllReadonly")
                }
                b'v' => {
                    track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqRestoreFramebuffer")
                }
                b'w' => {
                    track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqDumpBlockedTasks")
                }
                b'x' => {
                    track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqDumpFtraceBuffer")
                }
                b'0' => track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqLogLevel0"),
                b'1' => track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqLogLevel1"),
                b'2' => track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqLogLevel2"),
                b'3' => track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqLogLevel3"),
                b'4' => track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqLogLevel4"),
                b'5' => track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqLogLevel5"),
                b'6' => track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqLogLevel6"),
                b'7' => track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqLogLevel7"),
                b'8' => track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqLogLevel8"),
                b'9' => track_stub!(TODO("https://fxbug.dev/319745106"), "SysRqLogLevel9"),

                _ => return error!(EINVAL),
            }
        }
        Ok(commands.len())
    }

    fn seek(
        &self,
        _file: &FileObject,
        _current_task: &CurrentTask,
        _current_offset: off_t,
        _target: SeekTarget,
    ) -> Result<off_t, Errno> {
        error!(EINVAL)
    }
}
