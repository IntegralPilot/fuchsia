[#](#) Tutorial: Debug tests using zxdb

This tutorial walks through a debugging workflow using the `fx test` command
and the Fuchsia debugger (`zxdb`).

For additional info on debugging tests using zxdb, see
[Debug tests using zxdb][zxdb-test-doc]

## Understanding test cases {:#understand-test-cases .numbered}

Note: In most test cases, no additional setup is necessary in the code to run
the `fx test` command with `zxdb`.

* {Rust}

  Rust tests are executed by the [Rust test runner][rust-test-runner]. Unlike
  gTest or gUnit runners for C++ tests, the Rust test runner defaults to running
  test cases in parallel. This creates a different experience while using the
  `--break-on-failure` feature. For more information on expectations while
  debugging parallel test processes, see
  [Executing test cases in parallel][zxdb-parallel-tests]. Parallel test
  processes are supported.

  Here is an example based on some sample
  [rust test code][rust-calculator-test-example]:

  Note: This code is a modified version of the original code and is abbreviated
  for brevity.

  ```rust
  #[fuchsia::test]
  async fn add_test() {
      let (proxy, stream) = create_proxy_and_stream::<CalculatorMarker>();

      // Run two tasks: The calculator_fake & the calculator_line method we're interested
      // in testing.
      let fake_task = calculator_fake(stream).fuse();
      let calculator_line_task = calculator_line("1 + 2", &proxy).fuse();
      futures::pin_mut!(fake_task, calculator_line_task);
      futures::select! {
          actual = calculator_line_task => {
              let actual = actual.expect("Calculator didn't return value");
              assert_eq!(actual, 5.0);
          },
          _ = fake_task => {
              panic!("Fake should never complete.")
          }
      };
  }
  ```

  You can add this test target to your build graph with `fx set`:

  ```posix-terminal
  fx set workbench_eng.x64 --with-test //examples/fidl/calculator/rust/client:hermetic_tests
  ```

* {C++}

  By default, the gTest test runner executes test cases serially, so only one
  test failure is debugged at a time. You can execute test cases in parallel
  by adding the `--parallel-cases` flag to the `fx test` command.

  Here is an example based on some sample
  [C++ test code][debug-agent-test-example]:

  Note: This code is abbreviated for brevity.

  ```cpp
  // Inject 1 process.
  auto process1 = std::make_unique<MockProcess>(nullptr, kProcessKoid1, kProcessName1);
  process1->AddThread(kProcess1ThreadKoid1);
  harness.debug_agent()->InjectProcessForTest(std::move(process1));

  // And another, with 2 threads.
  auto process2 = std::make_unique<MockProcess>(nullptr, kProcessKoid2, kProcessName2);
  process2->AddThread(kProcess2ThreadKoid1);
  process2->AddThread(kProcess2ThreadKoid2);
  harness.debug_agent()->InjectProcessForTest(std::move(process2));

  reply = {};
  remote_api->OnStatus(request, &reply);

  ASSERT_EQ(reply.processes.size(), 3u);  // <-- This will fail, since reply.processes.size() == 2
  EXPECT_EQ(reply.processes[0].process_koid, kProcessKoid1);
  EXPECT_EQ(reply.processes[0].process_name, kProcessName1);
  ...
  ```

  You can add this test target to your build graph with `fx set`:

  ```posix-terminal
  fx set workbench_eng.x64 --with-test //src/developer/debug:tests
  ```

## Executing tests {:#execute-tests .numbered}

* {Rust}

  Execute the tests with the `fx test --break-on-failure` command, for example:

  ```posix-terminal
  fx test -o --break-on-failure calculator-client-rust-unittests
  ```

  The output looks like:

  ```none {:.devsite-disable-click-to-copy}
  <...fx test startup...>

  ...
  [RUNNING]       tests::add_test
  [RUNNING]       tests::divide_test
  [RUNNING]       tests::multiply_test
  [PASSED]        parse::tests::parse_expression
  [RUNNING]       tests::pow_test
  [PASSED]        parse::tests::parse_expression_with_negative_numbers
  [RUNNING]       tests::subtract_test
  [PASSED]        parse::tests::parse_expression_with_multiplication
  [PASSED]        parse::tests::parse_expression_with_subtraction
  [PASSED]        parse::tests::parse_expression_with_pow
  [PASSED]        parse::tests::parse_operators
  [01234.052188][218417][218422][<root>][add_test] ERROR: [examples/fidl/calculator/rust/client/src/main.rs(110)] PANIC info=panicked at ../../examples/fidl/calculato
  r/rust/client/src/main.rs:110:17:
  assertion `left == right` failed
    left: 3.0
   right: 5.0
  [PASSED]        parse::tests::parse_expression_with_division
  [PASSED]        tests::multiply_test
  [PASSED]        tests::divide_test
  ...
    👋 zxdb is loading symbols to debug test failure in calculator-client-rust-unittest, please wait.
  ⚠  test failure in calculator-client-rust-unittest, type `frame` or `help` to get started.
     108            actual = calculator_line_task => {
     109                 let actual = actual.expect("Calculator didn't return value");
   ▶ 110                 assert_eq!(actual, 5.0);
     111            },
     112            _ = fake_task => {
  🛑 process 8 calculator_client_bin_test::tests::add_test::test_entry_point::λ(core::task::wake::Context*) • main.rs:110
  [zxdb]
  ```

  Notice that the output from the test is mixed up, this is because
  the rust test runner runs test cases in
  [parallel by default][rust-test-runner-parallel-default]. You can avoid
  this by using the `--parallel-cases` option with `fx test`, for example:
  `fx test --parallel-cases 1 --break-on-failure
  calculator-client-rust-unittests`. This flag is not required for the tools to
  function, but it can be helpful for debugging as it prevents the output of
  multiple tests from being interleaved, making it easier to read.

  With that option, the output looks like this:

  ```none {:.devsite-disable-click-to-copy}
  fx test --parallel-cases 1 -o --break-on-failure calculator-client-rust-unittests

  ...
  [RUNNING]       parse::tests::parse_operators
  [PASSED]        parse::tests::parse_operators
  [RUNNING]       tests::add_test
  [01391.909144][249125][249127][<root>][add_test] ERROR: [examples/fidl/calculator/rust/client/src/main.rs(110)] PANIC info=panicked at ../../examples/fidl/calculato
  r/rust/client/src/main.rs:110:17:
  assertion `left == right` failed
    left: 3.0
   right: 5.0

  Status: [duration: 5.0s]
    Running 1 tests                        [                                                                                                        ]            0.0%
      fuchsia-pkg://fuchsia.com/calculator-client-rust-unittests#meta/calculator-client-rust-unittests.cm                            [0.5s]
  👋 zxdb is loading symbols to debug test failure in calculator-client-rust-unittest, please wait.
  ⚠  test failure in calculator-client-rust-unittest, type `frame` or `help` to get started.
     108            actual = calculator_line_task => {
     109                 let actual = actual.expect("Calculator didn't return value");
   ▶ 110                 assert_eq!(actual, 5.0);
     111            },
     112            _ = fake_task => {
  🛑 process 2 calculator_client_bin_test::tests::add_test::test_entry_point::λ(core::task::wake::Context*) • main.rs:110
  [zxdb]
  ```

* {C++}

  Execute the tests with the `fx test --break-on-failure` command, for example:

  ```posix-terminal
  fx test -o --break-on-failure debug_agent_unit_tests
  ```

  The output looks like:

  ```none {:.devsite-disable-click-to-copy}
  <...fx test startup...>

  Starting: fuchsia-pkg://fuchsia.com/debug_agent_unit_tests#meta/debug_agent_unit_tests.cm (NOT HERMETIC)
  Command: fx ffx test run --realm /core/testing/system-tests --max-severity-logs WARN --break-on-failure fuchsia-pkg://fuchsia.com/debug_agent_unit_tests?hash=3f6d97801bb147034a344e3fe1bb69291a7b690b9d3d075246ddcba59397ac12#meta/debug_agent_unit_tests.cm

  Status: [duration: 30.9s]  [tasks: 3 running, 15/19 complete]
    Running 2 tests                      [                                                                                                     ]           0.0%
  👋 zxdb is loading symbols to debug test failure in debug_agent_unit_tests.cm, please wait.
  ⚠️  test failure in debug_agent_unit_tests.cm, type `frame` or `help` to get started.
    103   remote_api->OnStatus(request, &reply);
    104
  ▶ 105   ASSERT_EQ(reply.processes.size(), 3u);
    106   EXPECT_EQ(reply.processes[0].process_koid, kProcessKoid1);
    107   EXPECT_EQ(reply.processes[0].process_name, kProcessName1);
  🛑 thread 1 debug_agent::DebugAgentTests_OnGlobalStatus_Test::TestBody(debug_agent::DebugAgentTests_OnGlobalStatus_Test*) • debug_agent_unittest.cc:105
  [zxdb]
  ```

## Examining failures {:#examine-failures .numbered}

* {Rust}

  The example contains a test failure, so Rust tests issue an `abort` on
  failure, which `zxdb` notices and reports. `zxdb` also analyzes the call
  stack from the abort and conveniently drop us straight into the source code
  that failed. You can view additional lines of the code from the current frame
  with `list`, for example:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  list
    105         let calculator_line_task = calculator_line("1 + 2", &proxy).fuse();
    106         futures::pin_mut!(fake_task, calculator_line_task);
    107         futures::select! {
    108            actual = calculator_line_task => {
    109                 let actual = actual.expect("Calculator didn't return value");
  ▶ 110                 assert_eq!(actual, 5.0);
    111            },
    112            _ = fake_task => {
    113                panic!("Fake should never complete.")
    114            }
    115         };
    116     }
    117
    118     #[fuchsia::test]
    119     async fn subtract_test() {
    120         let (proxy, stream) = create_proxy_and_stream::<CalculatorMarker>()
  ```

  You can also examine the entire call stack with `frame`, for example:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  frame
    0…12 «Rust library» (-r expands)
    13 std::panicking::begin_panic_handler(…) • library/std/src/panicking.rs:697
    14 core::panicking::panic_fmt(…) • library/core/src/panicking.rs:75
    15 core::panicking::assert_failed_inner(…) • library/core/src/panicking.rs:448
    16 core::panicking::assert_failed<…>(…) • fuchsia-third_party-rust/library/core/src/panicking.rs:403
  ▶ 17 calculator_client_bin_test::tests::add_test::test_entry_point::λ(…) • main.rs:110
    18 core::future::future::«impl»::poll<…>(…) • future/future.rs:133
    19…44 «Polled event in fuchsia::test_singlethreaded» (-r expands)
    45 calculator_client_bin_test::tests::add_test() • main.rs:98
    46 calculator_client_bin_test::tests::add_test::λ(…) • main.rs:99
    47 core::ops::function::FnOnce::call_once<…>(…) • fuchsia-third_party-rust/library/core/src/ops/function.rs:253
    48 core::ops::function::FnOnce::call_once<…>(…) • library/core/src/ops/function.rs:253 (inline)
    49…70 «Rust test startup» (-r expands)
  ```

  Or, when in an asynchronous context, you can use `async-backtrace`, for example:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  async-backtrace
    Task(id = 0)
    └─ calculator_client_bin_test::tests::divide_test::test_entry_point • select_mod.rs:321
       └─ select!
          └─ (terminated)
          └─ calculator_client_bin_test::tests::calculator_fake • main.rs:93
             └─ futures_util::stream::try_stream::try_for_each::TryForEach
    Task(id = 1)
    └─ diagnostics_log::fuchsia::filter::«impl»::listen_to_interest_changes • fuchsia/filter.rs:63
       └─ fidl::client::QueryResponseFut
  ```

  All commands that you run are in the context of frame #17, as indicated by `▶`.
  You can list the source code again with a little bit of additional context:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  list -c 10
    100         let (proxy, stream) = create_proxy_and_stream::<CalculatorMarker>();
    101
    102         // Run two tasks: The calculator_fake & the calculator_line method we're interested
    103         // in testing.
    104         let fake_task = calculator_fake(stream).fuse();
    105         let calculator_line_task = calculator_line("1 + 2", &proxy).fuse();
    106         futures::pin_mut!(fake_task, calculator_line_task);
    107         futures::select! {
    108            actual = calculator_line_task => {
    109                 let actual = actual.expect("Calculator didn't return value");
  ▶ 110                 assert_eq!(actual, 5.0);
    111            },
    112            _ = fake_task => {
    113                panic!("Fake should never complete.")
    114            }
    115         };
    116     }
    117
    118     #[fuchsia::test]
    119     async fn subtract_test() {
    120         let (proxy, stream) = create_proxy_and_stream::<CalculatorMarker>();
  ```

  To find out why the test failed, print out some variables to see what is
  happening. The `actual` frame contains a local variable, which
  should have some strings that were added by calling `write_log` on the
  `log_helper` and `log_helper2` instances and by receiving them with the
  mpsc channel `recv_logs`:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  print actual
    3
  ```

  It seems that the test's expectations are slightly incorrect. It was expected
  that the calculator would return "1 + 2" would be equal to 3, but the test
  expected it to be 5! The calculator returned the right answer but the test
  expectation is incorrect. You can now detach from the failed test case and fix
  the test expectation.

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  detach

  <...fx test output continues...>

  Failed tests: tests::add_test
  11 out of 12 tests passed...

  Test fuchsia-pkg://fuchsia.com/calculator-client-rust-unittests?hash=b105775fa7c39eb67195a09d63be6c4314eeef8e93eb542616c0b5dbda73b8e2#meta/calculator-client-rust-unittests.cm produced unex
  pected high-severity logs:
  ----------------xxxxx----------------
  [09353.731026][1225676][1225678][<root>][add_test] ERROR: [examples/fidl/calculator/rust/client/src/main.rs(110)] PANIC info=panicked at ../../examples/fidl/calculator/rust/client/src/main
  .rs:110:17:
  assertion `left == right` failed
    left: 3.0
   right: 5.0

  ----------------xxxxx----------------
  Failing this test. See: https://fuchsia.dev/fuchsia-src/development/diagnostics/test_and_logs#restricting_log_severity

  fuchsia-pkg://fuchsia.com/calculator-client-rust-unittests?hash=b105775fa7c39eb67195a09d63be6c4314eeef8e93eb542616c0b5dbda73b8e2#meta/calculator-client-rust-unittests.cm completed with res
  ult: FAILED
  The test was executed in the hermetic realm. If your test depends on system capabilities, pass in correct realm. See https://fuchsia.dev/go/components/non-hermetic-tests
  Tests failed.
  ```

  Now you can fix the test by making the following change to the code:

  Note: `-` indicates a removal of a line and `+` indicates an added line.

  ```diff
  - assert_eq!(actual, 5.0);
  + assert_eq!(actual, 3.0);
  ```

  You can now run the tests again:

  ```posix-terminal
  fx test --break-on-failure calculator-client-rust-unittests
  ```

  The output should look like:

  ```none {:.devsite-disable-click-to-copy}
  <...fx test startup...>

  Running 1 tests

  Status: [duration: 13.5s]
    Running 1 tests

  Starting: fuchsia-pkg://fuchsia.com/calculator-client-rust-unittests#meta/calculator-client-rust-unittests.cm
  Command: fx --dir /usr/local/google/home/jruthe/upstream/fuchsia/out/default ffx test run --max-severity-logs WARN --parallel 1 --no-exception-channel --break-on-failure fuchsia-pkg://fuchsia.com/calculator-client-rust-unittests?hash=abc77325b830d25e47d1de85b764f2b7a0d8975269dfc654f3a7f9a6859b851a#meta/calculator-client-rust-unittests.cm

  Running test 'fuchsia-pkg://fuchsia.com/calculator-client-rust-unittests?hash=abc77325b830d25e47d1de85b764f2b7a0d8975269dfc654f3a7f9a6859b851a#meta/calculator-client-rust-unittests.cm'
  [RUNNING]       parse::tests::parse_expression
  [PASSED]        parse::tests::parse_expression
  [RUNNING]       parse::tests::parse_expression_with_division
  [PASSED]        parse::tests::parse_expression_with_division
  [RUNNING]       parse::tests::parse_expression_with_multiplication
  [PASSED]        parse::tests::parse_expression_with_multiplication
  [RUNNING]       parse::tests::parse_expression_with_negative_numbers
  [PASSED]        parse::tests::parse_expression_with_negative_numbers
  [RUNNING]       parse::tests::parse_expression_with_pow
  [PASSED]        parse::tests::parse_expression_with_pow
  [RUNNING]       parse::tests::parse_expression_with_subtraction
  [PASSED]        parse::tests::parse_expression_with_subtraction
  [RUNNING]       parse::tests::parse_operators
  [PASSED]        parse::tests::parse_operators
  [RUNNING]       tests::add_test
  [PASSED]        tests::add_test
  [RUNNING]       tests::divide_test
  [PASSED]        tests::divide_test
  [RUNNING]       tests::multiply_test
  [PASSED]        tests::multiply_test
  [RUNNING]       tests::pow_test
  [PASSED]        tests::pow_test
  [RUNNING]       tests::subtract_test
  [PASSED]        tests::subtract_test

  12 out of 12 tests passed...
  fuchsia-pkg://fuchsia.com/calculator-client-rust-unittests?hash=abc77325b830d25e47d1de85b764f2b7a0d8975269dfc654f3a7f9a6859b851a#meta/calculator-client-rust-unittests.cm completed with res
  ult: PASSED
  Deleting 1 files at /tmp/tmprwdcy73n: ffx_logs/ffx.log
  To keep these files, set --ffx-output-directory.

  Status: [duration: 14.8s] [tests: PASSED: 1 FAILED: 0 SKIPPED: 0]
  ```

* {C++}

  The example contains a test failure, gTest has an option to insert a software
  breakpoint in the path of a test failure, which `zxdb` picked up. `zxdb` has
  also determined the location of your test code based on this, and jumps
  straight to the frame from your test. You can view additional lines of code of
  the current frame with `list`, for example:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  list
    100   harness.debug_agent()->InjectProcessForTest(std::move(process2));
    101
    102   reply = {};
    103   remote_api->OnStatus(request, &reply);
    104
  ▶ 105   ASSERT_EQ(reply.processes.size(), 3u);
    106   EXPECT_EQ(reply.processes[0].process_koid, kProcessKoid1);
    107   EXPECT_EQ(reply.processes[0].process_name, kProcessName1);
    108   ASSERT_EQ(reply.processes[0].threads.size(), 1u);
  ```

  You can see more lines of source code by using `list`'s `-c` flag:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  list -c 10
      95   constexpr uint64_t kProcess2ThreadKoid2 = 0x2;
      96
      97   auto process2 = std::make_unique<MockProcess>(nullptr, kProcessKoid2, kProcessName2);
      98   process2->AddThread(kProcess2ThreadKoid1);
      99   process2->AddThread(kProcess2ThreadKoid2);
    100   harness.debug_agent()->InjectProcessForTest(std::move(process2));
    101
    102   reply = {};
    103   remote_api->OnStatus(request, &reply);
    104
  ▶ 105   ASSERT_EQ(reply.processes.size(), 3u);
    106   EXPECT_EQ(reply.processes[0].process_koid, kProcessKoid1);
    107   EXPECT_EQ(reply.processes[0].process_name, kProcessName1);
    108   ASSERT_EQ(reply.processes[0].threads.size(), 1u);
    109   EXPECT_EQ(reply.processes[0].threads[0].id.process, kProcessKoid1);
    110   EXPECT_EQ(reply.processes[0].threads[0].id.thread, kProcess1ThreadKoid1);
    111
    112   EXPECT_EQ(reply.processes[1].process_koid, kProcessKoid2);
    113   EXPECT_EQ(reply.processes[1].process_name, kProcessName2);
    114   ASSERT_EQ(reply.processes[1].threads.size(), 2u);
    115   EXPECT_EQ(reply.processes[1].threads[0].id.process, kProcessKoid2);
  [zxdb]
  ```

  You can also examine the full stack trace with the `frame` command:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  frame
    0 testing::UnitTest::AddTestPartResult(…) • gtest.cc:5383
    1 testing::internal::AssertHelper::operator=(…) • gtest.cc:476
  ▶ 2 debug_agent::DebugAgentTests_OnGlobalStatus_Test::TestBody(…) • debug_agent_unittest.cc:105
    3 testing::internal::HandleSehExceptionsInMethodIfSupported<…>(…) • gtest.cc:2635
    4 testing::internal::HandleExceptionsInMethodIfSupported<…>(…) • gtest.cc:2690
    5 testing::Test::Run(…) • gtest.cc:2710
    6 testing::TestInfo::Run(…) • gtest.cc:2859
    7 testing::TestSuite::Run(…) • gtest.cc:3038
    8 testing::internal::UnitTestImpl::RunAllTests(…) • gtest.cc:5942
    9 testing::internal::HandleSehExceptionsInMethodIfSupported<…>(…) • gtest.cc:2635
    10 testing::internal::HandleExceptionsInMethodIfSupported<…>(…) • gtest.cc:2690
    11 testing::UnitTest::Run(…) • gtest.cc:5506
    12 RUN_ALL_TESTS() • gtest.h:2318
    13 main(…) • run_all_unittests.cc:20
    14…17 «libc startup» (-r expands)
  [zxdb]
  ```

  Notice that the `▶` points to your test's source code frame, indicating that
  all commands are executed within this context. You can select other frames
  by using the `frame` command with the associated number from the stack trace.

  To find out why the test failed, print out some variables to see what is
  happening. The `reply` frame contains a local variable, which should have been
  populated by the function call to `remote_api->OnStatus`:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  print reply
  {
    processes = {
      [0] = {
        process_koid = 4660
        process_name = "process-1"
        components = {}
        threads = {
          [0] = {
            id = {process = 4660, thread = 1}
            name = "test thread"
            state = kRunning
            blocked_reason = kNotBlocked
            stack_amount = kNone
            frames = {}
          }
        }
      }
      [1] = {
        process_koid = 22136
        process_name = "process-2"
        components = {}
        threads = {
          [0] = {
            id = {process = 22136, thread = 1}
            name = "test thread"
            state = kRunning
            blocked_reason = kNotBlocked
            stack_amount = kNone
            frames = {}
          }
          [1] = {
            id = {process = 22136, thread = 2}
            name = "test thread"
            state = kRunning
            blocked_reason = kNotBlocked
            stack_amount = kNone
            frames = {}
          }
        }
      }
    }
    limbo = {}
    breakpoints = {}
    filters = {}
  }
  ```

  From the output, you can see the `reply` variable has been filled in with some
  information, the expectation is that the size of the `processes` vector should
  be equal to 3. Print the member variable of `reply` to see more information.
  You can also print the size method of that vector (general function calling
  support is not implemented yet):

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  print reply.processes
  {
    [0] = {
      process_koid = 4660
      process_name = "process-1"
      components = {}
      threads = {
        [0] = {
          id = {process = 4660, thread = 1}
          name = "test thread"
          state = kRunning
          blocked_reason = kNotBlocked
          stack_amount = kNone
          frames = {}
        }
      }
    }
    [1] = {
      process_koid = 22136
      process_name = "process-2"
      components = {}
      threads = {
        [0] = {
          id = {process = 22136, thread = 1}
          name = "test thread"
          state = kRunning
          blocked_reason = kNotBlocked
          stack_amount = kNone
          frames = {}
        }
        [1] = {
          id = {process = 22136, thread = 2}
          name = "test thread"
          state = kRunning
          blocked_reason = kNotBlocked
          stack_amount = kNone
          frames = {}
        }
      }
    }
  }
  [zxdb] print reply.processes.size()
  2
  ```

  It seems that the test's expectations are slightly incorrect. You only
  injected 2 mock processes, but the test was expecting 3. You can simply
  update the test to expect the size of the `reply.processes` vector to be
  2 instead of 3. You can now close zxdb with `quit` to then update and fix the
  tests:

  ```none {: .devsite-terminal data-terminal-prefix="[zxdb]" }
  quit

  <...fx test output continues...>

  Failed tests: DebugAgentTests.OnGlobalStatus <-- Failed test case that we debugged.
  175 out of 176 attempted tests passed, 2 tests skipped...
  fuchsia-pkg://fuchsia.com/debug_agent_unit_tests?hash=3f6d97801bb147034a344e3fe1bb69291a7b690b9d3d075246ddcba59397ac12#meta/debug_agent_unit_tests.cm completed with result: FAILED
  Tests failed.


  FAILED: fuchsia-pkg://fuchsia.com/debug_agent_unit_tests#meta/debug_agent_unit_tests.cm
  ```

  Now that you have found the source of the test failure, you can fix the test:

  ```diff
  -ASSERT_EQ(reply.processes.size(), 3u)
  +ASSERT_EQ(reply.processes.size(), 2u)
  ```

  Then, run `fx test`:

  ```posix-terminal
  fx test --break-on-failure debug_agent_unit_tests
  ```

  The output should look like:

  ```none {:.devsite-disable-click-to-copy}
  You are using the new fx test, which is currently ready for general use ✅
  See details here: https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/scripts/fxtest/rewrite
  To go back to the old fx test, use `fx --enable=legacy_fxtest test`, and please file a bug under b/293917801.

  Default flags loaded from /usr/local/google/home/jruthe/.fxtestrc:
  []

  Logging all output to: /usr/local/google/home/jruthe/upstream/fuchsia/out/workbench_eng.x64/fxtest-2024-03-25T15:56:31.874893.log.json.gz
  Use the `--logpath` argument to specify a log location or `--no-log` to disable

  To show all output, specify the `-o/--output` flag.

  Found 913 total tests in //out/workbench_eng.x64/tests.json

  Plan to run 1 test

  Refreshing 1 target
  > fx build src/developer/debug/debug_agent:debug_agent_unit_tests host_x64/debug_agent_unit_tests
  Use --no-build to skip building

  Executing build. Status output suspended.
  ninja: Entering directory `/usr/local/google/home/jruthe/upstream/fuchsia/out/workbench_eng.x64'
  [22/22](0) STAMP obj/src/developer/debug/debug_agent/debug_agent_unit_tests.stamp

  Running 1 test

  Starting: fuchsia-pkg://fuchsia.com/debug_agent_unit_tests#meta/debug_agent_unit_tests.cm (NOT HERMETIC)
  Command: fx ffx test run --realm /core/testing/system-tests --max-severity-logs WARN --break-on-failure fuchsia-pkg://fuchsia.com/debug_agent_unit_tests?hash=399ff8d9871a6f0d53557c3d7c233cad645061016d44a7855dcea2c7b8af8101#meta/debug_agent_unit_tests.cm
  Deleting 1 files at /tmp/tmp8m56ht95: ffx_logs/ffx.log
  To keep these files, set --ffx-output-directory.

  PASSED: fuchsia-pkg://fuchsia.com/debug_agent_unit_tests#meta/debug_agent_unit_tests.cm

  Status: [duration: 16.9s] [tests: PASS: 1 FAIL: 0 SKIP: 0]
    Running 1 tests                      [=====================================================================================================]         100.0%
  ```

  zxdb no longer appears, because you have successfully fixed all of the test
  failures!

[debug-agent-test-example]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/developer/debug/debug_agent/debug_agent_unittest.cc;l=64-148;drc=0ebebaff9f1f0f4b48325c9d63fddda924cd8da7
[rust-calculator-test-example]: https://cs.opensource.google/fuchsia/fuchsia/+/main:examples/fidl/calculator/rust/client/src/main.rs;l=99;drc=97a1774dd7457ffec1ffcebf81e35b7695a4cf54
[rust-test-runner]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/test_runners/rust/
[rust-test-runner-parallel-default]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/sys/test_runners/rust/src/test_server.rs;l=48;drc=906023d3bdbf3d0aeb3b1080b2cdeb316112112f
[zxdb-test-doc]: /docs/development/debugger/tests.md
[zxdb-parallel-tests]: /docs/development/debugger/tests.md#executing-test-cases-in-parallel
