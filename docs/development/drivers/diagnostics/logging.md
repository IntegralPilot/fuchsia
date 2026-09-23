# Driver Logging

You can have a driver send log messages to the
[syslog](/docs/development/diagnostics/logs/recording.md) using the Driver
Framework logging libraries.

## How to write logs

### Add dependencies

* {C++}

  To use the logging library in a C++ driver, add the following dependency to
  your `BUILD.gn`:

  ```gn
  fuchsia_cc_driver("my_driver") {
    deps = [
      "//sdk/lib/driver/logging/cpp",
    ]
  }
  ```

  If your driver uses Bazel, add the dependency to `BUILD.bazel`:

  ```bazel
  fuchsia_cc_driver(
      name = "my_driver",
      deps = [
          "@fuchsia_sdk//pkg/driver_logging_cpp",
      ],
  )
  ```

* {Rust}

  To use logging in a Rust driver, add the `log` crate dependency to your
  `BUILD.gn`:

  ```gn
  fuchsia_rust_driver("my_driver") {
    deps = [
      "//third_party/rust_crates:log",
    ]
  }
  ```

  If your driver uses Bazel, add the dependency to `BUILD.bazel`:

  ```bazel
  fuchsia_rust_driver(
      name = "my_driver",
      deps = [
          "@crate_index//:log",
      ],
  )
  ```

### Write log messages

* {C++}

  In your C++ driver source code, include the logger header:

  ```cpp
  #include <lib/driver/logging/cpp/logger.h>
  ```

  The Driver Framework provides logging functions in the `fdf` namespace
  corresponding to each severity level:

  * `fdf::trace(...)`
  * `fdf::debug(...)`
  * `fdf::info(...)`
  * `fdf::warn(...)`
  * `fdf::error(...)`
  * `fdf::fatal(...)`

  These functions use `std::format`-style string formatting (`{}`):

  ```cpp
  // Basic message
  fdf::info("Initializing driver");

  // Formatted arguments
  fdf::info("Configured device with {} endpoints and {} buffers",
            num_endpoints, num_buffers);

  // Formatting hex or padded values
  fdf::debug("Register address: {:#x}, value: {:02x}", reg_addr, val);
  ```

  **Formatting `zx::result` and `zx::status`**

  Fuchsia provides specialized `std::formatter` implementations for `zx::result`
  and `zx::status`. You can pass these types directly to the format string
  without calling `.status_string()`:

  ```cpp
  zx::result<uint32_t> result = ReadRegister();
  if (result.is_error()) {
    fdf::error("Failed to read register: {}", result);
    return result.take_error();
  }
  ```

* {Rust}

  In your Rust driver source code, import the logging macros from the `log`
  crate:

  ```rust
  use log::{debug, error, info, trace, warn};
  ```

  The Driver Framework initializes the standard `log` facade for the driver
  component, routing logs to `LogSink`. You can use the standard logging macros:

  * `trace!(...)`
  * `debug!(...)`
  * `info!(...)`
  * `warn!(...)`
  * `error!(...)`

  These macros use standard Rust format strings:

  ```rust
  // Basic message
  info!("Initializing driver");

  // Formatted arguments
  info!("Configured device with {num_endpoints} endpoints and {num_buffers} buffers");

  // Formatting hex or padded values
  debug!("Register address: {reg_addr:#x}, value: {val:02x}");

  // Formatting errors or Zircon status
  if let Err(status) = result {
      error!("Failed to read register: {status:?}");
  }
  ```

## How to read logs

### View driver logs

Fuchsia tags driver logs with the driver's moniker and name. You can filter and
view them using `ffx log`:

```posix-terminal
ffx log --filter <driver_name>
```

For more details about filtering, streaming, and querying logs, see
[Viewing logs](/docs/development/diagnostics/logs/viewing.md).

### Log severity levels

The log severities are, in order from most to least severe:

* `FATAL` (C++ only)
* `ERROR`
* `WARN`
* `INFO`
* `DEBUG`
* `TRACE`

By default, the system sends log messages with `INFO` severity and higher to the
[syslog](/docs/development/diagnostics/logs/recording.md#logsinksyslog). The
system suppresses `DEBUG` and `TRACE` severities by default at the producer side
to avoid unnecessary overhead.

### Control log severity

To enable lower severity logs (such as `DEBUG` and `TRACE`), you can adjust the
driver's minimum log level at runtime or at build time.

#### Runtime configuration with `ffx log`

You can dynamically set the minimum log severity for a running driver using
`ffx log --set-severity`:

```posix-terminal
ffx log --set-severity <component_selector>#<SEVERITY>
```

For example, to enable `DEBUG` logs for a driver by moniker:

```posix-terminal
ffx log --set-severity bootstrap/boot-drivers:dev.sys.pci#DEBUG
```

This sends an update request to the component's `LogSettings`, which persists
until you restart the component or update its severity setting again.

#### Build-time configuration with product assembly

To set the initial log level in product assembly, specify
`component_log_initial_interests` in the
[platform configuration][diagnosticsConfig-ref] or through
[developer overrides for product assembly][dev-overrides-docs].

To configure this through developer overrides, add the following to
`//local/BUILD.gn`:

```gn
# //local/BUILD.gn:
import("//build/assembly/developer_overrides.gni")

assembly_developer_overrides("sdhci-debug-logs") {
    platform = {
        diagnostics = {
            component_log_initial_interests = [
                {
                    component = "fuchsia-boot:///sdhci#meta/sdhci.cm"
                    log_severity = "debug"
                },
            ]
        }
    }
}
```

Then run `fx set` with the `--assembly-override` flag:

```posix-terminal
fx set <product.board> --assembly-override=//local:sdhci-debug-logs
```

This sets the minimum log level to `DEBUG`, enabling both `DEBUG` and higher
severity logs.

[diagnosticsConfig-ref]: /reference/assembly/DiagnosticsConfig
[dev-overrides-docs]: /docs/development/build/software_assembly/developer_overrides.md