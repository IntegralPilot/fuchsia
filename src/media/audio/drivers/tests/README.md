## Audio driver tests

The audio driver test suites validate implementations of the interfaces
`fuchsia.hardware.audio.Codec`, `fuchsia.hardware.audio.Composite`,
`fuchsia.hardware.audio.Dai`, `fuchsia.hardware.audio.Health`,
`fuchsia.hardware.audio.RingBuffer`, `fuchsia.hardware.audio.StreamConfig` and
`fuchsia.hardware.audio.signalprocessing`.

Because these tests run _non-hermetically_, they generally _cannot_ be run on a
product that includes the `audio_core` or `audio_device_registry` services. For
more detail, see the "Test case groups" section below.

### Test targets
These suites detect, initialize and test all audio device drivers registered
with devfs under `/dev/class/audio-composite` `/dev/class/audio-input`,
`/dev/class/audio-output`, `/dev/class/codec` and `/dev/class/dai`. Drivers are
tested non-hermetically, validating them when running on the hardware for which
they were written.

The suites also create and test a number of `virtual_audio` driver instances,
using default settings. These tests can be disabled with the `--no-virtual`
flag.

The suites also create and test a private instance of the Bluetooth audio
library (see `//src/connectivity/bluetooth/tests/audio-device-output-harness`).
These tests can be disabled by specifying `--no-bluetooth`.

### Test case groups
With the exception of a few `fuchsia.hardware.audio.StreamConfig` methods,
audio drivers are intended to have only a single connected client at any time.
Some products (those with 'user' and 'userdebug' build flavors, although this
applies to their 'eng' variants as well) _demand-start_ either `audio_core` or
`audio_device_registry` early in the boot-up process, triggering audio device
initialization and configuration. This means that on these products,
`audio_core` or `audio_device_registry` will connect to the `RingBuffer`s of
detected audio devices before these tests can run, thus blocking the majority of
test cases from working as expected.

The `audio_driver_tests` suites use generous timeout durations, enabling them to
function correctly even in heavily loaded test execution environments such as a
device emulator instance on a multi-tenant CQ server, or a build compiled with
significant instrumentation (ASAN) or without standard optimizations (debug).
However, audio drivers are part of the real-time data path, and _certain_ test
cases (see position_test.cc) should only be executed in an environment capable
of real-time response.

For these reasons, there are distinct test suites that each execute a subset of
the audio driver test cases suggested above.

### Test suites
`audio_driver_basic_tests` assumes that an audio service is present and running;
it runs only "basic" `StreamConfig`-related tests (see basic_test.cc) that can
execute even when `audio_core` is already connected to a `StreamConfig` driver.

`audio_driver_admin_tests` runs "privileged" test cases that (1) reconfigure
devices without restoring the previous state, or (2) use interfaces such as
RingBuffer that can only be allocated to one client at a time (and thus cannot
be validated by "basic" tests that assume `audio_core`/`audio_device_registry`
is already present and running). Note: audio services can be _manually_ demand-
started on `core`, `minimal` or other builds that include but do not auto-start
them; if for any reason `audio_core` or `audio_device_registry` is running, the
"admin" tests will fail (first with error code `ZX_ERR_PEER_CLOSED`, followed by
error code `ZX_ERR_INVALID_ARGS`).

`audio_driver_realtime_tests` runs the small subset of test cases that require
real-time response (e.g. ring-buffer position tests). These tests require the
same type of exclusive access needed by "admin" tests.

### Command-line configuration
Each test suite automatically executes one subset of tests, but can enable the
others by specifying cmdline flags when `fx test` is invoked. These flags are
`--basic`, `--admin` and `--position`. The `--all` flag enables all three.

In addition to these flags, there are the `--no-virtual` and `no-bluetooth`
flags mentioned above, as well as `--help` and `--?` which explain all this.

Note: all flags mentioned here are _suite-specific_. Any `fx test` command that
includes them must include an extra `-- ` before these flags.

Thus, to ***fully*** validate a devfs-based audio driver, execute any of the
following on a non-emulated system, running a release build without `audio_core`
or `audio_device_registry` (e.g. core.x64-release, minimal.vim3-release):

```
 fx test audio_driver_basic_tests -- --all
 fx test audio_driver_admin_tests -- --all
 fx test audio_driver_realtime_tests -- --all
```

To further avoid re-testing `virtual_audio` instances or the Bluetooth audio
library, add the two related flags, such as:

```
 fx test audio_driver_realtime_tests -- --basic --admin --no-virtual --no-bluetooth
```
