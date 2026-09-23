# Flash a Fuchsia image on a device

The [`ffx target flash`][ffx-target-flash] command can flash a Fuchsia image
on a device.

## Concepts

Fuchsia uses a Fastboot-based flashing mechanism to install a Fuchsia product
on a hardware device. The [Fastboot protocol][fastboot-src]{:.external}
(originally part of [Android][android-flash]{:.external}) is a mechanism for
communicating with bootloaders over USB or Ethernet. This mechanism allows you
to flash a Fuchsia prebuilt image on the device's non-volatile memory.

To be able to flash a Fuchsia image on a device, the bootloader of the device
must support Fastboot mode. Once the device can boot into Fastboot
mode, you can then use `ffx target flash` to flash a Fuchsia image on the
device. However, if your device's bootloader doesn't support Fastboot, you'd
first need to update the bootloader. Updating a device's bootloader (to support
Fastboot) typically requires instructions that are specific to the type and
maker of the device, which is not covered in this guide.

Fuchsia prebuilt images can be obtained from various sources, such as Google
Cloud Storage and project repositories. Additionally, custom prebuilt images can
be generated from a Fuchsia source checkout. In either case, the prebuilt image
used for flashing must match the target device.

## Flash the device {:#flash-the-device}

To flash a Fuchsia image on your device, do the following:

1. Connect the device to the host machine over USB or Ethernet.
2. [Boot the device into Fastboot
   mode](#boot-the-device-into-fastboot-mode).

3. Check the device's state:

   ```posix-terminal
   ffx target list
   ```

   This command prints output similar to the following:

   ```none {:.devsite-disable-click-to-copy}
   $ ffx target list
   NAME         SERIAL            TYPE       STATE       ADDRS/IP    RCS
   <unknown>    01234ABCD012YZ    Unknown    Fastboot    []          N
   ```

   Verify that the device's state is `Fastboot`.

   If `ffx target list` shows only a single device in Fastboot mode (as in
   the example above), `ffx` commands automatically target it. However,
   if multiple devices are connected, you need to specify a target device.

4. (**Optional**) Set a default Fuchsia target if multiple devices are present.

   This step is needed if the `ffx target list` command in step 3 shows
   multiple devices and you wish to set a default target for `ffx` commands
   (instead of using the `--target` flag).

   Setting a default Fuchsia target depends on the environment that you're
   working in:

   * {Fuchsia SDK}

     For the Fuchsia SDK environment, use the `FUCHSIA_NODENAME` environment
     variable to set the target:

     ```posix-terminal
     FUCHSIA_NODENAME=<TARGET_DEVICE>
     ```

     For example:

     ```none {:.devsite-disable-click-to-copy}
     $ FUCHSIA_NODENAME=fuchsia-5254-0063-5e7a
     ```

   * {fuchsia.git}

     When working in the Fuchsia source checkout (`fuchsia.git`) environment,
     you can run the following `fx` command to set a default target device
     for your current build directory:

     ```posix-terminal
     fx set-device <TARGET_DEVICE>`
     ```

     For example:

     ```none {:.devsite-disable-click-to-copy}
     $ fx set-device fuchsia-5254-0063-5e7a
     ```

     Note: Having the `FUCHSIA_NODENAME` environment variable set
     overrides the `fx set-device` default target.

     You can run `fx unset-device` to unset the default target.


   Tip: Run `ffx target default get` to verify your default target.

5. Flash the device:

   ```posix-terminal
   ffx target flash -b <FUCHSIA_IMAGE>
   ```

   Replace `FUCHSIA_IMAGE` with an archive file that contains
   a Fuchsia prebuilt image and its flash manifest file, for example:

   ```none {:.devsite-disable-click-to-copy}
   $ ffx target flash -b ~/Downloads/fuchsia-image-example.zip
   ```

   Once the flashing is finished, the device reboots and starts running
   Fuchsia.

## Flash the device using a product bundle from fx create-pb-zip {:#flash-pb-zip}

To flash a product bundle (`.zip`) created from running the `fx create-pb-zip`
command in a Fuchsia source checkout setup, run the `ffx target flash` command
with the `-b` flag:

   ```posix-terminal
   ffx target flash -b <FUCHSIA_IMAGE>
   ```

   Replace `FUCHSIA_IMAGE` with the generated product bundle zip file, for
   example:

   ```none {:.devsite-disable-click-to-copy}
   $ ffx target flash -b pb.zip
   ```

   Once the flashing is finished, the device reboots and starts running
   Fuchsia.

## Boot the device into Fastboot mode {:#boot-the-device-into-fastboot-mode}

To trigger a Fuchsia device to boot into Fastboot mode, run the following
command:

Note: This command works only if your device is already flashed with Fuchsia.
Manually booting a device into Fastboot mode may require instructions
specific to the device.

```posix-terminal
ffx target reboot -b
```

After rebooting, the device boots into Fastboot mode.

<!-- Reference links -->

[fastboot-src]: https://android.googlesource.com/platform/system/core/+/master/fastboot/
[android-flash]: https://source.android.com/setup/build/running
[ffx-target-flash]: https://fuchsia.dev/reference/tools/sdk/ffx#flash
