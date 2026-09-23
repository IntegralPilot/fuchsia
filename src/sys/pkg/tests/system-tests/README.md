# System Tests

This is the integration tests for a number of system tests.

## Test Setup

In order to build the system tests, add this to your `fx set`:

```sh
% fx set ... --with //src/sys/pkg:e2e_tests

% fx build
```

Next, you need to authenticate against luci to be able to download build
artifacts. First, authenticate against cipd with:

```sh
% cipd auth-login
```

Next, install chromium's `depot_tools` by following
[these instructions](https://commondatastorage.googleapis.com/chrome-infra-docs/flat/depot_tools/docs/html/depot_tools_tutorial.html).
Then, login to luci by running:

```sh
% cd depot_tools
% ./luci-auth login -scopes "https://www.googleapis.com/auth/userinfo.email https://www.googleapis.com/auth/devstorage.read_write"
...
```

Now you should be able to run the tests against any device that is discoverable
by `ffx target list`.

## Available Tests

### Upgrade Tests

This tests Over the Air (OTA) updates. At a high level, it:

* Downloads downgrade and upgrade build artifacts
* Tells the device to reboot into recovery.
* Paves the device with the downgrade build artifacts (known as version N-1).
* OTAs the to the upgrade build artifacts (known as version N).
* OTAs the device to a variant of the upgrade build artifacts (known as version
  N').
* OTAs back into upgrade build artifacts (version N).

Note that for all tests below, the `--device name` and `--device-hostname
host/ip_addr` can be looked up from the output of `ffx target list`.
Without specifying them, the tests **might not work, especially if multiple
targets are connected**.

The tests can be run with:

```sh
% cd $(fx get-build-dir) && host_x64/system_tests_upgrade \
  --ssh-private-key $(ffx config get ssh.priv | tr -d '"') \
  --ffx-path $(fx get-build-dir)/host-tools/ffx \
  --builder-name fuchsia/global.ci/core.x64-release \
  --fuchsia-build-dir $(fx get-build-dir) \
  --device fuchsia-5254-475e-82ef \
  --device-hostname fe80::e493:5839:b86a:f7f1%qemu
```

This will run through the whole test paving the build to the latest version
available from the specified builder, then OTA-ing the device to the local build
directory.

The Upgrade Tests also support reproducing a specific build. To do this,
determine the build ids from the downgrade and upgrade builds, then run:

```sh
% cd $(fx get-build-dir) && host_x64/system_tests_upgrade \
  --ssh-private-key $(ffx config get ssh.priv | tr -d '"') \
  --ffx-path $(fx get-build-dir)/host-tools/ffx \
  --build-id 123456789... \
  --build-id 987654321... \
  --device fuchsia-5254-475e-82ef \
  --device-hostname fe80::e493:5839:b86a:f7f1%qemu
```

Or you can combine these options:

```sh
% cd $(fx get-build-dir) && host_x64/system_tests_upgrade \
  --ssh-private-key $(ffx config get ssh.priv | tr -d '"') \
  --ffx-path $(fx get-build-dir)/host-tools/ffx \
  --build-id 123456789... \
  --fuchsia-build-dir $(fx get-build-dir) \
  --device fuchsia-5254-475e-82ef \
  --device-hostname fe80::e493:5839:b86a:f7f1%qemu
```

### Reboot Testing

The system tests support running reboot tests, where a device is rebooted a
configurable number of times, or errs out if a problem occurs. This
can be done by running:

```sh
% cd $(fx get-build-dir) && host_x64/system_tests_reboot \
  --ssh-private-key $(ffx config get ssh.priv | tr -d '"') \
  --ffx-path $(fx get-build-dir)/host-tools/ffx \
  --fuchsia-build-dir $(fx get-build-dir) \
  --device fuchsia-5254-475e-82ef \
  --device-hostname fe80::e493:5839:b86a:f7f1%qemu
```

Or if you want to test a build, you can use:

* `--builder-name fuchsia/global.ci/core.x64-release`, to test
  the latest build published by that builder.
* `--build-id 1234...` to test the specific build.

## Running the Tests

When running the system tests, it's helpful to capture the serial logs, and
system logs, and the test output to a file in order to triage any failures. This
is especially handy when cycle testing. To simplify the setup, the system-tests
come with a helper script `run-test` that can setup a `tmux` session
for you. You can run it like this:

```sh
% ${FUCHSIA_DIR}/src/sys/pkg/tests/system-tests/bin/run-test \
  -o ~/logs \
  --tty /dev/ttyUSB0 \
  $(fx get-build-dir)/host_x64/system_tests_upgrade \
  --ffx-path $(fx get-build-dir)/host-tools/ffx \
  --builder-name fuchsia/global.ci/core.x64-release \
  --fuchsia-build-dir $(fx get-build-dir) \
  --device fuchsia-5254-475e-82ef \
  --device-hostname fe80::e493:5839:b86a:f7f1%qemu
```

This will setup a `tmux` with 3 windows, one for the serial session on
`/dev/ttyUSB0`, one for the system logs, and one for the test. All output from
the `tmux` windows will be saved into `~/logs`.

See the `run-test --help` for more options.

## Running the tests locally in the Fuchsia Emulator

The Fuchsia emulator supports running the tests locally. This is fully supported
on x64 only at present. Support for arm64 is work in progress. Follow these
instructions:

Build a supported x64 product with enabled system tests:

```sh
% fx set core.x64 --with //src/sys/pkg:e2e_tests

% fx build
```

Start the emulator in `--uefi` mode, providing valid keys and metadata files for
signing the ZBI (needed for Fuchsia's verified boot). For testing purposes, the
test keys included in the Fuchsia source can be used:

```sh
% ffx emu start --net tap -H --uefi \
  --vbmeta-key \
    third_party/android/platform/external/avb/test/data/testkey_atx_psk.pem \
  --vbmeta-key-metadata \
    third_party/android/platform/external/avb/test/data/atx_metadata.bin
```

### N+1 OTA test

After the emulator has started up, a local "N+1 OTA test". can be performed.
This is helpful to verify that the build which is currently in the `out`
directory does not have a bug that prevents being OTA'ed in the future.
This test creates a no-op change (called OTA N-prime in the log later) and
performs an OTA to this new version:

```sh
% cd $(fx get-build-dir) && host_x64/system_tests_upgrade \
  --ssh-private-key $(ffx config get ssh.priv | tr -d '"') \
  --ffx-path $(fx get-build-dir)/host-tools/ffx \
  --fuchsia-build-dir $(fx get-build-dir) \
  --device fuchsia-5254-475e-82ef \
  --device-hostname fe80::e493:5839:b86a:f7f1%qemu
```

### Last Known Good to local build OTA

Instead of the mentioned N+1 OTA, it is also possible to perform an OTA test
from the LKG (**L**ast **K**known **G**ood) build from the Fuchsia build
infrastructure (if you have successfully authenticated to cipd, see above at
the top of the document). This will verify that the build in the local `out`
directory will successfully boot coming from a previous version.

```sh
% $(fx get-build-dir)/host_x64/system_tests_upgrade \
  --ssh-private-key $(ffx config get ssh.priv | tr -d '"') \
  --ffx-path $(fx get-build-dir)/host-tools/ffx \
  --builder-name fuchsia/global.ci/core.x64-release \
  --fuchsia-build-dir $(fx get-build-dir) \
  --device fuchsia-5254-475e-82ef \
  --device-hostname fe80::e493:5839:b86a:f7f1%qemu
```