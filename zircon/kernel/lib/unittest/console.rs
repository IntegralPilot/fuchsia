// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

use core::ffi::{c_int, c_void};
use core::slice;

use debug::kernel_oops;
use kalloc::Box;
use kprint::{kprint, kprintln};
use unittest::{TestCaseRegistration, TestSuiteRegistration};

use crate::console_rust::console::{CMD_AVAIL_NORMAL, CmdArgs, static_command};
use crate::kernel::mp::get_online_mask;
use crate::kernel::scheduler::peek_active_mask;
use crate::kernel::thread;
use crate::platform_rs::timer::{InstantMono, current_mono_time};
use crate::vm::vm_aspace::{Type as AspaceType, VmAspace};

ksync::declare_singleton_mutex!(UnittestLock);

unsafe extern "C" {
    static __start_unittest_testcases: TestSuiteRegistration;
    static __stop_unittest_testcases: TestSuiteRegistration;
}

fn get_testcases() -> &'static [TestSuiteRegistration] {
    let start = &raw const __start_unittest_testcases;
    let stop = &raw const __stop_unittest_testcases;
    // Safety: both symbols bound the same linker-defined section.
    let count = unsafe { stop.offset_from(start) };
    debug_assert!(count >= 0);
    // Safety: the section holds `count` contiguous registrations.
    unsafe { slice::from_raw_parts(start, count as usize) }
}

/// Returns the length of the longest of the given names, used as the column
/// width when printing them.
fn max_namelen<'a>(names: impl Iterator<Item = &'a str>) -> usize {
    names.map(str::len).max().unwrap_or(0)
}

fn print_padded(s: &str, width: usize) {
    kprint!("{:s}", s);
    if s.len() < width {
        for _ in 0..(width - s.len()) {
            kprint!(" ");
        }
    }
}

fn usage(progname: &str) {
    kprintln!(
        concat!(
            "Usage:\n",
            "{:s} <case>\n",
            "  where case is a specific testcase name, or...\n",
            "  all : run all tests\n",
            "  ?   : list tests\n",
            "  [-r num]  : repeat a test case num times",
        ),
        progname
    );
}

fn list_cases() {
    let testcases = get_testcases();

    let name_width = max_namelen(testcases.iter().map(TestSuiteRegistration::name));

    let count = testcases.len();
    let is_are = if count == 1 { "is" } else { "are" };
    let plurality = if count == 1 { "" } else { "s" };
    kprintln!("There {:s} {} test case{:s} available...", is_are, count, plurality);

    for testcase in testcases {
        kprint!("  ");
        print_padded(testcase.name(), name_width);
        kprintln!(" : {:s}", testcase.desc().unwrap_or("<no description>"));
    }
}

fn run_unittest(testcase: &'static TestSuiteRegistration, repeat: usize) -> bool {
    let tests = testcase.cases();

    let name_width = max_namelen(tests.iter().map(TestCaseRegistration::name));

    let case_name = testcase.name();
    let plurality = if testcase.test_cnt == 1 { "" } else { "s" };
    kprintln!("{:s} : Running {} test{:s}...", case_name, testcase.test_cnt, plurality);

    let testcase_start = current_mono_time();

    let mut passed = 0;
    for j in 0..repeat {
        for test in tests {
            let test_name = test.name();
            kprint!("  ");
            print_padded(test_name, name_width);
            kprint!(" : ");

            let online_mask_before = get_online_mask();
            let active_mask_before = peek_active_mask();

            let test_start = current_mono_time();
            let test_fn = test.fn_;
            let mut good = test_fn();
            let test_runtime = current_mono_time().0 - test_start.0;

            let online_mask_after = get_online_mask();
            let active_mask_after = peek_active_mask();
            if online_mask_before != online_mask_after || active_mask_before != active_mask_after {
                kernel_oops!(
                    "Online/active CPUs changed during test!\n(online after={:#010x} before={:#010x}, active after={:#010x} before={:#010x})\n",
                    online_mask_after,
                    online_mask_before,
                    active_mask_after,
                    active_mask_before
                );
                good = false;
            }

            kprint!("{:s} ({} nSec) ", if good { "PASSED" } else { "FAILED" }, test_runtime);
            if repeat > 1 {
                kprint!(" [{} / {}]", j + 1, repeat);
            }
            kprintln!("");

            if good {
                if j == repeat - 1 {
                    passed += 1;
                }
            } else {
                break;
            }
        }
    }

    let testcase_runtime = current_mono_time().0 - testcase_start.0;
    kprintln!(
        "{:s} : {:s}ll tests passed ({}/{}) in {} nSec",
        case_name,
        if passed != testcase.test_cnt { "Not a" } else { "A" },
        passed,
        testcase.test_cnt,
        testcase_runtime
    );

    passed == testcase.test_cnt
}

struct ThreadContext {
    testcase: &'static TestSuiteRegistration,
    repeat: usize,
}

extern "C" fn run_unittest_thread_entry(arg: *mut c_void) -> i32 {
    // SAFETY: `arg` is the `ThreadContext` supplied at thread creation.
    let ctx = unsafe { &*(arg as *const ThreadContext) };
    run_unittest(ctx.testcase, ctx.repeat) as i32
}

fn run_testcase_in_thread(testcase: &'static TestSuiteRegistration, repeat: usize) -> bool {
    let aspace = match VmAspace::create(AspaceType::User, c"unittest") {
        Some(a) => a,
        None => {
            kprintln!("failed to create unittest user aspace");
            return false;
        }
    };
    let _destroy_aspace = zr::defer(|| {
        let destroy_status = aspace.destroy();
        debug_assert!(destroy_status.is_ok());
    });

    let ctx = ThreadContext { testcase, repeat };
    // Safety: `ctx` outlives the thread, as every path below joins before
    // returning.
    let thread = match unsafe {
        thread::create(
            c"unittest".as_ptr(),
            run_unittest_thread_entry,
            &ctx as *const ThreadContext as *mut c_void,
        )
    } {
        Ok(t) => t,
        Err(_) => {
            kprintln!("failed to create unittest thread");
            return false;
        }
    };

    aspace.attach_to_thread(thread);
    unsafe { thread.resume() };

    match unsafe { thread.join(InstantMono::INFINITE) } {
        Ok(ret) => ret != 0,
        Err(status) => {
            kprintln!("failed to join unittest thread: {}", status.into_raw());
            false
        }
    }
}

unsafe extern "C" fn run_unittests(argc: c_int, argv: *const CmdArgs, _flags: u32) -> c_int {
    // Ensures unittests are not run concurrently.
    ksync::lock!(let _guard = UnittestLock::Get().lock());

    // SAFETY: the console passes `argc` arguments.
    let args = unsafe { slice::from_raw_parts(argv, argc as usize) };

    if argc < 2 {
        usage(args[0].as_str());
        return 0;
    }

    let mut casename = args[1].as_str();
    if casename == "?" {
        list_cases();
        return 0;
    }

    let mut repeat = 1;
    if casename == "-r" {
        if argc < 4 {
            usage(args[0].as_str());
            return 0;
        }
        repeat = args[2].arg_uint as usize;
        casename = args[3].as_str();
    }

    let run_all = casename == "all";
    let testcases = get_testcases();
    let num_tests = if run_all { testcases.len() } else { 1 };
    let mut failed: Box<[usize]> = match Box::try_new_zeroed_slice(num_tests) {
        Ok(b) => b,
        Err(_) => {
            kprintln!("failed to allocate memory for test results");
            return -1;
        }
    };

    let mut chosen = 0;
    let mut passed = 0;
    let mut failed_count = 0;
    for (i, testcase) in testcases.iter().enumerate() {
        if !run_all && casename != testcase.name() {
            continue;
        }
        chosen += 1;

        let status = run_testcase_in_thread(testcase, repeat);
        kprintln!("");
        if status {
            passed += 1;
        } else {
            failed[failed_count] = i;
            failed_count += 1;
        }

        if !run_all {
            break;
        }
    }

    let mut ret = 0;
    if !run_all && chosen == 0 {
        ret = -1;
        kprintln!("Test case \"{:s}\" not found!", casename);
        list_cases();
    } else {
        let s = if chosen == 1 { "" } else { "s" };
        kprintln!("SUMMARY: Ran {} test case{:s}: {} failed", chosen, s, chosen - passed);
        if passed < chosen {
            ret = -1;
            kprintln!("\nThe following test cases failed:");
            for i in 0..failed_count {
                kprintln!("{:s}", testcases[failed[i]].name());
            }
        }
    }

    ret
}

static_command!(CMD_UT, c"ut".as_ptr(), c"Run unittests".as_ptr(), run_unittests, CMD_AVAIL_NORMAL);
