// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#![recursion_limit = "1024"]

use criterion::Criterion;
use fuchsia_criterion::FuchsiaCriterion;
use security::PermissionFlags;
use selinux::policy::{AccessVector, KernelAccessDecision};
use selinux::{AccessQueryArgs, ConcurrentAccessCache, FileClass, KernelClass, SecurityId};
use starnix_core::security;
use starnix_core::task::CurrentTask;
use starnix_core::testing::{PanickingFile, spawn_kernel_with_selinux_and_run};
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

const POLICY_BYTES: &[u8] =
    include_bytes!("../../../../lib/selinux/testdata/policies/aosp_sepolicy");

async fn spawn_test_kernel<F>(callback: F)
where
    F: AsyncFnOnce(&mut CurrentTask) + Send + Sync + 'static,
{
    spawn_kernel_with_selinux_and_run(async |current_task, security_server| {
        security_server.load_policy(POLICY_BYTES.to_vec()).unwrap();
        callback(current_task).await
    })
    .await;
}

fn create_file_bench(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    name: &'static str,
    current_task: &'static CurrentTask,
    hook_closure: impl Fn(&CurrentTask, &starnix_core::vfs::FileObject) + Send + Sync + 'static,
) {
    let file = Arc::new(PanickingFile::new_file(current_task));

    let _ = group.bench_function(name, move |bench| {
        let file = file.clone();
        bench.iter(|| {
            hook_closure(current_task, &file);
        })
    });
}

fn load_policy_bench(group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>) {
    let _ = group.bench_function("load_policy", move |b| {
        b.iter(|| {
            let server = selinux::SecurityServer::new_default();
            let _ = std::hint::black_box(server.load_policy(POLICY_BYTES.to_vec()));
        })
    });
}

fn security_context_to_sid_bench(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    name_suffix: &'static str,
    context_bytes: &'static [u8],
) {
    let server = selinux::SecurityServer::new_default();
    let _ = server.load_policy(POLICY_BYTES.to_vec()).unwrap();

    let server_clone = server.clone();
    let _ = group.bench_function(format!("security_context_to_sid_{}", name_suffix), move |b| {
        b.iter(|| {
            let _ = std::hint::black_box(
                server_clone.security_context_to_sid(context_bytes.into()).unwrap(),
            );
        })
    });
}

fn sid_to_security_context_bench(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    name_suffix: &'static str,
    context_bytes: &'static [u8],
) {
    let server = selinux::SecurityServer::new_default();
    let _ = server.load_policy(POLICY_BYTES.to_vec()).unwrap();
    let sid = server.security_context_to_sid(context_bytes.into()).unwrap();

    let server_clone = server.clone();
    let _ = group.bench_function(format!("sid_to_security_context_{}", name_suffix), move |b| {
        b.iter(|| {
            let _ = std::hint::black_box(server_clone.sid_to_security_context(sid).unwrap());
        })
    });
}

fn compute_access_decision_bench(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    name_suffix: &'static str,
    context_bytes: &'static [u8],
) {
    let server = selinux::SecurityServer::new_default();
    let _ = server.load_policy(POLICY_BYTES.to_vec()).unwrap();
    let sid = server.security_context_to_sid(context_bytes.into()).unwrap();
    let class_id = server.class_id_by_name("process").unwrap();

    let server_clone = server.clone();
    let _ = group.bench_function(format!("compute_access_decision_{}", name_suffix), move |b| {
        b.iter(|| {
            let _ =
                std::hint::black_box(server_clone.compute_access_decision_raw(sid, sid, class_id));
        })
    });
}

fn compute_create_sid_bench(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    name_suffix: &'static str,
    source_context: &'static [u8],
    target_context: &'static [u8],
) {
    let server = selinux::SecurityServer::new_default();
    let _ = server.load_policy(POLICY_BYTES.to_vec()).unwrap();
    let source_sid = server.security_context_to_sid(source_context.into()).unwrap();
    let target_sid = server.security_context_to_sid(target_context.into()).unwrap();
    let class_id = server.class_id_by_name("process").unwrap();

    let server_clone = server.clone();
    let _ = group.bench_function(format!("compute_create_sid_{}", name_suffix), move |b| {
        b.iter(|| {
            let _ = std::hint::black_box(server_clone.compute_create_sid_raw(
                source_sid,
                target_sid,
                class_id,
                &[],
            ));
        })
    });
}

fn cached_create_sid_filename_bench(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    bench_name: &'static str,
    source_context: &'static [u8],
    target_context: &'static [u8],
    filename: &'static [u8],
) {
    let server = selinux::SecurityServer::new_default();
    let _ = server.load_policy(POLICY_BYTES.to_vec()).unwrap();
    let source_sid = server.security_context_to_sid(source_context.into()).unwrap();
    let target_sid = server.security_context_to_sid(target_context.into()).unwrap();

    let server_clone = server.clone();
    let local_cache = Default::default();
    let permission_check = server_clone.as_permission_check(&local_cache);
    let _ = group.bench_function(format!("cached_create_sid_filename_{}", bench_name), move |b| {
        b.iter(|| {
            let _ = std::hint::black_box(
                permission_check
                    .compute_create_sid(source_sid, target_sid, FileClass::File.into(), filename)
                    .unwrap(),
            );
        })
    });
}

fn compute_create_sid_filename_bench(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    bench_name: &'static str,
    source_context: &'static [u8],
    target_context: &'static [u8],
    filename: &'static [u8],
) {
    let server = selinux::SecurityServer::new_default();
    let _ = server.load_policy(POLICY_BYTES.to_vec()).unwrap();
    let source_sid = server.security_context_to_sid(source_context.into()).unwrap();
    let target_sid = server.security_context_to_sid(target_context.into()).unwrap();
    let class_id = server.class_id_by_name("file").unwrap();

    let server_clone = server.clone();
    let _ = group.bench_function(format!("compute_create_sid_filename_{}", bench_name), move |b| {
        b.iter(|| {
            let _ = std::hint::black_box(
                server_clone
                    .compute_create_sid_raw(source_sid, target_sid, class_id, filename)
                    .unwrap(),
            );
        })
    });
}

fn concurrent_access_cache_get_bench(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
) {
    let cache = ConcurrentAccessCache::new(selinux::DEFAULT_SHARED_SIZE.access_cache_capacity);
    let value = KernelAccessDecision {
        allow: AccessVector::ALL,
        audit: AccessVector::NONE,
        flags: 0,
        todo_bug: None,
    };

    let keys: Vec<_> = (1..=1000)
        .map(|i| AccessQueryArgs {
            source_sid: SecurityId(NonZeroU32::new(i).unwrap()),
            target_sid: SecurityId(NonZeroU32::new(i + 1).unwrap()),
            target_class: KernelClass::Process,
        })
        .collect();

    for key in &keys {
        let _ = cache.get_or_try_insert::<()>(key, || Ok(value));
    }

    let _ = group.bench_function("concurrent_access_cache_get", move |b| {
        b.iter(|| {
            for key in &keys {
                let _ = std::hint::black_box(cache.get_or_try_insert::<()>(key, || Ok(value)));
            }
        })
    });
}

fn file_permission_bench(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    current_task: &'static CurrentTask,
) {
    create_file_bench(group, "file_permission", current_task, |task, file| {
        let _ = std::hint::black_box(
            security::file_permission(task, file, PermissionFlags::READ).unwrap(),
        );
    });
}

fn fs_node_permission_bench(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    current_task: &'static CurrentTask,
) {
    create_file_bench(group, "fs_node_permission", current_task, |task, file| {
        let _ = std::hint::black_box(
            security::fs_node_permission(
                task,
                file.node(),
                PermissionFlags::READ,
                security::Auditable::None,
            )
            .unwrap(),
        );
    });
}

fn check_file_ioctl_access_bench(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    current_task: &'static CurrentTask,
) {
    create_file_bench(group, "check_file_ioctl_access", current_task, |task, file| {
        let _ = std::hint::black_box(
            security::check_file_ioctl_access(task, file, starnix_uapi::TCGETS).unwrap(),
        );
    });
}

fn binder_transaction_bench(
    group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>,
    current_task: &'static CurrentTask,
) {
    let _ = group.bench_function("binder_transaction", move |b| {
        let connection_state = Arc::new(security::binder_connection_alloc(current_task));
        b.iter(|| {
            let _ = std::hint::black_box(
                security::binder_transaction(current_task, current_task, &connection_state)
                    .unwrap(),
            );
        })
    });
}

fn main() {
    let mut executor = fuchsia_async::LocalExecutor::default();
    executor.run_singlethreaded(spawn_test_kernel(async move |current_task| {
        // SAFETY: The Criterion benchmarks run synchronously and block until completion
        // entirely within the scope of this closure. Therefore, `current_task` is guaranteed
        // to outlive the benchmark execution, making it safe to cast to a `'static` reference.
        let current_task = unsafe { &*(current_task as *const CurrentTask) };

        // List of benchmark programs is passed as the argument list from the
        // component manifest. The arguments passed by the test executor are
        // separated from the arguments in the manifest file by adding "--" at
        // the end of the argument list in the manifest file.
        let mut args: Vec<_> = std::env::args().collect();
        let Some(separator_pos) = args.iter().position(|s| s == "--") else {
            eprintln!("{:?}\n-- not found in the argument list", args);
            std::process::exit(1);
        };

        // Replace separator with the program name.
        args[separator_pos] = args[0].clone();

        let benchmark_args: Vec<_> = args[separator_pos..].iter().map(|s| &**s).collect();

        let mut fc = FuchsiaCriterion::fuchsia_bench_with_args(&benchmark_args);
        let c: &mut Criterion = &mut fc;

        *c = std::mem::take(c)
            .warm_up_time(Duration::from_millis(100))
            .measurement_time(Duration::from_secs(1))
            .sample_size(50);

        let mut group = c.benchmark_group("fuchsia.sestarnix");
        load_policy_bench(&mut group);
        security_context_to_sid_bench(&mut group, "simple", b"u:r:kernel:s0");
        security_context_to_sid_bench(&mut group, "c0_c255", b"u:r:kernel:s0:c0.c255");
        sid_to_security_context_bench(&mut group, "simple", b"u:r:kernel:s0");
        sid_to_security_context_bench(&mut group, "c0_c255", b"u:r:kernel:s0:c0.c255");
        compute_access_decision_bench(&mut group, "simple", b"u:r:kernel:s0");
        compute_access_decision_bench(&mut group, "c0_c255", b"u:r:kernel:s0:c0.c255");
        compute_create_sid_bench(&mut group, "simple", b"u:r:kernel:s0", b"u:r:kernel:s0");
        compute_create_sid_bench(
            &mut group,
            "c0_c255",
            b"u:r:kernel:s0:c0.c255",
            b"u:r:kernel:s0:c0.c255",
        );
        let filename_transition_cases: &[(&str, &[u8], &[u8], &[u8])] = &[
            ("match", b"u:r:init:s0", b"u:object_r:tmpfs:s0", b"shm"),
            ("no_match", b"u:r:init:s0", b"u:object_r:tmpfs:s0", b"unlikely_filename_1234"),
            ("no_target_rules", b"u:r:init:s0", b"u:r:kernel:s0", b"unlikely_filename_1234"),
            ("nameless", b"u:r:init:s0", b"u:object_r:tmpfs:s0", b""),
        ];
        for &(bench_name, source, target, filename) in filename_transition_cases {
            compute_create_sid_filename_bench(&mut group, bench_name, source, target, filename);
            cached_create_sid_filename_bench(&mut group, bench_name, source, target, filename);
        }
        concurrent_access_cache_get_bench(&mut group);
        file_permission_bench(&mut group, current_task);
        fs_node_permission_bench(&mut group, current_task);
        check_file_ioctl_access_bench(&mut group, current_task);
        binder_transaction_bench(&mut group, current_task);
        group.finish();
    }));
}
