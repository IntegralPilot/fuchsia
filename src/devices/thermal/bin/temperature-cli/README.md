# temperature-cli

`temperature-cli` is a command-line tool for interacting with temperature, ADC (Analog-to-Digital
Converter), and trippoint devices in Fuchsia.

## Usage

```
temperature-cli [device] <command> [args...] [<command2> [args2...] ...]
temperature-cli list
temperature-cli read-all
temperature-cli --help
```

`device` can be:
- An absolute path (e.g., `/dev/class/temperature/000` or
  `/svc/fuchsia.hardware.temperature.Service/default`)
- A friendly name (e.g., `soc-thermal`)
- A service instance name/hash (e.g., `a2471f28e36fbe951476bce7910aa396`)

If `device` is omitted, the tool will attempt to resolve it automatically:
- If only one compatible device is found, it will be used.
- If multiple compatible devices are found, you will be prompted to select one.
- For ADC commands, it will scan for ADC devices. For other commands, it will scan for temperature
  devices.

### Command Chaining

Multiple commands can be chained together sequentially (e.g., `trippoint ... wait`). Commands
targeting the same device protocol share the same persistent connection. This is particularly useful
for oneshot trippoints to guarantee they trigger and get handled without a connection drop reset:

```bash
$ temperature-cli LITTLE trippoint 0:above,35 wait
```

Note that _only one device_ can be targeted with each `temperature-cli` invocation.

### Commands & Examples

#### General Commands

*   **`help`** (also `-h`, `--help`): Show the usage help message.
    ```bash
    $ temperature-cli --help
    Usage: temperature-cli [device_path_or_name] <command> [args...]
           temperature-cli list
           temperature-cli --help
    ...
    ```
*   **`list`**: List all temperature device paths and their friendly names.
    ```bash
    $ temperature-cli list
    Found 2 temperature devices:
      soc-thermal          (/svc/fuchsia.hardware.temperature.Service/default)
      pmic-thermal         (/dev/class/temperature/001)
    ```
*   **`read-all`**: List all temperature devices found and read their temperatures.
    ```bash
    $ temperature-cli read-all
    Found 2 temperature devices:
      soc-thermal          (/svc/fuchsia.hardware.temperature.Service/default)
      pmic-thermal         (/dev/class/temperature/001)

    Reading soc-thermal ...
    temperature = 42.500000
    Reading pmic-thermal ...
    temperature = 38.200000
    ```

#### Temperature Device Commands

These commands operate on `fuchsia.hardware.temperature` devices. You can refer to them by their
absolute path or their friendly name (as shown in `list`).

*   **`read`**: Read the current temperature in Celsius.
    *   *By friendly name*:
        ```bash
        $ temperature-cli soc-thermal read
        temperature = 42.500000
        ```
    *   *By absolute path*:
        ```bash
        $ temperature-cli /dev/class/temperature/000 read
        temperature = 42.500000
        ```
    *   *With interactive selection (if no device is specified and multiple exist)*:
        ```bash
        $ temperature-cli read
        Multiple temperature devices found:
          1. soc-thermal (/svc/fuchsia.hardware.temperature.Service/default)
          2. pmic-thermal (/dev/class/temperature/001)
        Select a device (1-2): 1
        Using temperature device: soc-thermal (/svc/fuchsia.hardware.temperature.Service/default/device)
        temperature = 42.500000
        ```
*   **`name`**: Get the sensor's friendly name.
    ```bash
    $ temperature-cli /svc/fuchsia.hardware.temperature.Service/default name
    Sensor Name = soc-thermal
    ```

#### ADC Device Commands

These commands operate on `fuchsia.hardware.adc` devices.

*   **`resolution`**: Get the ADC resolution.
    ```bash
    $ temperature-cli /dev/class/adc/000 resolution
    adc resolution  = 12
    ```
*   **`read`**: Read a raw ADC sample.
    ```bash
    $ temperature-cli /dev/class/adc/000 read
    Value = 2048
    ```
*   **`read-norm`**: Read a normalized ADC sample (range 0.0 to 1.0).
    ```bash
    $ temperature-cli /dev/class/adc/000 read-norm
    Value  = 0.500000
    ```

#### Trippoint Device Commands

These commands operate on `fuchsia.hardware.trippoint` devices.

*   **`trippoint`**: Get or set trippoints.
    *   *Get Trippoints*:
        ```bash
        $ temperature-cli /svc/fuchsia.hardware.trippoint.Service/default/trippoint trippoint
        {
           .index = 0,
           .type = OneshotTempAbove,
           .configuration = OneshotTempAboveTripPoint(85.000000),
        },
        {
           .index = 1,
           .type = OneshotTempBelow,
           .configuration = ClearedTripPoint,
        },
        ```
    *   *Set Trippoints* (format: `index:type,configuration`):
        ```bash
        $ temperature-cli /svc/fuchsia.hardware.trippoint.Service/default/trippoint trippoint 0:above,90.0 1:below,cleared
        Setting trippoints:
        {
           .index = 0,
           .type = OneshotTempAbove,
           .configuration = OneshotTempAboveTripPoint(90.000000),
        },
        {
           .index = 1,
           .type = OneshotTempBelow,
           .configuration = ClearedTripPoint,
        },
        ```
*   **`wait`**: Wait for any trippoint to be triggered.
    ```bash
    $ temperature-cli /svc/fuchsia.hardware.trippoint.Service/default/trippoint wait
    # (blocks until triggered)
    TripPoint indexed 0 was tripped. Measured temperature was 90.500000 C
    ```
*   **`trigger`** (Debug): Manually trigger a trippoint (requires Debug service).
    ```bash
    $ temperature-cli /svc/fuchsia.hardware.trippoint.DebugService/default/debug trigger 1
    ```
