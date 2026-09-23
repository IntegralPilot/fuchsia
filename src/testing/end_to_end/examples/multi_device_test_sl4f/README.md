# Multi-device local testing guide

This README will document the steps needed to setup multiple Fuchsia devices for
Lacewing testing. We will be using two VIM3 devices running `core.vim3` build
and run a Lacewing test that uses both these devices.

Before proceeding further please ensure you have completed the
[Lacewing Getting Started Guide] guide.

## Steps

1. Connect the two VIM3 devices to the host machine.

2. If needed, [build and flash Fuchsia on VIM3s](https://fuchsia.dev/fuchsia-src/development/hardware/khadas-vim3).

3. Stabilize the connection for multi-device setup. For each device, follow the
   below steps
    1. Navigate to Network Settings menu of your Linux host
       ([screenshot](../images/multi_device_4a.png))
    2. Find the USB Ethernet Interface, then click the + (Plus) symbol
       ([screenshot](../images/multi_device_4b.png))
    3. In the New Profile window, in the "Identity" section, Create an unique name; i.e. FuchsiaDevice1
       ([screenshot](../images/multi_device_4c.png))
    4. Switch to "IPv4" tab and select "Disable"
       ([screenshot](../images/multi_device_4d.png))
    5. Switch to "IPv6" tab and select "Link-Local Only"
       ([screenshot](../images/multi_device_4e.png))
    6. Then click Add on the top right.

    If this was done correctly, you will see the newly created profile under
    "USB Ethernet".

    Click on the name of the profile you created. If done correctly, a Checkmark
    will appear next to the new name profile.

4. Afterwards, ensure that this stable connection is complete via
    ```sh
    $ ffx target list
    NAME                      SERIAL            TYPE         STATE      ADDRS/IP                                       RCS
    fuchsia-f80f-f96b-6f59    04140YCABZZ25M    core.vim3    Product    [fe80::4a9c:d65:1e95:999e%enxf80ff96b6f58]     Y
    fuchsia-201f-3b62-e9d3*   1C281F4ABZZ07Z    core.vim3    Product    [fe80::f02f:c160:bfbf:3690%enx201f3b62e9d2]    Y
    ```

5. Finally, determine if you need to provide a local Mobly config yaml file.
    1. If any combination of the connected devices can be used during the test,
       skip this step.
    2. If only specific subsets of connected devices can be used for testing,
       provide a handcrafted local Mobly config YAML file.
       See example below of a local Mobly config YAML.
        ```sh
        Bluetooth_Test.yaml
        ```
        to point our testbeds to those devices.
        ```yaml
        TestBeds:
          - Name: Testbed-One-BT
            Controllers:
              FuchsiaDevice:
                - name: fuchsia-201f-3b62-e9d3
                - name: fuchsia-f80f-f96b-6f59
        ```
        Save this file inside the same folder as the test.
        Finally, update the BUILD.gn that you had created in
        [Lacewing Getting Started Guide]. Below is an example of an updated
        BUILD.gn with the local_config_source for this multi device test
        ```sh
        python_mobly_test("multi_device_test_sl4f") {
          main_source = "test_multi_device.py"

          # The library below provides device interaction APIs.
          libraries = [
            "//src/testing/end_to_end/honeydew",
            "//src/testing/end_to_end/mobly_base_tests/fuchsia_base_test",
          ]
          local_config_source = "Bluetooth_Test.yaml"
          params_source = "params.yaml"
        }
        ```

## Execution

Finally, let's run the test!
```sh
$ fx set core.vim3 \
    --args=core_realm_shards+="[\"//src/testing/sl4f:sl4f_core_shard\"]" \
    --with //src/testing/end_to_end/examples

$ fx test //src/testing/end_to_end/examples/multi_device_test_sl4f --e2e --output
```

[Lacewing Getting Started Guide]: ../../README.md#getting-started-30-mins
