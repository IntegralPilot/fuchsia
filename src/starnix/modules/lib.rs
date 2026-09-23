// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Increase recursion limit because LTO causes overflow.
#![recursion_limit = "256"]

use starnix_core::bpf::fs::BpfFs;
use starnix_core::device::kobject::DeviceMetadata;
use starnix_core::device::mem::{DevRandom, mem_device_init};
use starnix_core::device::{DeviceMode, simple_device_ops};
use starnix_core::fs::devpts::{dev_pts_fs, tty_device_init};
use starnix_core::fs::devtmpfs::dev_tmp_fs;
use starnix_core::fs::fuchsia::{RemoteBundle, new_remote_fs, new_remote_vol};
use starnix_core::fs::sysfs::sys_fs;
use starnix_core::fs::tmpfs::tmp_fs;
use starnix_core::task::Kernel;
use starnix_core::vfs::fs_registry::FsRegistry;
use starnix_core::vfs::pipe::register_pipe_fs;
use starnix_modules_binderfs::BinderFs;
use starnix_modules_cgroupfs::{CgroupV1Fs, cgroup2_fs};
use starnix_modules_debugfs::debug_fs;
use starnix_modules_device_mapper::{create_device_mapper, device_mapper_init};
use starnix_modules_ext4::ExtFilesystem;
use starnix_modules_functionfs::FunctionFs;
use starnix_modules_fuse::{new_fuse_fs, new_fusectl_fs, open_fuse_device};
use starnix_modules_inotify::inotify::inotify_init;
use starnix_modules_loop::{create_loop_control_device, loop_device_init};
use starnix_modules_overlayfs::new_overlay_fs;
use starnix_modules_procfs::proc_fs;
use starnix_modules_pstore::pstore_fs;
use starnix_modules_selinuxfs::selinux_fs;
use starnix_modules_tracefs::trace_fs;
use starnix_modules_tun::DevTun;
use starnix_modules_zram::zram_device_init;
use starnix_uapi::device_id::DeviceId;
use starnix_uapi::errors::Errno;
use starnix_uapi::fs_type::FileSystemTypeFlags;

fn misc_device_init(kernel: &Kernel) -> Result<(), Errno> {
    let registry = &kernel.device_registry;
    let misc_class = registry.objects.misc_class();
    registry.register_device(
        kernel,
        // TODO(https://fxbug.dev/322365477) consider making this configurable
        "hw_random".into(),
        DeviceMetadata::new("hwrng".into(), DeviceId::HW_RANDOM, DeviceMode::Char),
        misc_class.clone(),
        simple_device_ops::<DevRandom>,
    )?;
    registry.register_device(
        kernel,
        "fuse".into(),
        DeviceMetadata::new("fuse".into(), DeviceId::FUSE, DeviceMode::Char),
        misc_class.clone(),
        open_fuse_device,
    )?;
    registry.register_device(
        kernel,
        "device-mapper".into(),
        DeviceMetadata::new("mapper/control".into(), DeviceId::DEVICE_MAPPER, DeviceMode::Char),
        misc_class.clone(),
        create_device_mapper,
    )?;
    registry.register_device(
        kernel,
        "loop-control".into(),
        DeviceMetadata::new("loop-control".into(), DeviceId::LOOP_CONTROL, DeviceMode::Char),
        misc_class.clone(),
        create_loop_control_device,
    )?;
    registry.register_device(
        kernel,
        "tun".into(),
        DeviceMetadata::new("tun".into(), DeviceId::TUN, DeviceMode::Char),
        misc_class,
        simple_device_ops::<DevTun>,
    )?;
    Ok(())
}

/// Initializes common devices in `Kernel`.
///
/// Adding device nodes to devtmpfs requires the current running task. The `Kernel` constructor does
/// not create an initial task, so this function should be triggered after a `CurrentTask` has been
/// initialized.
pub fn init_common_devices(kernel: &Kernel) -> Result<(), Errno> {
    misc_device_init(kernel)?;
    mem_device_init(kernel)?;
    tty_device_init(kernel)?;
    loop_device_init(kernel)?;
    device_mapper_init(kernel)?;
    zram_device_init(kernel)?;
    Ok(())
}

pub fn register_common_file_systems(kernel: &Kernel) {
    let registry = kernel.expando.get::<FsRegistry>();
    registry.register(b"binder".into(), FileSystemTypeFlags::empty(), BinderFs::new_fs);
    registry.register(b"bpf".into(), FileSystemTypeFlags::empty(), BpfFs::new_fs);
    registry.register(b"cgroup".into(), FileSystemTypeFlags::empty(), CgroupV1Fs::new_fs);
    registry.register(b"cgroup2".into(), FileSystemTypeFlags::empty(), cgroup2_fs);
    // Cpusets use the generic cgroup (v1) subsystem.
    // From https://docs.kernel.org/admin-guide/cgroup-v1/cpusets.html
    registry.register(b"cpuset".into(), FileSystemTypeFlags::empty(), CgroupV1Fs::new_fs_cpuset);
    registry.register(b"debugfs".into(), FileSystemTypeFlags::empty(), debug_fs);
    registry.register(b"devpts".into(), FileSystemTypeFlags::empty(), dev_pts_fs);
    registry.register(b"devtmpfs".into(), FileSystemTypeFlags::empty(), dev_tmp_fs);
    registry.register(b"erofs".into(), FileSystemTypeFlags::empty(), starnix_modules_erofs::new_fs);
    registry.register(b"ext4".into(), FileSystemTypeFlags::REQUIRES_DEV, ExtFilesystem::new_fs);
    registry.register(b"functionfs".into(), FileSystemTypeFlags::empty(), FunctionFs::new_fs);
    registry.register(b"fuse".into(), FileSystemTypeFlags::empty(), new_fuse_fs);
    registry.register(b"fusectl".into(), FileSystemTypeFlags::empty(), new_fusectl_fs);
    registry.register(b"overlay".into(), FileSystemTypeFlags::empty(), new_overlay_fs);
    register_pipe_fs(registry.as_ref());
    registry.register(b"proc".into(), FileSystemTypeFlags::empty(), proc_fs);
    registry.register(b"pstore".into(), FileSystemTypeFlags::empty(), pstore_fs);
    registry.register(b"remotefs".into(), FileSystemTypeFlags::empty(), new_remote_fs);
    registry.register(b"remotevol".into(), FileSystemTypeFlags::empty(), new_remote_vol);
    registry.register(b"remote_bundle".into(), FileSystemTypeFlags::empty(), RemoteBundle::new_fs);
    registry.register(b"selinuxfs".into(), FileSystemTypeFlags::empty(), selinux_fs);
    registry.register(b"sysfs".into(), FileSystemTypeFlags::empty(), sys_fs);
    registry.register(b"tmpfs".into(), FileSystemTypeFlags::empty(), tmp_fs);
    registry.register(b"tracefs".into(), FileSystemTypeFlags::empty(), trace_fs);
}

pub fn register_common_syscalls(kernel: &Kernel) {
    inotify_init(kernel);
}
