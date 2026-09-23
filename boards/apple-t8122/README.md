# Apple M3 (T8122)

This board represents the Apple Silicon M3 chip (identified by Apple as "t8122"). It is currently
a basic, experimental target, with support for serial, interrupts, and timer. There is 
currently only support for a single CPU. 

On this board, `m1n1` (MIT-licenced open-source boot firmware) currently runs as a hypervisor 
in EL2, and Zircon runs in EL1, with userspace in EL0.

This board target has been tested on Apple MacBook Air (J613), however, should work with
any device with the base M3 chip.

## Build

```sh
fx set bringup.apple-t8122 --with //zircon/kernel/arch/arm64/phys/boot-shim:linux-arm64-boot-shim
fx build
```

## Boot flow

When powered on, the board executes "SecureROM" located in Mask ROM on the CPU. After that, Apple's
bootloader, iBoot, is loaded. On MacBook devices, iBoot can be freely configured to boot a custom, 
unsigned boot object.

When booting Fuchsia, MIT-licenced open firmware, called "m1n1" ([available on GitHub](https://github.com/AsahiLinux/m1n1)) takes over from iBoot, and configures the device and a virtual UART. It then loads the guest kernel in EL1 using the Linux ARM64 Boot Protocol.

Fuchsia's [linux-arm64-boot-shim](../../zircon/kernel/arch/arm64/phys/boot-shim/linux-arm64-boot-shim.cc)
takes over from there, and successfuly loads Zircon and Fuchsia.

## Run

You will need to first install [m1n1](https://github.com/AshaiLinux/m1n1) and configure it as the boot object for your target device.

On a host device (which can be any macOS or Linux device), clone m1n1 and add [the fuchsia boot script](https://gist.github.com/IntegralPilot/1ce343c851e463b171ce17bfa8d537bf) to its `proxyclient/tools` folder. The host device should have `dtc` available.

Then, connect both devices by USB, and start the target. On the host, set `M1N1_DEVICE` environment variable to the serial device through which m1n1 connects.

Then, run `python proxyclient/tools/fuchsia.py /path/to/linux-arm64-boot-shim.bin /path/to/fuchsia.zbi` and Fuchsia will start!
