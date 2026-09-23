# thermal-sensor-manager-cli

`thermal-sensor-manager-cli` is a command line developer tool used to query and control the state of the `fuchsia.thermal.SensorManager` protocol exposed by `power-manager`.

Unlike low-level, driver-facing temperature CLI tools (like `temperature-cli`), this tool operates at a higher level (in the `bootstrap` realm) to list sensors, get their current temperature, and set or clear overrides.

## How to Run

This tool is run within the sandbox environment of `power-manager` using the `component explore` command.

1. Launch `component explore` for `power-manager`:
   ```bash
   $ component explore bootstrap/power_manager
   ```

2. Inside the explore shell, run the tool:
   ```bash
   $ thermal-sensor-manager-cli <command> [<args>]
   ```

## Commands and Examples

### 1. List Sensors
Lists all temperature sensors exposed by `SensorManager`.
```bash
$ thermal-sensor-manager-cli list
Sensors found: 3
  charger
  cpu
  usb
```

### 2. Get Temperature
Retrieves the current temperature for a specific sensor.
```bash
$ thermal-sensor-manager-cli read cpu
Sensor 'cpu' temperature: 34.50°C
```

### 3. Set Temperature Override
Sets an override temperature for a specific sensor. This forces `power-manager` to use the overridden value for thermal policy decisions.
```bash
$ thermal-sensor-manager-cli override cpu 75.0
Successfully set override for 'cpu' to 75.00°C
```

### 4. Clear Temperature Override
Clears any temperature override set on a specific sensor, reverting back to reading the real sensor value.
```bash
$ thermal-sensor-manager-cli clear cpu
Successfully cleared override for 'cpu'
```
