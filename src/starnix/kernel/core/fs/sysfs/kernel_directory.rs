// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use crate::task::Kernel;
use crate::vfs::pseudo::simple_directory::SimpleDirectoryMutator;
use crate::vfs::pseudo::simple_file::create_bytes_file_with_handler;
use crate::vfs::pseudo::stub_empty_file::StubEmptyFile;
use starnix_logging::bug_ref;
use starnix_uapi::file_mode::mode;

/// This directory contains various files and subdirectories that provide information about
/// the running kernel.
pub fn build_kernel_directory(kernel: &Kernel, dir: &SimpleDirectoryMutator) {
    let dir_mode = 0o755;
    dir.subdir("debug", dir_mode, |_| ());
    dir.subdir("tracing", dir_mode, |_| ());
    dir.subdir("wakeup_reasons", dir_mode, |dir| {
        let read_only_file_mode = mode!(IFREG, 0o444);
        dir.entry(
            "last_resume_reason",
            create_bytes_file_with_handler(kernel.weak_self.clone(), |kernel| {
                kernel.suspend_resume_manager.suspend_stats().last_resume_reason.unwrap_or_default()
            }),
            read_only_file_mode,
        );
        dir.entry(
            "last_suspend_time",
            create_bytes_file_with_handler(kernel.weak_self.clone(), |kernel| {
                let suspend_stats = kernel.suspend_resume_manager.suspend_stats();
                // First number is the time spent in suspend and resume processes.
                // Second number is the time spent in sleep state.
                format!(
                    "{} {}",
                    suspend_stats.last_time_in_suspend_operations.into_seconds_f64(),
                    suspend_stats.last_time_in_sleep.into_seconds_f64()
                )
            }),
            read_only_file_mode,
        );
    });

    dir.subdir("fs", dir_mode, |dir| {
        dir.subdir("cgroup", dir_mode, |dir| {
            dir.entry(
                "cgroup.subtree_control",
                StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                mode!(IFREG, 0o644),
            );
            for name in ["apps", "system"] {
                dir.subdir(name, dir_mode, |dir| {
                    dir.entry(
                        "cgroup.subtree_control",
                        StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                        mode!(IFREG, 0o644),
                    );
                });
            }
        });
    });
    dir.subdir("dmabuf", dir_mode, |dir| {
        // TODO(https://fxbug.dev/452096300"): Do not report any buffer for now.
        dir.subdir("buffers", dir_mode, |_| {});
    });
    dir.subdir("ion", dir_mode, |dir| {
        dir.entry(
            "total_heaps_kb",
            StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
            mode!(IFREG, 0o444),
        );
        dir.entry(
            "total_pools_kb",
            StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
            mode!(IFREG, 0o444),
        );
    });
    dir.subdir("mm", dir_mode, |dir| {
        dir.entry(
            "cma",
            StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
            mode!(IFREG, 0o444),
        );
        dir.subdir("lru_gen", dir_mode, |dir| {
            dir.entry(
                "enabled",
                StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                mode!(IFREG, 0o644),
            );
        });
        dir.subdir("pgsize_migration", dir_mode, |dir| {
            dir.entry(
                "enabled",
                StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/452096300")),
                mode!(IFREG, 0o644),
            );
        });
        dir.subdir("transparent_hugepage", dir_mode, |dir| {
            dir.entry(
                "enabled",
                StubEmptyFile::new_node(bug_ref!("https://fxbug.dev/322894184")),
                mode!(IFREG, 0o644),
            );
        });
    });
}
