# Troubleshooting ADB connections to Starnix

This page provides troubleshooting tips for making an ADB connection to Fuchsia
devices.

## Install the adb tool {:#install-adb}

To use `ffx starnix adb` commands, you must have the `adb` tool installed on
your host machine.

To install the `adb` tool:

* **Linux (Debian-based)**: Run the following command:

    ```posix-terminal
    sudo apt install adb
    ```

* **Other platforms**: Download the [Android SDK Platform-Tools][platform-tools]
  and ensure the `adb` binary is in your `PATH`.

## Forwarding ADB ports

If you cannot use the `adb` command to connect to your Fuchsia device over USB,
you can connect to an Android instance running inside Starnix using TCP port
forwarding:

1.  In one terminal, forward a host port to the device's ADB port (`5555`):

    ```posix-terminal
    ffx forward "{{ '<var>' }}HOST_PORT{{ '</var>' }}=>5555"
    ```

    Replace `HOST_PORT` with an available port on your machine
    (for example, `5559`).

    Tip: You can also run this `ffx forward` command in the background
    with the `-q` (quiet) flag, for example: `ffx forward -q "5559=>5555" &`.

2.  In a second terminal, connect the ADB server to that host port:

    ```posix-terminal
    adb connect localhost:{{ '<var>' }}HOST_PORT{{ '</var>' }}
    ```

However, there are a few ways this setup can fail to work, and this page
includes guidance for addressing common issues.

## Make an ADB connection to an emulator

For making an ADB connection to the Fuchsia emulator (FEMU), you can run the
following command instead:

```posix-terminal
ffx starnix adb connect
```

This command enables `adb` to connect to your Fuchsia emulator using TCP and
makes use of a network address provided by `ffx` for the connection.

## Multiple adb devices

When multiple Android devices are connected to your development machine or when
the Android device is running in an emulator, you may see an error from `adb`
that multiple devices are present and it doesn't know which one to use.

To verify if your development machine's ADB server sees multiple devices,
run the following command:

```posix-terminal
adb devices -l
```

If the output lists multiple devices, you need to specify a target device for
your `adb` commands. The identifier for the device connected via port forwarding
is the address you used in the `adb connect` command (for example,
`localhost:5559`).

You can pass this identifier as an argument to your `adb` commands using the
`-s` flag, for example:

```posix-terminal
adb -s localhost:{{ '<var>' }}HOST_PORT{{ '</var>' }} shell ls
```

Alternatively, you can set it as the `ANDROID_SERIAL` environment variable,
which will be used by subsequent `adb` commands:

```posix-terminal
export ANDROID_SERIAL=localhost:{{ '<var>' }}HOST_PORT{{ '</var>' }}
```

Once this environment variable is set, you can use `adb` commands normally.

<!-- Reference links -->

[platform-tools]: https://developer.android.com/tools/releases/platform-tools