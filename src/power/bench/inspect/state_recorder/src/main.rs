// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use anyhow::Result;
use fuchsia_criterion::FuchsiaCriterion;
use fuchsia_criterion::criterion::{self, Criterion};
use fuchsia_inspect::Inspector;
use fuchsia_inspect::hierarchy::DiagnosticsHierarchyGetter;
use state_recorder::{NumericStateRecorder, RecorderOptions, StateRecorderManager, units};

fn bench_populate_lazy(b: &mut criterion::Bencher<'_>, num_entries: u32) {
    let inspector = Inspector::default();
    let manager = StateRecorderManager::new(&inspector);
    let options = RecorderOptions {
        lazy_record: true,
        capacity: num_entries as usize,
        manager: Some(manager),
        persistence: None,
    };

    let mut recorder = NumericStateRecorder::<u32>::new(
        "bench_populate_lazy".to_string(),
        c"power_bench",
        units!(Percent),
        None,
        options,
    )
    .unwrap();

    b.iter(|| {
        for i in 0..num_entries {
            recorder.record(i);
        }
    });
}

fn bench_read_lazy(b: &mut criterion::Bencher<'_>, num_entries: u32) {
    let mut executor = fuchsia_async::LocalExecutor::default();
    let inspector = Inspector::default();
    let manager = StateRecorderManager::new(&inspector);
    let options = RecorderOptions {
        lazy_record: true,
        capacity: num_entries as usize,
        manager: Some(manager),
        persistence: None,
    };

    let mut recorder = NumericStateRecorder::<u32>::new(
        "bench_read_lazy".to_string(),
        c"power_bench",
        units!(Percent),
        None,
        options,
    )
    .unwrap();

    // Populate the buffer up to its target scale before measuring reads
    for i in 0..num_entries {
        recorder.record(i);
    }

    b.iter(|| {
        let _hierarchy = executor.run_singlethreaded(inspector.get_diagnostics_hierarchy());
    });
}

fn main() -> Result<()> {
    let mut c = FuchsiaCriterion::default();
    let internal_c: &mut Criterion = &mut c;
    *internal_c = std::mem::take(internal_c)
        .warm_up_time(std::time::Duration::from_millis(10))
        .measurement_time(std::time::Duration::from_millis(100))
        .sample_size(20);

    let mut group = c.benchmark_group("fuchsia.power.state_recorder");
    for entries in [100, 200, 400, 800, 1600] {
        let _ = group.bench_function(format!("PopulateLazy/{entries}"), move |b| {
            bench_populate_lazy(b, entries)
        });

        let _ = group
            .bench_function(format!("ReadLazy/{entries}"), move |b| bench_read_lazy(b, entries));
    }
    group.finish();

    Ok(())
}
