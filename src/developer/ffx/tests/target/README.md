# ffx_target_test

This directory contains self-contained & hermetic e2e tests for targets running in emulators. Most importantly it offers coverage of `ffx target flash` to catch flashing issues early.

These tests rely on various components from build outputs:

* Gigaboot (UEFI Bootloader & Fastboot Implementation)
* Zircon
* SSH & Overnet support (nominally `core.x64` build flavor)

## Running Locally

There are not many requirements to run this test locally.

* Build architecture is `x64` and includes SSH and overnet (e.g. `fx set core.x64`)
  * `core.x64` on emulators is known not to work with this test as it does not support flashing
* ffx host tests are included in the build (`fx set <...> --with //src/developer/ffx:tests`)
* TAP/TUN is configured correctly and a running emulator is not using it
  * `sudo ip tuntap add dev qemu mode tap user $USER && sudo ip link set qemu up`

After ensuring all requirements are met, run `fx test ffx_target_test`.

## Debugging

To run a system image in QEMU with flashing support:

Build a disk image using `make-fuchsia-vol`, then rename the EFI partition to `fuchsia-esp`.

```bash
DISK=$FUCHSIA_DIR/out/disk.img
# Create 25GB disk image if it doesn't already exist:
[[ -e $DISK ]] || truncate -s 25G $DISK
# NOTE: If using core.x64-fxfs add --use-fxfs
$FUCHSIA_DIR/out/default/host_x64/make-fuchsia-vol --fuchsia-build-dir $FUCHSIA_DIR/out/default --bootloader $FUCHSIA_DIR/out/default/kernel.efi_x64/fuchsia-efi.efi $DISK
CGPT=$FUCHSIA_DIR/prebuilt/tools/cgpt/linux-x64/cgpt
# Normalize partition names
$CGPT add -i `$CGPT find -t efi -n $DISK` -l fuchsia-esp $DISK
$CGPT add -i `$CGPT find -t DE30CC86-1F4A-4A31-93C4-66F147D33E05 -n $DISK` -l zircon-a $DISK
$CGPT add -i `$CGPT find -t 23CC04DF-C278-4CE7-8471-897D1A4BCDF7 -n $DISK` -l zircon-b $DISK
$CGPT add -i `$CGPT find -t A0E5CF57-2DEF-46BE-A80C-A2067C37CD49 -n $DISK` -l zircon-r $DISK
```

Then use `fx qemu` to start the emulator using the disk:

```bash
fx qemu -a x64 -N --uefi --disktype=nvme -D $DISK
```

In the bootloader, press f at the prompt to interrupt auto-boot and instead enable fastboot.

Once fastboot is enabled it'll show up in ffx.

For example, flash a product bundle:

```bash
# List targets to find the serial.
# Usually fuchsia-5254-0063-5e7a, but change it in subsequent commands.
ffx target list
ffx --target fuchsia-5254-0063-5e7a target flash -b $FUCHSIA_DIR/out/default/obj/build/images/fuchsia/product_bundle/
```

To kill the emulator (if not able to gracefully shutdown via `ffx target off`):

```bash
pkill -HUP qemu
```

## Infra Diagnosis

Other than stdout and stderr the task outputs contain two important log files: `ffx.log` and `emulator.serial.log`

Issues typically happen between host & target. Check `emulator.serial.log` first.
