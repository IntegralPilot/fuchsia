# Fake-clock

A system enabling a test component to manipulate monotonic and boot time for
components under test.

Fake-clock consists of a library and service component.

The service component maintains a fake monotonic time, which may be accessed
through the [`fuchsia.testing.FakeClock`][fidl] protocol and may be controlled
through the [`fuchsia.testing.FakeClockControl`][fidl] protocol.

The library overrides syscalls defined in the
[vDSO][vdso] that interact with time or have a built in deadline.
The overrides route time-based waits and signals to
the service component via the [`fuchsia.testing.FakeClock`][fidl] protocol.

In addition, the named-timer crate allows an author to annotate critical
deadlines in a component with names. When linked against the fake-clock
library, the deadline set and expired events are reported to the fake-clock
service. A test author may register interest in these events to stop time when
these critical points are reached.

## Use

A test that wishes to use fake-clock must correctly link the library to each
component in the test and route the [`fuchsia.testing.*`][fidl] protocols.

Add the service component as a dependency in the test package.
```
fuchsia_test_package("my-integ-test") {
  test_components = [ "integ-test-component" ]
  deps = [
    ":component_with_fake_time",
    "//src/lib/fake-clock/svc",
  ]
}
```

Link the library against each binary. Since the library is only available to
test targets, create a new version of the binary that depends on
"//src/lib/fake-clock/lib".
```
# Rust binary
rustc_binary("bin_with_fake_time") {
    ...
    testonly = true
    non_rust_deps = [ "//src/lib/fake-clock/lib" ]
}

# C++ binary
executable("bin_with_fake_time") {
    ...
    testonly = true
    deps = [
        ...
        "//src/lib/fake-clock/lib"
    ]
}

# Component with fake time
fuchsia_component("component_with_fake_time") {
    testonly = true
    component_name = "component_with_fake_time"
    manifest "meta/component_with_fake_time.cml"
    deps = [":bin_with_fake_time"]
}
```

Include the fake-clock shard in the manifest for each component with fake time.
```
    ...
    "include": [
        "//src/lib/fake-clock/lib/client.shard.cml"
    ],
```

Add the [`fuchsia.testing.FakeClockControl`][fidl] protocol to the test
component's manifest.
```
    ...
    "use": [
        {
            "protocol": [
                "fuchsia.testing.FakeClockControl"
            ]
        }
    ]
```

The test may then connect to
[`fuchsia.testing.FakeClockControl`][fidl] to control monotonic or boot time.

See the [examples][examples] directory for example setups.

## Design Invariants

When using `FakeClockControl` to manipulate time, it's important to be aware of the following
invariants to avoid putting the system into an inconsistent state:

*   **Advance of Monotonic Clock Advances the Boot Clock:** The `Advance` method
    in `FakeClockControl` takes a single increment which is applied to both
    clocks equally. This prevents the monotonic clock from moving forward
    independently of the boot clock.

*   **Mono-to-Boot Offset is Always Non-Negative and Never Decreases:** The
    offset between the boot and monotonic clocks (`boot_time - monotonic_time`)
    must obey these two invariants. The FIDL API offers
    `IncrementMonoToBootOffsetBy` to allow advances on the boot clock
    independently of the monotonic clock.

Adhering to these invariants is crucial for maintaining a consistent and predictable timeline
during testing.

## Boot Clock Support

In addition to the monotonic clock, the fake-clock library provides full
support for `ZX_CLOCK_BOOT`. Tests can create, set, and cancel boot timers, and
the fake-clock service will correctly manage their lifecycle.

### How it Works

The library intercepts the `zx_timer_create` syscall. When a timer is created,
its handle and the specified clock type (`ZX_CLOCK_MONOTONIC` or
`ZX_CLOCK_BOOT`) are stored in a client-side map. Subsequent calls like
`zx_timer_set` use this map to determine which clock the timer belongs to and
forward the request to the correct implementation in the fake-clock service.

### Example Usage

Here is a simple example of creating and waiting on a boot timer in a test:

```cpp
TEST_F(FakeClockTest, timer_fire_boot) {
  auto [t1_boot, _] = GetTime();
  auto deadline = t1_boot + zx::msec(500);
  zx::timer timer;
  ASSERT_OK(zx::timer::create(0, ZX_CLOCK_BOOT, &timer));
  ASSERT_OK(timer.set(deadline, /*slack*/zx::msec(10)));
  Advance(zx::msec(500), zx::msec(600));
  zx_signals_t signals;
  ASSERT_OK(timer.wait_one(ZX_TIMER_SIGNALED, zx::time::infinite(), &signals));
  ASSERT_EQ(signals, ZX_TIMER_SIGNALED);
}
```

## Troubleshooting

### "UTC clock may interact in unexpected ways with the fake-clock library"

If your test binary crashes with this message, this means that your tests are
attempting to use the `fake-clock` library together with a real UTC
clock. This can happen even if your code does not explicitly get the UTC clock
handle, for example if one of your dependencies does so instead.

Having this happen in your `fake-clock` test is not a good idea. Your tests
may be subtly wrong as a result of the interaction of the fake monotonic clock
and the real UTC clock, depending on whether there is a code path in which this
interaction is important.

You should make a choice that determines what happens next. Your options to
fix this are:

1. Use the [Timekeeper Test Realm Factory][ttrf] to spin up a [Test Realm][tr]
   that wires all the clock-related dependencies properly. It is slightly more
   involved to set up, but will work correctly.

2. Use the dependency `//src/lib/fake-clock/lib:lib_with_utc` instead of
   `//src/lib/fake-clock/lib`. This dependency ignores the potential UTC
   problems. To use this you must accept the possibility that your test
   might be subtly broken as a result.

[vdso]: /docs/concepts/kernel/vdso.md
[fidl]: fidl/fake_clock.fidl
[examples]: examples/
[ttrf]: /src/sys/time/testing/realm-proxy/README.md
[tr]: https://fuchsia.dev/fuchsia-src/development/testing/components/test_realm_factory
