// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Measures the amount of time used to save and restore extended processor state such as floating
// point and vector registers and floating point control state. There are two types of operations
// measured:
//
//   - Reset             Measures the time taken to reset the in-memory copy of the state
//   - SaveAndRestore/*  Measure the time taken to save and immediately restore state using a
//                       particular strategy.
//
// x86_64 processors may support multiple instructions to save and restore state. The instructions
// tested in this benchmark are:
//   - XSaveOpt XSAVEOPT + XRSTOR
//   - XSave    XSAVE + XRSTOR
//   - FXSave   FXSAVE + FXRSTOR
//
// The benchmark exercises all instructions available on the currently running hardware and so may
// not test all instructions on a particular device.
//
// aarch64 processors provide only one mechanism for saving and restoring state.

use criterion::Criterion;
use extended_pstate::{ExtendedPstateState, restore_extended_pstate, save_extended_pstate};
use fuchsia as _;
use fuchsia_criterion::FuchsiaCriterion;
use std::mem;
use std::time::Duration;

#[cfg(target_arch = "x86_64")]
use extended_pstate::x86_64::{PREFERRED_STRATEGY, Strategy};

#[cfg(target_arch = "aarch64")]
use extended_pstate::{restore_extended_aarch32_pstate, save_extended_aarch32_pstate};

fn main() {
    let mut fc = FuchsiaCriterion::default();
    let c: &mut Criterion = &mut fc;
    *c = mem::take(c)
        .warm_up_time(Duration::from_millis(1))
        .measurement_time(Duration::from_millis(10))
        .sample_size(100);

    let mut group = c.benchmark_group("fuchsia.extended_pstate");

    let _ = group.bench_function("Reset", |b| {
        let mut state = ExtendedPstateState::default();
        b.iter(|| {
            state.reset();
        })
    });

    #[cfg(target_arch = "x86_64")]
    let bench_strategy =
        |group: &mut criterion::BenchmarkGroup<'_, criterion::measurement::WallTime>, strategy| {
            if *PREFERRED_STRATEGY <= strategy {
                let _ = group.bench_function(format!("SaveAndRestore/{:?}", strategy), move |b| {
                    use extended_pstate::ExtendedPstatePointer;

                    let mut state = ExtendedPstateState::with_strategy(strategy);
                    let mut pstate_ptr = ExtendedPstatePointer { extended_pstate: &raw mut state };
                    let ptr_ptr = &raw mut pstate_ptr as usize;
                    #[allow(clippy::undocumented_unsafe_blocks)]
                    b.iter(|| unsafe {
                        save_extended_pstate(ptr_ptr);
                        restore_extended_pstate(ptr_ptr);
                    });
                });
            }
        };

    #[cfg(target_arch = "x86_64")]
    {
        bench_strategy(&mut group, Strategy::XSaveOpt);
        bench_strategy(&mut group, Strategy::XSave);
        bench_strategy(&mut group, Strategy::FXSave);
    }
    #[cfg(target_arch = "aarch64")]
    {
        let _ = group.bench_function("SaveAndRestore/Aarch64", |b| {
            use extended_pstate::ExtendedPstatePointer;
            let mut state = ExtendedPstateState::default();
            let mut pstate_ptr = ExtendedPstatePointer { extended_pstate: &raw mut state };
            let ptr_ptr = &raw mut pstate_ptr as usize;
            #[allow(clippy::undocumented_unsafe_blocks)]
            b.iter(|| unsafe {
                save_extended_pstate(ptr_ptr);
                restore_extended_pstate(ptr_ptr);
            });
        });
        let _ = group.bench_function("SaveAndRestore/Aarch32", |b| {
            use extended_pstate::{ExtendedAarch32PstateState, ExtendedPstatePointer};
            let mut state = ExtendedAarch32PstateState::default();
            let mut pstate_ptr = ExtendedPstatePointer { extended_aarch32_pstate: &raw mut state };
            let ptr_ptr = &raw mut pstate_ptr as usize;
            #[allow(clippy::undocumented_unsafe_blocks)]
            b.iter(|| unsafe {
                save_extended_aarch32_pstate(ptr_ptr);
                restore_extended_aarch32_pstate(ptr_ptr);
            });
        });
    }

    group.finish();
}
