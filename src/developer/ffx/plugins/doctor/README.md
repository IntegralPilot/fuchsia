# FFX Doctor Plugin

`ffx doctor` is a tool to diagnose issues with the `ffx` development environment.

It performs a series of checks on the host environment and target devices to identify common configuration and operational problems.

## Code Structure

The plugin is organized into several modular files:

- **[lib.rs](src/lib.rs)**: Contains the main tool entry point (`DoctorTool`), the plugin orchestration logic (`doctor`), and the integration tests.
- **[types.rs](src/types.rs)**: Shared types, enums (e.g., `StepType`), and traits (e.g., `DoctorStepHandler`).
- **[environment.rs](src/environment.rs)**: Environment-related checks (SSH keys, inotify watches, lock files, emulator instances).
- **[target.rs](src/target.rs)**: Target discovery, collection, and individual target diagnostics.
- **[usb.rs](src/usb.rs)**: Checks for the `ffx-usb-driver`.
- **[network.rs](src/network.rs)**: Internal Google network checks (if applicable).
- **[record.rs](src/record.rs)**: Logic for generating doctor records (zip files containing logs and config).

## Build

To build the doctor plugin, build `ffx`:

```bash
fx build src/developer/ffx
```

## Test

Run the unit tests:

```bash
fx test ffx_doctor_tests
```
