# Build and flash Fuchsia

This document walks through how to build and flash a Fuchsia image
on a hardware device using `fx` commands.

Note: For more information on these commands, run `fx help <COMMAND>`.

## Identify USB drive device path {:#usb-drive-device-path .numbered}

Before you can build and flash Fuchsia on a target device, you first
need to identify the path of your USB drive.

It is recommended to run the command below once with the USB drive
disconnected, then run it again with the USB drive connected to see
the difference.

* {fx tool}

  To check the correct path to your USB drive using the `fx` tool,
  run the following command:

  ```posix-terminal
  fx mkzedboot
  ```

  The `fx` tool is platform agnostic and lists available USB drives.

* {Linux command}

  To check the correct path to your USB drive using a Linux command,
  run the following command:

  ```posix-terminal
  sudo fdisk -l
  ```

Drives are usually in the form `/dev/sd*` such as `/dev/sdc`. Make sure
that you select the drive rather than a specific partition. Forexample,
a specific partition has a number at the end of the path such as
`/dev/sdc1`.

## Build and flash Fuchsia {:#build-and-flash-fuchsia .numbered}

To perform an initial build and flash of a Fuchsia image using the `fx`
tool, do the following:

1.  Set your Fuchsia build configuration:

    ```posix-terminal
    fx set core.x64
    ```

    This configures the build to build the `core` product on a generic x64
    board. For a list of available products and boards, see `fx list-products`
    and `fx list-boards` for lists of available products, respectively.

1.  Build a Fuchsia image:

    ```posix-terminal
    fx build
    ```

    This command builds Zircon and then the rest of Fuchsia.

1.  Build the Zedboot media and install to a USB device target:

    ```posix-terminal
    fx mkzedboot <usb_drive_device_path>
    ```

    For information on obtaining the USB drive device path, see
    [USB drive device path](#usb-drive-device-path).

1.  Attach Zedboot USB drive to your target device and reboot that device.

1.  On your target device, run:

    ```posix-terminal
    install-disk-image init-partition-tables
    ```

1.  From your host, start the bootserver:

    ```posix-terminal
    fx flash
    ```

    The bootserver connects to the target device to upload the Fuchsia
    image and then flashes your target device.

## Rebuild and reflash Fuchsia {:#rebuild-and-reflash-fuchsia .numbered}

To re-deploy Fuchsia using the `fx` tool, do the following:

1.  Ensure that HEAD is in a good state to pull at the
    [build dashboard](https://luci-milo.appspot.com/p/fuchsia).
1.  Fetch the latest code:

    ```posix-terminal
    jiri update
    ```

1.  Build a Fuchsia image:

    ```posix-terminal
    fx build
    ```

    This command builds Zircon and then the rest of Fuchsia.

1.  From your host, start a development package server:

    ```posix-terminal
    fx serve
    ```

1.  Boot your target device without the Zedboot USB attached.

1.  From your host, push updated Fuchsia packages to the target device:

    ```posix-terminal
    fx ota
    ```

    In some cases, if `fx ota` does not complete successfully, consider repaving
    with `fx flash`.

## Troubleshooting

If `fx build` fails, make sure that your `PATH` environment variable is set
correctly.

To check the value of your `PATH` variable:

```posix-terminal
echo $PATH
```

Make that sure that the output of your `PATH` variable is a list of
directories separated by colons. Make sure that none of the directories are
separated by a dot (`.`).

Note: The `fx` script changes the working directory in a way that may create
conflicts between the commands it uses (such as `touch`) and the binaries in
the working directory.

