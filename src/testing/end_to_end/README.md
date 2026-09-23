# End to end test framework

[TOC]

Lacewing is Fuchsia's python end-to-end multi-device testing framework.

Lacewing can be used for authoring host-driven E2E tests that interact
with 1 or more Fuchsia devices. Lacewing can be extended to work with
non-Fuchsia devices as well.

Lacewing provide a uniform test writing experience and exposes convenience
methods for Fuchsia device interactions that are ergonomic and robust, with the
ultimate goal of providing scriptable host-side FIDL access to a Fuchsia
device-under-test.

At a high level, the Lacewing test framework consists of three layers of
abstraction:
* Test framework - [Mobly](https://cs.opensource.google/fuchsia/fuchsia/+/main:third_party/mobly/)
* Device Controller - [Honeydew](honeydew/README.md)
* Host-Target Transport - Currently SL4F (eventually [Fuchsia Controller](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/developer/ffx/lib/fuchsia-controller/))

Why the name Lacewing? Lacewings are insects that keep the Fuchsia plant healthy
and free of harmful bugs - which is something that this framework aspires to be.

## Getting Started [30 mins]

Host tests development with the Lacewing framework is simple and fast.

* Leverages widely used Mobly test framework for standardized testbed definition
and multi-device support.
* Fuchsia device interaction logic is built into the Honeydew library.
* Fast edit-compile-test local development workflow via `fx test`.
* Fuchsia infra integration works out-of-the-box (provided a testbed exists in
lab and Swarming).

The following sections walk through the local development process for a basic
single-device Lacewing test case.

### Confirm device connection

Ensure that there's a Fuchsia device that's accessible from the host via:

```sh
$ cd $FUCHSIA_DIR
$ ffx target list
```

If you do not have a physical device handy, refer to the [Fuchsia-Emulator](./honeydew/tests/functional_tests/README.md#Fuchsia-Emulator) section to start an emulator.

### Test directory

Start by creating a test directory:

```sh
$ cd $FUCHSIA_DIR/src/testing/end_to_end/examples
$ mkdir my_test_dir
$ cd my_test_dir
$ touch my_test.py
$ touch BUILD.gn
````

### Mobly Config YAML File

The Lacewing framework uses the open source Mobly test framework for handling
generic E2E test framework features such as testbed specification, device
initialization abstraction, assertions, and logging.

Every Mobly test requires a testbed configuration file to be provided to specify
the device(s) under test (DUTs) that the E2E test will need to interact with.

For local testing, Lacewing generates a default Mobly config based on the host
environment. This is a best-effort algorithm. This generated config file should
serve the majority of in-tree use cases where devices and emulators are
paved/flashed/built directly from Fuchsia.git).

If the default config generation does not fit your use-case, see the
[Local Manual Mobly Config section](#local-manual-mobly-config) to create a YAML
file from scratch and override the default behavior:

```sh
$ touch my_config.yaml
```

NOTE: For infra execution, Mobly config generation is entirely automated by
Lacewing so nothing is required from test authors (besides picking the correct
|environments| in the `BUILD.gn` for test targets to run against).

Multi-device testbeds follows a similar configuration but is out-of-scope of
this guide - Please direct to
[Multi-device setup and execution](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/examples/multi_device_test_sl4f/README.md)

### Lacewing test module

Every Lacewing Mobly test follows the simple scaffolding below:

```py
import logging

import fuchsia_base_test
from mobly import test_runner

_LOGGER = logging.getLogger(__name__)


class MyFirstLacewingTest(fuchsia_base_test.FuchsiaBaseTest):
    async def test_an_async_testcase(self) -> None:
        _LOGGER.info("Running my first Lacewing test...")
        # Test logic goes here.
        # e.g. await self.dut.some_async_api(...)

    def test_an_all_sync_testcase(self) -> None:
        _LOGGER.info("Running my first Lacewing test...")
        # Test logic goes here.
        # e.g. self.dut.some_sync_api(...)

if __name__ == '__main__':
    test_runner.main()
```

The `self.dut` object contains convenient functions for common Fuchsia
device interactions. Fore more information see the
[Exploring the APIs section](#exploring-the-apis).

### Build definition

Now that you have a Lacewing test module you can integrate it with the build
system:

```gn
import("//build/python/python_mobly_test.gni")

python_mobly_test("my_test_target") {
    main_source = "my_test.py"
    libraries = [
      # Honeydew provides device interaction APIs.
      "//src/testing/end_to_end/honeydew",
      # Base class provides common Fuchsia testing setup and teardown logic.
      "//src/testing/end_to_end/mobly_base_tests/fuchsia_base_test",
    ]
}
```

### Local test execution

Once the Lacewing target has been defined, trigger the test to run locally via
`fx test`:

```sh
# Step 1 - Configure build

# 1.a - If Fuchsia-Controller transport.
$ fx set workbench_eng.x64 \
    --with-host //src/testing/end_to_end/examples/my_test_dir:my_test_target

# 1.b - If SL4F transport.
$ fx set core.x64 \
    --with-host //src/testing/end_to_end/examples/my_test_dir:my_test_target \
    --with //src/testing/sl4f \
    --with //src/sys/bin/start_sl4f \
    --args 'core_realm_shards += [ "//src/testing/sl4f:sl4f_core_shard" ]'

# Step 2- Start the emulator, package server and monitor (in separate terminals)
$ fx serve
$ ffx emu stop ; ffx emu start -H --net tap
$ ffx monitor start

# Step 3 - Run the test
$ fx test //src/testing/end_to_end/examples/my_test_dir:my_test_target --e2e \
    --output
```

By default, test logs are deleted after the test run. Users may
override this by supplying `--artifact-output-directory <custom_path>` option
with `fx test` invocation.

You've just written and run your first Lacewing test!

### Infra test execution

To run a Lacewing test in infra, refer to
[Lacewing's user guide for infra test execution](https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/src/tests/end_to_end/#running-an-end_to_end-test-in-infra).

## Further Reading

### Test parameters

To pass parameters to the Lacewing test, you can do so by creating a Mobly Test
Param YAML file:

```sh
$ touch params.yaml
```

The YAML content can be in any organization structure and any common data types:

```yaml
bool_param: True
str_param: some_string
dict_param:
  fld_1: val_1
  fld_2: val_2
list_param:
  - 1
  - 2
  - 3
  - 4
```

After updating the content of this file, update BUILD.gn to include it in the
`python_mobly_test()` target:

```gn
python_mobly_test("my_test") {
    ...
    params_source = "params.yaml"
    ...
}
```

Inside your Lacewing test, access the parameters:

```py
# The params are accessible from any test methods with access to `self`as well,
# not just in setup related methods.
def setup_class(self):
    ...
    my_bool_param = self.user_params["bool_param"]
    my_list_param = self.user_params["list_param"]
    ...
```

### Test output

If your test generates artifacts, it is recommended that you do so under the
`self.log_path` directory.

Similar to `self.user_params`, this is a framework-provided variable that can be
accessed at any point during the test. Storing your output under this directory
ensures your test and its output integrates well if it's ever run in Fuchsia
infra.

```py
my_artifact_path = os.path.join(self.log_path, 'my_artifact')
with open(my_artifact_path, 'w+', encoding="utf8") as file_handle:
    file_handle.write('data')
```

### Exploring the APIs

After creating your first Lacewing test, it's time to add the test logic that
performs all of the interesting host-device-interactions.

The Lacewing framework comes with its built-in API for mediating interactions
with Fuchsia devices.

See the [Honeydew README.md](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/honeydew/README.md) to learn more.

### Existing test examples

See [examples](https://cs.opensource.google/fuchsia/fuchsia/+/main:src/testing/end_to_end/examples/) for working examples of existing Lacewing tests.

### Local Manual Mobly Config

Read on if the default generated Mobly config is insufficient.

```sh
$ touch local/my_config.yaml
````

For simplicity, this is an example of a single Fuchsia device testbed.

NOTE: Multi-device testbeds follows a similar configuration but is out-of-scope
of this guide - stay tuned for the "Multi-device testing" guide.
TODO(https://fxbug.dev/42075357)

```yaml
TestBeds:
  - Name: My_Simple_Testbed
    Controllers:
      FuchsiaDevice:
        - name: $FUCHSIA_NODENAME
```

`$FUCHSIA_NODENAME` need to be manually substituted to fit your
local development environment (More info on this field is in the section
below).

After updating the content of this file, you can pass the absolute path to that
file to your test via the `--config-yaml-path` flag:

```sh
$ fx test //src/testing/end_to_end/examples/my_test_dir:my_test_target \
    --e2e -- --config-yaml-path $FUCHSIA_DIR/local/my_config.yaml
```

You can also update `BUILD.gn` to include it in the `python_mobly_test()`
target:

```gn
python_mobly_test("my_test") {
    ...
    local_config_source = "//local/my_config.yaml"
    ...
}
```

#### $FUCHSIA_NODENAME

`name: $FUCHSIA_NODENAME` is the device-under-test that's accessible from the
host. A quick way to determine what this is in your local environment is to use
`ffx`:

```sh
$ ffx target list
NAME                SERIAL       TYPE             STATE      ADDRS/IP                           RCS
fuchsia-emulator*   <unknown>    core.x64    Product    [fe80::1a1c:ebd2:2db:6104%qemu]    Y
```

The `$FUCSHIA_NODENAME` in the above example would be `fuchsia-emulator`.

NOTE: This may be an emulator or a physical device. If there are multiple
accessible devices, choose only the one to target in the host test.
