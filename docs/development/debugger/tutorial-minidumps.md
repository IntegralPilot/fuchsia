# Tutorial: Debug minidumps using zxdb

A minidump is a memory dump of a process at the time of a crash, which can be
useful to debug the cause of a crash.

This tutorial walks through a debugging workflow that loads a minidump file into
zxdb and then walks through debugging steps to understand the failure captured
by the minidump.

## Capturing a minidump file {:#capturing-minidump .numbered}

Minidumps can be captured from any debugging session in zxdb with the `savedump`
command.

For example, if you are using zxdb on the `cobalt.cm` component:

```posix-terminal
ffx component debug cobalt.cm
```

You should see an output like:

```none {:.devsite-disable-click-to-copy}
Waiting for process matching "job 20999".
Type "filter" to see the current filters.
👉 To get started, try "status" or "help".
Attached Process 1 state=Running koid=21246 name=cobalt.cm component=cobalt.cm
Loading 15 modules for cobalt.cm ...Done.
```

If you wanted to capture a minidump, you would first `pause` the debugger:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
pause
```

You should see an output like:

```none {:.devsite-disable-click-to-copy}
   508                 const zx_port_packet_t* packet))
   509
 ▶ 510 BLOCKING_SYSCALL(port_wait, zx_status_t, /* no attributes */, 3, (handle, deadline, packet),
   511                  (_ZX_SYSCALL_ANNO(use_handle("Fuchsia")) zx_handle_t handle, zx_time_t deadline,
   512                   zx_port_packet_t* packet))
🛑 thread 1 $elf(SYSCALL_zx_port_wait) + 0x7 • syscalls.inc:510
```

Use `savedump` to save a minidump:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
savedump cobalt.dump
```

You should see an output like:

```none {:.devsite-disable-click-to-copy}
Saving minidump...
Minidump written to cobalt.dump
```

Once you have saved the minidump, you can exit the debugger:

```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
quit
```

Your minidump is now saved in your working directory. You are now ready to load
the minidump file and debug it.

## Loading and debugging a minidump file {:#loading-minidump .numbered}

Loading and debugging a minidump file in zxdb is the same regardless of the
language of the component that you are trying to debug. However, there are some
differences about things to look out for in each language.

* {Rust}

  1. To load the minidump file into zxdb, you can use `ffx debug core`. For
     example, to load a minidump file named `archivist_unittest.dump`:

      Note: This minidump was captured from a rust unittest with
      `fx test --breakpoint=`. For more information, see
      [Basic test debugging][basic-test-debugging].

      ```posix-terminal
      ffx debug core archivist_unittest.dump
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      Opening dump file...
      Dump loaded successfully.
      👉 To get started, try "status" or "help".
      Attached Process 1 state=Running koid=14446435 name=<_>
      Loading 6 modules for <_> Downloading symbols...
      Done.
      🛑  (no location information)
      Attached Process 2 state=Running koid=14446435 name=<_>
      Attaching to previously connected processes:
      14446435: <_>
      Loading 6 modules for <_> Done.
      Symbol downloading complete. 5 succeeded, 0 failed.
      ```

  1. List the stack frames:

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      frame
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}\
      ▶ 0 archivist_lib_lib_test::archivist::tests::can_log_and_retrieve_log::test_entry_point::λ(…) • archivist.rs:547
        1 core::future::future::«impl»::poll<…>(…) • future/future.rs:123
        2 fuchsia_async::test_support::«impl»::run_singlethreaded::λ::λ(…) • test_support.rs:27
        3 fuchsia_async::test_support::«impl»::run_singlethreaded::λ::λ(…) • test_support.rs:122
        4 fuchsia_async::atomic_future::«impl»::poll<…>(…) • atomic_future.rs:72
        5 fuchsia_async::atomic_future::AtomicFuture::try_poll(…) • atomic_future.rs:230
        6 fuchsia_async::runtime::fuchsia::executor::common::Executor::try_poll(…) • executor/common.rs:554
        7 fuchsia_async::runtime::fuchsia::executor::common::Executor::poll_ready_tasks(…) • executor/common.rs:133
        8 fuchsia_async::runtime::fuchsia::executor::common::Executor::worker_lifecycle<…>(…) • executor/common.rs:429
        9 fuchsia_async::runtime::fuchsia::executor::local::LocalExecutor::run<…>(…) • executor/local.rs:104
        10 fuchsia_async::runtime::fuchsia::executor::local::LocalExecutor::run_singlethreaded<…>(…) • executor/local.rs:70
        11 fuchsia_async::test_support::«impl»::run_singlethreaded::λ() • test_support.rs:120
        12 fuchsia_async::test_support::Config::in_parallel(…) • test_support.rs:215
        13 fuchsia_async::test_support::«impl»::run_singlethreaded(…) • test_support.rs:117
        14 fuchsia_async::test_support::run_singlethreaded_test<…>(…) • test_support.rs:227
        15 fuchsia::test_singlethreaded<…>(…) • fuchsia/src/lib.rs:195
        16 archivist_lib_lib_test::archivist::tests::can_log_and_retrieve_log() • archivist.rs:524
        17 archivist_lib_lib_test::archivist::tests::can_log_and_retrieve_log::λ(…) • archivist.rs:525
        18 core::ops::function::FnOnce::call_once<…>(…) • fuchsia-third_party-rust/library/core/src/ops/function.rs:250
        19 core::ops::function::FnOnce::call_once<…>(…) • library/core/src/ops/function.rs:250 (inline)
        20 test::__rust_begin_short_backtrace<…>(…) • library/test/src/lib.rs:625
        21 test::run_test_in_spawned_subprocess(…) • library/test/src/lib.rs:753
        22 test::test_main_static_abort(…) • library/test/src/lib.rs:199
        23 archivist_lib_lib_test::main() • archivist/src/lib.rs:1
        24 core::ops::function::FnOnce::call_once<…>(…) • fuchsia-third_party-rust/library/core/src/ops/function.rs:250
        25 std::sys::backtrace::__rust_begin_short_backtrace<…>(…) • fuchsia-third_party-rust/library/std/src/sys/backtrace.rs:155
        26 std::rt::lang_start::λ() • fuchsia-third_party-rust/library/std/src/rt.rs:159
        27 core::ops::function::impls::«impl»::call_once<…>(…) • library/core/src/ops/function.rs:284 (inline)
        28 std::panicking::try::do_call<…>(…) • library/std/src/panicking.rs:553 (inline)
        29 std::panicking::try<…>() • library/std/src/panicking.rs:517 (inline)
        30 std::panic::catch_unwind<…>() • library/std/src/panic.rs:350 (inline)
        31 std::rt::lang_start_internal::λ() • library/std/src/rt.rs:141 (inline)
        32 std::panicking::try::do_call<…>(…) • library/std/src/panicking.rs:553 (inline)
        33 std::panicking::try<…>() • library/std/src/panicking.rs:517 (inline)
        34 std::panic::catch_unwind<…>() • library/std/src/panic.rs:350 (inline)
        35 std::rt::lang_start_internal(…) • library/std/src/rt.rs:141
        36 std::rt::lang_start<…>(…) • fuchsia-third_party-rust/library/std/src/rt.rs:158
        37 $elf(main) + 0x21
        38…40 «libc startup» (-r expands)
      ```

  1. You can now use shortcuts to quickly perform actions across all threads.

      Note: The wildcard operator (`*`) performs an action for all threads. You
      can use both nouns and verbs with this operator. Commands from the `Step`
      section of the help menu are not available because this isn't a real process.

      List the frames of each thread:

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      thread * frame
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      Thread 1 state="Core Dump" koid=14450503 name=""
      ▶ 0 archivist_lib_lib_test::archivist::tests::can_log_and_retrieve_log::test_entry_point::λ(…) • archivist.rs:547
      ```

  1. List the frames of each thread, but with more detailed information,
     you can combine `thread` with the `backtrace` verb:

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      thread * backtrace
      ```

      This command helps you see the local variables for each stack frame. Keep
      in mind that only the stack memory is captured in the minidump to improve
      the capturing time and size. Pointers to heap variables mostly do not
      resolve to anything useful.

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      Thread 1 state="Core Dump" koid=14450503 name=""
      ▶ 0 archivist_lib_lib_test::archivist::tests::can_log_and_retrieve_log::test_entry_point::λ(…) • archivist.rs:547
            (*)0x5c626904e0 ➔ Context{waker: (*)0x5c626904c8, local_waker: (*)0x5c626904c8, ext: AssertUnwindSafe<core::task::wake::ExtData>(…), _marker: PhantomData<fn(&())->&()>, _marker2: PhantomData<*mut()>}
        1 core::future::future::«impl»::poll<…>(…) • future/future.rs:123
            self = Pin<&mut core::pin::Pin<alloc…>{__pointer: (*)0xa2987a4580}
            cx = (*)0x5c626904e0 ➔ Context{waker: (*)0x5c626904c8, local_waker: (*)0x5c626904c8, ext: AssertUnwindSafe<core::task::wake::ExtData>(…), _marker: PhantomData<fn(&())->&()>, _marker2: PhantomData<*mut()>}
        2 fuchsia_async::test_support::«impl»::run_singlethreaded::λ::λ(…) • test_support.rs:27
            (*)0x5c626904e0 ➔ Context{waker: (*)0x5c626904c8, local_waker: (*)0x5c626904c8, ext: AssertUnwindSafe<core::task::wake::ExtData>(…), _marker: PhantomData<fn(&())->&()>, _marker2: PhantomData<*mut()>}
        3 fuchsia_async::test_support::«impl»::run_singlethreaded::λ::λ(…) • test_support.rs:122
            (*)0x5c626904e0 ➔ Context{waker: (*)0x5c626904c8, local_waker: (*)0x5c626904c8, ext: AssertUnwindSafe<core::task::wake::ExtData>(…), _marker: PhantomData<fn(&())->&()>, _marker2: PhantomData<*mut()>}
        4 fuchsia_async::atomic_future::«impl»::poll<…>(…) • atomic_future.rs:72
            self = (*)0xa2987a4520
            cx = (*)0x5c626904e0 ➔ Context{waker: (*)0x5c626904c8, local_waker: (*)0x5c626904c8, ext: AssertUnwindSafe<core::task::wake::ExtData>(…), _marker: PhantomData<fn(&())->&()>, _marker2: PhantomData<*mut()>}
        5 fuchsia_async::atomic_future::AtomicFuture::try_poll(…) • atomic_future.rs:230
            self = (*)0x9d587a4118
            cx = (*)0x5c626904e0 ➔ Context{waker: (*)0x5c626904c8, local_waker: (*)0x5c626904c8, ext: AssertUnwindSafe<core::task::wake::ExtData>(…), _marker: PhantomData<fn(&())->&()>, _marker2: PhantomData<*mut()>}
        6 fuchsia_async::runtime::fuchsia::executor::common::Executor::try_poll(…) • executor/common.rs:554
            self = (*)0x9b187a9300
            task = (*)0x5c62690588 ➔ (*)0x9d587a40f0
        7 fuchsia_async::runtime::fuchsia::executor::common::Executor::poll_ready_tasks(…) • executor/common.rs:133
            self = (*)0x9b187a9300
            local_collector = (*)0x5c62690600 ➔ LocalCollector{collector: (*)0x9b187a9438, last_ticks: 1050997948718604, polls: 1, tasks_pending_max: 10}
        8 fuchsia_async::runtime::fuchsia::executor::common::Executor::worker_lifecycle<…>(…) • executor/common.rs:429
            self = (*)0xa4187a90a0
        9 fuchsia_async::runtime::fuchsia::executor::local::LocalExecutor::run<…>(…) • executor/local.rs:104
            self = (*)0x5c62690878 ➔ LocalExecutor{ehandle: EHandle{…}}
            main_future = AtomicFuture{state: AtomicUsize{1}, future: UnsafeCell{$(alloc::boxed::Box<dyn fuchsia_async::atomic_future::FutureOrResultAccess, alloc::alloc::Global>){…}}}
        10 fuchsia_async::runtime::fuchsia::executor::local::LocalExecutor::run_singlethreaded<…>(…) • executor/local.rs:70
            self = (*)0x5c62690878 ➔ LocalExecutor{ehandle: EHandle{…}}
            main_future = Unresumed{run_stream: (*)0xa3187a86a0, test: λ{…}}
        11 fuchsia_async::test_support::«impl»::run_singlethreaded::λ() • test_support.rs:120
        12 fuchsia_async::test_support::Config::in_parallel(…) • test_support.rs:215
            self = (*)0x5c62690ae8 ➔ Config{repeat_count: 1, max_concurrency: 0, max_threads: 0, timeout: …}
            f = <The value of type '*const alloc::sync::ArcInner<(dyn core::ops::function::Fn<(), Output=()> + core::marker::Send + core::marker::Sync)>' is the incorrect size (expecting 8, got 16). Please file a bug.>
        13 fuchsia_async::test_support::«impl»::run_singlethreaded(…) • test_support.rs:117
            test = <The value of type '*const alloc::sync::ArcInner<(dyn core::ops::function::Fn<(usize), Output=core::pin::Pin<alloc::boxed::Box<dyn core::future::future::Future<Output=()>, alloc::alloc::Global>>> + core::marker::Send + core::marker::Sync)>' is the incorrect size (expecting 8, got 16). Please file a bug.>
            cfg = Config{repeat_count: 1, max_concurrency: 0, max_threads: 0, timeout: None<Duration>}
        14 fuchsia_async::test_support::run_singlethreaded_test<…>(…) • test_support.rs:227
            test = <Unavailable>
        15 fuchsia::test_singlethreaded<…>(…) • fuchsia/src/lib.rs:195
            f = <Register rdi not available.>
        16 archivist_lib_lib_test::archivist::tests::can_log_and_retrieve_log() • archivist.rs:524
        17 archivist_lib_lib_test::archivist::tests::can_log_and_retrieve_log::λ(…) • archivist.rs:525
            (*)0x5c62690bde ➔ λ
        18 core::ops::function::FnOnce::call_once<…>(…) • fuchsia-third_party-rust/library/core/src/ops/function.rs:250
            λ
            <Value has no data.>
        19 core::ops::function::FnOnce::call_once<…>(…) • library/core/src/ops/function.rs:250 (inline)
            <Register rsi not available.>
            <Optimized out>
        20 test::__rust_begin_short_backtrace<…>(…) • library/test/src/lib.rs:625
            f = <Register rsi not available.>
        21 test::run_test_in_spawned_subprocess(…) • library/test/src/lib.rs:753
            desc = <Register rdi not available.>
            runnable_test = Static(&core::ops::function::FnOnce::call_once<archivist_lib_lib_test::archivist::tests::can_log_and_retrieve_log::{closure_env#0}, ()>)
        22 test::test_main_static_abort(…) • library/test/src/lib.rs:199
            tests = <Unavailable>
        23 archivist_lib_lib_test::main() • archivist/src/lib.rs:1
        24 core::ops::function::FnOnce::call_once<…>(…) • fuchsia-third_party-rust/library/core/src/ops/function.rs:250
            &archivist_lib_lib_test::main
            <Value has no data.>
        25 std::sys::backtrace::__rust_begin_short_backtrace<…>(…) • fuchsia-third_party-rust/library/std/src/sys/backtrace.rs:155
            f = &archivist_lib_lib_test::main
        26 std::rt::lang_start::λ() • fuchsia-third_party-rust/library/std/src/rt.rs:159
        27 core::ops::function::impls::«impl»::call_once<…>(…) • library/core/src/ops/function.rs:284 (inline)
            self = $(&(dyn core::ops::function::Fn<(), Output=i32> + core::marker::Sync + core::panic::unwind_safe::RefUnwindSafe)){pointer: (*)0x5c62690f50 ➔ $((dyn core::ops::function::Fn<(), Output=i32> + core::marker::Sync + core::panic::unwind_safe::RefUnwindSafe)), vtable = <Invalid data offset 8 in object of size 8.>}
            args = <Optimized out>
        28 std::panicking::try::do_call<…>(…) • library/std/src/panicking.rs:553 (inline)
            data = <Optimized out>
        29 std::panicking::try<…>() • library/std/src/panicking.rs:517 (inline)
        30 std::panic::catch_unwind<…>() • library/std/src/panic.rs:350 (inline)
        31 std::rt::lang_start_internal::λ() • library/std/src/rt.rs:141 (inline)
        32 std::panicking::try::do_call<…>(…) • library/std/src/panicking.rs:553 (inline)
            data = <Optimized out>
        33 std::panicking::try<…>() • library/std/src/panicking.rs:517 (inline)
        34 std::panic::catch_unwind<…>() • library/std/src/panic.rs:350 (inline)
        35 std::rt::lang_start_internal(…) • library/std/src/rt.rs:141
            main = $(&(dyn core::ops::function::Fn<(), Output=i32> + core::marker::Sync + core::panic::unwind_safe::RefUnwindSafe)){pointer: (*)0x5c62690f50 ➔ $((dyn core::ops::function::Fn<(), Output=i32> + core::marker::Sync + core::panic::unwind_safe::RefUnwindSafe)), vtable: (*)0x13a66000958}
            argc = <Register rdx not available.>
            argv = <Register rcx not available.>
            sigpipe = <Register r8 not available.>
        36 std::rt::lang_start<…>(…) • fuchsia-third_party-rust/library/std/src/rt.rs:158
            main = &archivist_lib_lib_test::main
            argc = 2
            argv = (*)0xecf9d19fb0
            sigpipe = 0
        37 $elf(main) + 0x21
        38…40 «libc startup» (-r expands)
      ```

  1. List the current threads:

     Note: You can also use any of the regular zxdb thread and frame commands.

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      thread
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
        # state      koid name
      ▶ 1 Core Dump 14450503
      ```

  1. List a specific thread. For example, `thread 1`:

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      thread 1
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      Thread 1 state="Core Dump" koid=14450503 name=""
      ```

  1. You can then use `frame` to see this specific stack frame from `thread 1`:

      Note: As this component ran on a single thread, this output is The
      same as when you ran `frame` after loading the minidump into zxdb.

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      frame
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      ▶ 0 archivist_lib_lib_test::archivist::tests::can_log_and_retrieve_log::test_entry_point::λ(…) • archivist.rs:547
        1 core::future::future::«impl»::poll<…>(…) • future/future.rs:123
        2 fuchsia_async::test_support::«impl»::run_singlethreaded::λ::λ(…) • test_support.rs:27
        3 fuchsia_async::test_support::«impl»::run_singlethreaded::λ::λ(…) • test_support.rs:122
        4 fuchsia_async::atomic_future::«impl»::poll<…>(…) • atomic_future.rs:72
        5 fuchsia_async::atomic_future::AtomicFuture::try_poll(…) • atomic_future.rs:230
        6 fuchsia_async::runtime::fuchsia::executor::common::Executor::try_poll(…) • executor/common.rs:554
        7 fuchsia_async::runtime::fuchsia::executor::common::Executor::poll_ready_tasks(…) • executor/common.rs:133
        8 fuchsia_async::runtime::fuchsia::executor::common::Executor::worker_lifecycle<…>(…) • executor/common.rs:429
        9 fuchsia_async::runtime::fuchsia::executor::local::LocalExecutor::run<…>(…) • executor/local.rs:104
        10 fuchsia_async::runtime::fuchsia::executor::local::LocalExecutor::run_singlethreaded<…>(…) • executor/local.rs:70
        11 fuchsia_async::test_support::«impl»::run_singlethreaded::λ() • test_support.rs:120
        12 fuchsia_async::test_support::Config::in_parallel(…) • test_support.rs:215
        13 fuchsia_async::test_support::«impl»::run_singlethreaded(…) • test_support.rs:117
        14 fuchsia_async::test_support::run_singlethreaded_test<…>(…) • test_support.rs:227
        15 fuchsia::test_singlethreaded<…>(…) • fuchsia/src/lib.rs:195
        16 archivist_lib_lib_test::archivist::tests::can_log_and_retrieve_log() • archivist.rs:524
        17 archivist_lib_lib_test::archivist::tests::can_log_and_retrieve_log::λ(…) • archivist.rs:525
        18 core::ops::function::FnOnce::call_once<…>(…) • fuchsia-third_party-rust/library/core/src/ops/function.rs:250
        19 core::ops::function::FnOnce::call_once<…>(…) • library/core/src/ops/function.rs:250 (inline)
        20 test::__rust_begin_short_backtrace<…>(…) • library/test/src/lib.rs:625
        21 test::run_test_in_spawned_subprocess(…) • library/test/src/lib.rs:753
        22 test::test_main_static_abort(…) • library/test/src/lib.rs:199
        23 archivist_lib_lib_test::main() • archivist/src/lib.rs:1
        24 core::ops::function::FnOnce::call_once<…>(…) • fuchsia-third_party-rust/library/core/src/ops/function.rs:250
        25 std::sys::backtrace::__rust_begin_short_backtrace<…>(…) • fuchsia-third_party-rust/library/std/src/sys/backtrace.rs:155
        26 std::rt::lang_start::λ() • fuchsia-third_party-rust/library/std/src/rt.rs:159
        27 core::ops::function::impls::«impl»::call_once<…>(…) • library/core/src/ops/function.rs:284 (inline)
        28 std::panicking::try::do_call<…>(…) • library/std/src/panicking.rs:553 (inline)
        29 std::panicking::try<…>() • library/std/src/panicking.rs:517 (inline)
        30 std::panic::catch_unwind<…>() • library/std/src/panic.rs:350 (inline)
        31 std::rt::lang_start_internal::λ() • library/std/src/rt.rs:141 (inline)
        32 std::panicking::try::do_call<…>(…) • library/std/src/panicking.rs:553 (inline)
        33 std::panicking::try<…>() • library/std/src/panicking.rs:517 (inline)
        34 std::panic::catch_unwind<…>() • library/std/src/panic.rs:350 (inline)
        35 std::rt::lang_start_internal(…) • library/std/src/rt.rs:141
        36 std::rt::lang_start<…>(…) • fuchsia-third_party-rust/library/std/src/rt.rs:158
        37 $elf(main) + 0x21
        38…40 «libc startup» (-r expands)
      ```

  1. Based on the output from the previous step, you may want to dive deeper
     into `frame 0`:

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      frame 0
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      archivist_lib_lib_test::archivist::tests::can_log_and_retrieve_log::test_entry_point::λ(…) • archivist.rs:547
      ```

  1. To see additional lines of code from the current frame, use `list`:

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      list
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
        542
        543         let mut expected = vec!["my msg1".to_owned(), "my msg2".to_owned()];
        544         expected.sort();
        545
        546         let mut actual = vec![recv_logs.next().await.unwrap(), recv_logs.next().await.unwrap()];
      ▶ 547         actual.sort();
        548
        549         assert_eq!(expected, actual);
        550
        551         // can log after killing log sink proxy
        552         log_helper.kill_log_sink();
        553         log_helper.write_log("my msg1");
        554         log_helper.write_log("my msg2");
        555
        556         assert_eq!(
        557             expected,
      ```

      Since this frame actually contains some archivist code, you can use
      `locals` to see the local variables in this stack frame.

  1. To see all local variables in the current stack frame, use `locals`:

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      locals
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      _task_context = (*)0x5c626904e0 ➔ Context{
        waker: (*)0x5c626904c8
        local_waker: (*)0x5c626904c8
        ext: AssertUnwindSafe<core::task::wake::ExtData>(None(<Value has no data.>))
        _marker: PhantomData<fn(&())->&()>
        _marker2: PhantomData<*mut()>
      }
      actual = <Invalid pointer 0x9c187a04e8>
      directory = <Invalid pointer 0x9c187a0460>
      expected = <Invalid pointer 0x9c187a04d0>
      log_helper = <Invalid pointer 0x9c187a0470>
      log_helper2 = <Invalid pointer 0x9c187a04c0>
      recv_logs = <Invalid pointer 0x9c187a0468>
      ```

      You can then see more information about a specific variable with `print`.
      For example:


      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      print _task_context
      ```

      You should see an output like:

      Note: The `(*)` indicates that the variable is a pointer.

      ```none {:.devsite-disable-click-to-copy}
      (*)0x5c626904e0 ➔ Context{
        waker: (*)0x5c626904c8
        local_waker: (*)0x5c626904c8
        ext: AssertUnwindSafe<core::task::wake::ExtData>(None(<Value has no data.>))
        _marker: PhantomData<fn(&())->&()>
        _marker2: PhantomData<*mut()>
      }
      ```

    You have successfully loaded and analyzed a minidump for Rust code.

* {C++}

  1. To load the minidump file into zxdb, you can use `ffx debug core`.
     For example, to load a minidump file named `cobalt_minidump.dump`:

      ```posix-terminal
      ffx debug core cobalt_minidump.dump
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      Opening dump file...
      Dump loaded successfully.
      👉 To get started, try "status" or "help".
      Attached Process 1 state=Running koid=15650768 name=<_>
      Loading 15 modules for <_> ...Done.
      Attached Process 2 state=Running koid=15650768 name=<_>
      Attaching to previously connected processes:
      15650768: <_>
      Loading 15 modules for <_> Done.
      ```

      Now that you have loaded the minidump file, you can now inspect the stacks
      for all threads present that were present when the minidump was captured.

  1. List the stack frames:

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      frame
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      ▶ 0…4 «Waiting for event in async::Loop::Run()» (-r expands)
        5 main(…) • cobalt_main.cc:349
        6…8 «libc startup» (-r expands)
      ```

  1. You can now use shortcuts to quickly perform actions across all threads:

      Note: The wildcard operator (`*`) performs an action for all threads. You
      can use both nouns and verbs with this operator. Commands from the `Step`
      section of the help menu are not available because this isn't a real process.

      List the frames of each thread:

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      thread * frame
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      Thread 1 state="Core Dump" koid=19601 name=""
      ▶ 0…4 «Waiting for event in async::Loop::Run()» (-r expands)
        5 main(…) • cobalt_main.cc:349
        6…8 «libc startup» (-r expands)
      Thread 2 state="Core Dump" koid=26944 name=""
      ▶ 0 $elf(CODE_SYSCALL_zx_futex_wait) + 0xa • syscalls.inc:210
        1 _zx_futex_wait(…) • syscalls.inc:210
        2 __timedwait_assign_owner(…) • __timedwait.c:23
        3 __timedwait(…) • threads_impl.h:322 (inline)
        4 pthread_cond_timedwait(…) • pthread_cond_timedwait.c:78
        5 std::__2::__libcpp_condvar_timedwait(…) • stage2-bins/include/c++/v1/__thread/support/pthread.h:127 (inline)
        6 std::__2::condition_variable::__do_timed_wait(…) • condition_variable.cpp:54
        7 std::__2::condition_variable::wait_for<…>(…) • linux-x64/include/c++/v1/__condition_variable/condition_variable.h:196
        8 std::__2::condition_variable::__do_timed_wait<…>(…) • linux-x64/include/c++/v1/__condition_variable/condition_variable.h:235
        9 std::__2::condition_variable::wait_until<…>(…) • linux-x64/include/c++/v1/__condition_variable/condition_variable.h:161
        10 std::__2::condition_variable_any::wait_until<…>(…) • condition_variable:240
        11 std::__2::condition_variable_any::wait_until<…>(…) • condition_variable:247
        12 std::__2::condition_variable_any::wait_for<…>(…) • condition_variable:260
        13 cobalt::local_aggregation::DelayedLocalAggregateStorage::Run(…) • delayed_local_aggregate_storage.cc:200
        14 λ(…) • delayed_local_aggregate_storage.cc:45
        15 std::__2::__invoke<…>(…) • linux-x64/include/c++/v1/__type_traits/invoke.h:150
        16 std::__2::__thread_execute<…>(…) • linux-x64/include/c++/v1/__thread/thread.h:192
        17 std::__2::__thread_proxy<…>(…) • linux-x64/include/c++/v1/__thread/thread.h:201
        18…19 «pthread startup» (-r expands)
      Thread 3 state="Core Dump" koid=26975 name=""
      ▶ 0 $elf(CODE_SYSCALL_zx_futex_wait) + 0xa • syscalls.inc:210
        1 _zx_futex_wait(…) • syscalls.inc:210
        2 __timedwait_assign_owner(…) • __timedwait.c:23
        3 __timedwait(…) • threads_impl.h:322 (inline)
        4 pthread_cond_timedwait(…) • pthread_cond_timedwait.c:78
        5 std::__2::__libcpp_condvar_wait(…) • stage2-bins/include/c++/v1/__thread/support/pthread.h:122 (inline)
        6 std::__2::condition_variable::wait(…) • condition_variable.cpp:30
        7 std::__2::condition_variable_any::wait<…>(…) • condition_variable:225
        8 std::__2::condition_variable_any::wait<…>(…) • condition_variable:231
        9 cobalt::uploader::ShippingManager::Run(…) • shipping_manager.cc:217
        10 λ(…) • shipping_manager.cc:76
        11 std::__2::__invoke<…>(…) • linux-x64/include/c++/v1/__type_traits/invoke.h:150
        12 std::__2::__thread_execute<…>(…) • linux-x64/include/c++/v1/__thread/thread.h:192
        13 std::__2::__thread_proxy<…>(…) • linux-x64/include/c++/v1/__thread/thread.h:201
        14…15 «pthread startup» (-r expands)
      Thread 4 state="Core Dump" koid=26983 name=""
      ▶ 0 $elf(CODE_SYSCALL_zx_futex_wait) + 0xa • syscalls.inc:210
        1 _zx_futex_wait(…) • syscalls.inc:210
        2 __timedwait_assign_owner(…) • __timedwait.c:23
        3 __timedwait(…) • threads_impl.h:322 (inline)
        4 pthread_cond_timedwait(…) • pthread_cond_timedwait.c:78
        5 std::__2::__libcpp_condvar_timedwait(…) • stage2-bins/include/c++/v1/__thread/support/pthread.h:127 (inline)
        6 std::__2::condition_variable::__do_timed_wait(…) • condition_variable.cpp:54
        7 std::__2::condition_variable::wait_for<…>(…) • linux-x64/include/c++/v1/__condition_variable/condition_variable.h:196
        8 std::__2::condition_variable::__do_timed_wait<…>(…) • linux-x64/include/c++/v1/__condition_variable/condition_variable.h:235
        9 std::__2::condition_variable::wait_until<…>(…) • linux-x64/include/c++/v1/__condition_variable/condition_variable.h:161
        10 std::__2::condition_variable_any::wait_until<…>(…) • condition_variable:240
        11 std::__2::condition_variable_any::wait_until<…>(…) • condition_variable:247
        12 cobalt::local_aggregation::ObservationGenerator::Run(…) • observation_generator.cc:92
        13 λ(…) • observation_generator.cc:65
        14 std::__2::__invoke<…>(…) • linux-x64/include/c++/v1/__type_traits/invoke.h:150
        15 std::__2::__thread_execute<…>(…) • linux-x64/include/c++/v1/__thread/thread.h:192
        16 std::__2::__thread_proxy<…>(…) • linux-x64/include/c++/v1/__thread/thread.h:201
        17…18 «pthread startup» (-r expands)
      ```

  1. To list the frames of each thread, but with more detailed information,
     you can combine `thread` with the `backtrace` verb:

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      thread * backtrace
      ```

      This command helps you see the local variables for each stack frame. Keep
      in mind that only the stack memory is captured in the minidump to improve
      the capturing time and size. Pointers to heap variables mostly do not
      resolve to anything useful, these have a format like
       `<Invalid pointer 0x285f3fd5a18>`.

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      Thread 1 state="Core Dump" koid=19601 name=""
      ▶ 0…4 «Waiting for event in async::Loop::Run()» (-r expands)
        5 main(…) • cobalt_main.cc:349
            argc = 2
            argv = (*)0x26866821fc0
        6…8 «libc startup» (-r expands)
      Thread 2 state="Core Dump" koid=26944 name=""
      ▶ 0 $elf(CODE_SYSCALL_zx_futex_wait) + 0xa • syscalls.inc:210
        1 _zx_futex_wait(…) • syscalls.inc:210
            value_ptr = (*)0x285f3fd5904
            current_value = 2
            new_futex_owner = 0
            deadline = 612921722073211
        2 __timedwait_assign_owner(…) • __timedwait.c:23
            futex = <Register rdi not available.>
            val = <Register rsi not available.>
            clk = <Register rdx not available.>
            at = <Register rcx not available.>
            new_owner = <Register r8 not available.>
        3 __timedwait(…) • threads_impl.h:322 (inline)
            futex = (*)0x285f3fd5904
            val = 2
            clk = 0
            at = (*)0x285f3fd5918
        4 pthread_cond_timedwait(…) • pthread_cond_timedwait.c:78
            c = (*)0x370ff24ae20
            m = (*)0x3707f2b76c8
            ts = (*)0x285f3fd5918
        5 std::__2::__libcpp_condvar_timedwait(…) • stage2-bins/include/c++/v1/__thread/support/pthread.h:127 (inline)
            __cv = <Register rdi not available.>
            __m = <Register rsi not available.>
            __ts = <Optimized out>
        6 std::__2::condition_variable::__do_timed_wait(…) • condition_variable.cpp:54
            this = <Register rdi not available.>
            lk = <Register rsi not available.>
            tp = <Register rdx not available.>
        7 std::__2::condition_variable::wait_for<…>(…) • linux-x64/include/c++/v1/__condition_variable/condition_variable.h:196
            this = (*)0x370ff24ae20
            __lk = <Invalid pointer 0x285f3fd5a18>
            __d = <Invalid pointer 0x285f3fd59a8>
        8 std::__2::condition_variable::__do_timed_wait<…>(…) • linux-x64/include/c++/v1/__condition_variable/condition_variable.h:235
            this = (*)0x370ff24ae20
            __lk = <Invalid pointer 0x285f3fd5a18>
            __tp = <Invalid pointer 0x285f3fd59b0>
        9 std::__2::condition_variable::wait_until<…>(…) • linux-x64/include/c++/v1/__condition_variable/condition_variable.h:161
            this = (*)0x370ff24ae20
            __lk = <Invalid pointer 0x285f3fd5a18>
            __t = <Invalid pointer 0x285f3fd5a60>
        10 std::__2::condition_variable_any::wait_until<…>(…) • condition_variable:240
            this = (*)0x370ff24ae20
            __lock = <Invalid pointer 0x285f3fd5ab0>
            __t = <Invalid pointer 0x285f3fd5a60>
        11 std::__2::condition_variable_any::wait_until<…>(…) • condition_variable:247
            this = (*)0x370ff24ae20
            __lock = <Invalid pointer 0x285f3fd5ab0>
            __t = <Invalid pointer 0x285f3fd5a60>
            __pred = <Invalid pointer 0x285f3fd5a40>
        12 std::__2::condition_variable_any::wait_for<…>(…) • condition_variable:260
            this = (*)0x370ff24ae20
            __lock = <Invalid pointer 0x285f3fd5ab0>
            __d = <Invalid pointer 0x370ff24ae00>
            __pred = {locked_state = <Invalid pointer 0x285f3fd5ab0>}
        13 cobalt::local_aggregation::DelayedLocalAggregateStorage::Run(…) • delayed_local_aggregate_storage.cc:200
            this = (*)0x370ff24acf0
        14 λ(…) • delayed_local_aggregate_storage.cc:45
            this = (*)0x36bff27b228
        15 std::__2::__invoke<…>(…) • linux-x64/include/c++/v1/__type_traits/invoke.h:150
            __f = <Invalid pointer 0x36bff27b228>
        16 std::__2::__thread_execute<…>(…) • linux-x64/include/c++/v1/__thread/thread.h:192
            __t = <Invalid pointer 0x36bff27b220>
            {}
        17 std::__2::__thread_proxy<…>(…) • linux-x64/include/c++/v1/__thread/thread.h:201
            __vp = (*)0x36bff27b220
        18…19 «pthread startup» (-r expands)
      Thread 3 state="Core Dump" koid=26975 name=""
      ▶ 0 $elf(CODE_SYSCALL_zx_futex_wait) + 0xa • syscalls.inc:210
        1 _zx_futex_wait(…) • syscalls.inc:210
            value_ptr = (*)0x23b6ad77434
            current_value = 2
            new_futex_owner = 0
            deadline = 9223372036854775807
        2 __timedwait_assign_owner(…) • __timedwait.c:23
            futex = <Register rdi not available.>
            val = <Register rsi not available.>
            clk = <Register rdx not available.>
            at = <Register rcx not available.>
            new_owner = <Register r8 not available.>
        3 __timedwait(…) • threads_impl.h:322 (inline)
            futex = (*)0x23b6ad77434
            val = 2
            clk = 0
            at = (*)0x0
        4 pthread_cond_timedwait(…) • pthread_cond_timedwait.c:78
            c = (*)0x3673f24a610
            m = (*)0x3707f2b7998
            ts = (*)0x0
        5 std::__2::__libcpp_condvar_wait(…) • stage2-bins/include/c++/v1/__thread/support/pthread.h:122 (inline)
            __cv = <Register rdi not available.>
            __m = <Register rsi not available.>
        6 std::__2::condition_variable::wait(…) • condition_variable.cpp:30
            this = <Register rdi not available.>
            lk = <Register rsi not available.>
        7 std::__2::condition_variable_any::wait<…>(…) • condition_variable:225
            this = (*)0x3673f24a610
            __lock = <Invalid pointer 0x23b6ad77608>
        8 std::__2::condition_variable_any::wait<…>(…) • condition_variable:231
            this = (*)0x3673f24a610
            __lock = <Invalid pointer 0x23b6ad77608>
            __pred = <Invalid pointer 0x23b6ad77480>
        9 cobalt::uploader::ShippingManager::Run(…) • shipping_manager.cc:217
            this = (*)0x3673f24a530
        10 λ(…) • shipping_manager.cc:76
            this = (*)0x36bff27a7a8
        11 std::__2::__invoke<…>(…) • linux-x64/include/c++/v1/__type_traits/invoke.h:150
            __f = <Invalid pointer 0x36bff27a7a8>
        12 std::__2::__thread_execute<…>(…) • linux-x64/include/c++/v1/__thread/thread.h:192
            __t = <Invalid pointer 0x36bff27a7a0>
            {}
        13 std::__2::__thread_proxy<…>(…) • linux-x64/include/c++/v1/__thread/thread.h:201
            __vp = (*)0x36bff27a7a0
        14…15 «pthread startup» (-r expands)
      Thread 4 state="Core Dump" koid=26983 name=""
      ▶ 0 $elf(CODE_SYSCALL_zx_futex_wait) + 0xa • syscalls.inc:210
        1 _zx_futex_wait(…) • syscalls.inc:210
            value_ptr = (*)0x149c84b2bc4
            current_value = 2
            new_futex_owner = 0
            deadline = 615784347537787
        2 __timedwait_assign_owner(…) • __timedwait.c:23
            futex = <Register rdi not available.>
            val = <Register rsi not available.>
            clk = <Register rdx not available.>
            at = <Register rcx not available.>
            new_owner = <Register r8 not available.>
        3 __timedwait(…) • threads_impl.h:322 (inline)
            futex = (*)0x149c84b2bc4
            val = 2
            clk = 0
            at = (*)0x149c84b2bd8
        4 pthread_cond_timedwait(…) • pthread_cond_timedwait.c:78
            c = (*)0x3673f24b128
            m = (*)0x3707f2b6548
            ts = (*)0x149c84b2bd8
        5 std::__2::__libcpp_condvar_timedwait(…) • stage2-bins/include/c++/v1/__thread/support/pthread.h:127 (inline)
            __cv = <Register rdi not available.>
            __m = <Register rsi not available.>
            __ts = <Optimized out>
        6 std::__2::condition_variable::__do_timed_wait(…) • condition_variable.cpp:54
            this = <Register rdi not available.>
            lk = <Register rsi not available.>
            tp = <Register rdx not available.>
        7 std::__2::condition_variable::wait_for<…>(…) • linux-x64/include/c++/v1/__condition_variable/condition_variable.h:196
            this = (*)0x3673f24b128
            __lk = <Invalid pointer 0x149c84b2cd8>
            __d = <Invalid pointer 0x149c84b2c68>
        8 std::__2::condition_variable::__do_timed_wait<…>(…) • linux-x64/include/c++/v1/__condition_variable/condition_variable.h:235
            this = (*)0x3673f24b128
            __lk = <Invalid pointer 0x149c84b2cd8>
            __tp = <Invalid pointer 0x149c84b2c70>
        9 std::__2::condition_variable::wait_until<…>(…) • linux-x64/include/c++/v1/__condition_variable/condition_variable.h:161
            this = (*)0x3673f24b128
            __lk = <Invalid pointer 0x149c84b2cd8>
            __t = <Invalid pointer 0x3673f24b0d8>
        10 std::__2::condition_variable_any::wait_until<…>(…) • condition_variable:240
            this = (*)0x3673f24b128
            __lock = <Invalid pointer 0x149c84b2d20>
            __t = <Invalid pointer 0x3673f24b0d8>
        11 std::__2::condition_variable_any::wait_until<…>(…) • condition_variable:247
            this = (*)0x3673f24b128
            __lock = <Invalid pointer 0x149c84b2d20>
            __t = <Invalid pointer 0x3673f24b0d8>
            __pred = <Invalid pointer 0x149c84b2d00>
        12 cobalt::local_aggregation::ObservationGenerator::Run(…) • observation_generator.cc:92
            this = (*)0x3673f24b0a0
            clock = (*)0x36bff27aaa0
        13 λ(…) • observation_generator.cc:65
            this = (*)0x36bff24cf68
        14 std::__2::__invoke<…>(…) • linux-x64/include/c++/v1/__type_traits/invoke.h:150
            __f = <Invalid pointer 0x36bff24cf68>
        15 std::__2::__thread_execute<…>(…) • linux-x64/include/c++/v1/__thread/thread.h:192
            __t = <Invalid pointer 0x36bff24cf60>
            {}
        16 std::__2::__thread_proxy<…>(…) • linux-x64/include/c++/v1/__thread/thread.h:201
            __vp = (*)0x36bff24cf60
        17…18 «pthread startup» (-r expands)
      ```

  1. List the current threads:

     Note: You can also use any of the regular zxdb thread and frame commands.

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      thread
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
        # state      koid name
      ▶ 1 Core Dump 19601
        2 Core Dump 26944
        3 Core Dump 26975
        4 Core Dump 26983
      ```

  1. List a specific thread. For example, `thread 2`:

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      thread 2
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      Thread 2 state="Core Dump" koid=26944 name=""
      ```

  1. You can then use `frame` to see this specific stack frame from `thread 2`:

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      frame
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      ▶ 0 $elf(CODE_SYSCALL_zx_futex_wait) + 0xa • syscalls.inc:210
        1 _zx_futex_wait(…) • syscalls.inc:210
        2 __timedwait_assign_owner(…) • __timedwait.c:23
        3 __timedwait(…) • threads_impl.h:322 (inline)
        4 pthread_cond_timedwait(…) • pthread_cond_timedwait.c:78
        5 std::__2::__libcpp_condvar_timedwait(…) • stage2-bins/include/c++/v1/__thread/support/pthread.h:127 (inline)
        6 std::__2::condition_variable::__do_timed_wait(…) • condition_variable.cpp:54
        7 std::__2::condition_variable::wait_for<…>(…) • linux-x64/include/c++/v1/__condition_variable/condition_variable.h:196
        8 std::__2::condition_variable::__do_timed_wait<…>(…) • linux-x64/include/c++/v1/__condition_variable/condition_variable.h:235
        9 std::__2::condition_variable::wait_until<…>(…) • linux-x64/include/c++/v1/__condition_variable/condition_variable.h:161
        10 std::__2::condition_variable_any::wait_until<…>(…) • condition_variable:240
        11 std::__2::condition_variable_any::wait_until<…>(…) • condition_variable:247
        12 std::__2::condition_variable_any::wait_for<…>(…) • condition_variable:260
        13 cobalt::local_aggregation::DelayedLocalAggregateStorage::Run(…) • delayed_local_aggregate_storage.cc:200
        14 λ(…) • delayed_local_aggregate_storage.cc:45
        15 std::__2::__invoke<…>(…) • linux-x64/include/c++/v1/__type_traits/invoke.h:150
        16 std::__2::__thread_execute<…>(…) • linux-x64/include/c++/v1/__thread/thread.h:192
        17 std::__2::__thread_proxy<…>(…) • linux-x64/include/c++/v1/__thread/thread.h:201
        18…19 «pthread startup» (-r expands)
      ```

  1. List another specific thread. For example, `thread 1`:

      Note: This example is on a different thread to show the difference when
      you follow the `thread` noun with `frame`.

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      thread 1
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      Thread 1 state="Core Dump" koid=19601 name=""
      ```

  1. You can then use `frame` to see this specific stack frame from `thread 1`:

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      frame
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      ▶ 0…4 «Waiting for event in async::Loop::Run()» (-r expands)
        5 main(…) • cobalt_main.cc:349
        6…8 «libc startup» (-r expands)
      ```

  1. Based on the output from the previous step, you may want to dive deeper
     into `frame 5`:

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      frame 5
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      main(…) • cobalt_main.cc:349
      ```

  1. To see additional lines of code from the current frame, use `list`:

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      list
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
        344
        345   if (!app.ok()) {
        346     FX_LOGS(FATAL) << "Failed to construct the cobalt app: " << app.status();
        347   }
        348   inspector.Health().Ok();
      ▶ 349   loop.Run();
        350   FX_LOGS(INFO) << "Cobalt will now shut down.";
        351   return 0;
        352 }
      ```

      Since this frame actually contains some code, you can use `locals`
      to see the local variables in this stack frame.

  1. To see all local variables in the current stack frame, use `locals`:

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      locals
      ```

      You should see an output like:

      Note: The `(*)` indicates that the variable is a pointer.

      ```none {:.devsite-disable-click-to-copy}
      argc = 2
      argv = (*)0x26866821fc0
      command_line = <Invalid pointer 0x26866821418>
      context = <Invalid pointer 0x26866821100>
      event_aggregator_backfill_days = 2
      flag_value = <Invalid pointer 0x268668211d8>
      initial_interval = <Invalid pointer 0x26866821138>
      inspector = <Invalid pointer 0x268668212f0>
      loop = <Invalid pointer 0x26866821108>
      max_bytes_per_observation_store = 215040
      min_interval = <Invalid pointer 0x26866821118>
      require_lifecycle_service = true
      schedule_interval = <Invalid pointer 0x26866821140>
      start_event_aggregator_worker = true
      status = 0 (ZX_OK)
      storage_quotas = {
        per_project_reserved_bytes = 1024
        total_capacity_bytes = 731136
      }
      test_dont_backfill_empty_reports = false
      upload_jitter = 0.2
      upload_schedule = {
        target_interval = {__rep_ = 3600}
        min_interval = {__rep_ = 10}
        initial_interval = {__rep_ = 60}
        jitter = 0.2
      }
      use_fake_clock = false
      use_memory_observation_store = false
      ```

      You can then see more information about a specific variable with `print`.
      For example:

      ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
      print upload_schedule
      ```

      You should see an output like:

      ```none {:.devsite-disable-click-to-copy}
      {
        target_interval = {__rep_ = 3600}
        min_interval = {__rep_ = 10}
        initial_interval = {__rep_ = 60}
        jitter = 0.2
      }
      ```

    You have successfully loaded and analyzed a minidump for C++ code.

[basic-test-debugging]: /docs/reference/testing/fx-test.md#basic_test_debugging
