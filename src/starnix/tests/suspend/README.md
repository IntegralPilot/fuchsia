# Starnix suspend tests

Integration tests to ensure that Starnix runner behaves correctly when the kernel asks it to
suspend. The test runner uses the runner Manager FIDL directly instead of creating a kernel.

The specific tests here are:

- test_register_wake_watcher: We simulate a HAL registering a wake watcher and verify it
  receives an asleep signal then an awake signal.
- test_wake_lock: We simulate the runner taking a wake lock while suspend is in process and
  check that suspend is correctly cancelled.