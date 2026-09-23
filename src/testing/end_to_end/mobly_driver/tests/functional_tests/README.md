# Example Mobly Test execution

## Set up
1. Configure example test to be built.
    ```shell
    $ fx set core.x64 \
        --with-host //src/testing/end_to_end/mobly_driver/tests/functional_tests:tests

    $ fx clean-build
    ```

2. Start the package server. Keep this running in the background.
    ```shell
    $ fx serve
    ```

3. Ensure testbeds are detected on host
    ```shell
    # (optional) - If the DUT can be emulated, start an emulator via ffx.
    $ ffx emu start --net tap --headless

    $ ffx target list
    NAME                SERIAL       TYPE             STATE      ADDRS/IP                           RCS
    fuchsia-emulator*   <unknown>    core.x64    Product    [fe80::1a1c:ebd2:2db:6104%qemu]    Y
    ````


## Local mode
The majority of users will be using this local test execution method for test
development.
```shell
$ fx test //src/testing/end_to_end/mobly_driver/tests/functional_tests/smoke_test --e2e --output
$ fx test //src/testing/end_to_end/mobly_driver/tests/functional_tests/test_honeydew_integration_fc --e2e --output
```

## Infra mode
This method of execution is atypical and is only documented for the special case
of debugging and developing the infra_driver.InfraDriver() implementation which
most Mobly test owners will not need to know about.

In order to run a Mobly test locally as if it was executing in Fuchsia's lab
infrastructure, simply set the `FUCHSIA_TESTBED_CONFIG` environment variable to
point to a handcrafted `botanist.json` file.

```shell
$ FUCHSIA_TESTBED_CONFIG=<PATH_TO_BOTANIST_JSON> FUCHSIA_TEST_OUTDIR=/tmp fx test //src/testing/end_to_end/mobly_driver/tests/functional_tests/smoke_test --e2e --output
````

### Example `botanist.json` file

The exact schema of `targetInfo` is defined in //tools/botanist/cmd/run.go.

```json
[
    {
       "type": "FuchsiaDevice",
       "name":"fuchsia-54b2-030e-eb19",
       "ipv4":"192.168.42.112",
       "ipv6":"",
       "serial_socket":"/tmp/fuchsia-54b2-030e-eb19_mux",
    }
]
```
