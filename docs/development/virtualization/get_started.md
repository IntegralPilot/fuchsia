# Getting Started with Virtualization

Note: Virtualization is not actively supported. While basic functionality is
intended to work, additional support may be hard to obtain. And feature requests
are unlikely to be implemented.

## Available hardware

Virtualization is available on Intel devices with VMX enabled and ARMv8.0 and
above devices that boot into EL2. Virtualization is *not* available on AMD CPUs
([b/467349022][ref.bug467349022]).
The following hardware will have the best availability:

*   Pixelbook Go m3
*   Intel NUC7 (NUC7i5DNHE)
*   x64 QEMU/Nested VMX

Basic virtualization features will work when using the Fuchsia emulator with
nested VMX enabled. Notably, Vulkan acceleration using virtmagma will not be
available to any guests when running in the Fuchsia emulator.

## Available guests

While arbitrary Linux guests may run on Fuchsia, the following guest
configurations are available out-the-box:

*   Zircon Guest - A minimal Fuchsia system that boots to a Zircon virtcon.
*   Debian Guest - A Debian bookworm guest.
*   Termina Guest - A Linux guest that contains additional features for Vulkan
    and window manager integration, based on the
    [Termina VM][ref.termina]{:.external} from ChromeOS.

Note: Non-Googlers will need to build Debian images themselves. The Debian Guest
package expects the Linux kernel binaries and userspace image to be in
`//prebuilt/virtualization/packages/debian_guest`. While these are provided for
Googlers, external contributors should create them before running fx build by
following the instructions in [debian_guest/README.md][ref.debian_guest_readme].

## Build Fuchsia with virtualization

For each guest operating system, there is a guest manager and a core shard that
must be included in the build.

Firstly, enable virtualization by creating a `//local/BUILD.gn` file in your Fuchsia
checkout. In that file, add an assembly override for enabling virtualization:

```
assembly_developer_overrides("enable_virtualization") {
  platform = {
    virtualization = {
      enabled = true
    }
  }
}
```

Next, configure your build for the desired guest. The {{ '<var>' }}PRODUCT{{ '</var>' }}
is typically `core`, and the {{ '<var>' }}BOARD{{ '</var>' }} is typically one
of `x64`, `chromebook-x64`, `sherlock`.

*   {All Guests}

    `fx set {{ '<var>' }}PRODUCT{{ '</var>' }}.{{ '<var>' }}BOARD{{ '</var>' }} --assembly-override=//local:enable_virtualization --with //src/virtualization/bundles:all_guests`

*   {Debian Guest}

    `fx set {{ '<var>' }}PRODUCT{{ '</var>' }}.{{ '<var>' }}BOARD{{ '</var>' }} --assembly-override=//local:enable_virtualization --with //src/virtualization/bundles:debian`

*   {Zircon Guest}

    `fx set {{ '<var>' }}PRODUCT{{ '</var>' }}.{{ '<var>' }}BOARD{{ '</var>' }} --assembly-override=//local:enable_virtualization --with //src/virtualization/bundles:zircon`

*   {Termina Guest}

    `fx set {{ '<var>' }}PRODUCT{{ '</var>' }}.{{ '<var>' }}BOARD{{ '</var>' }} --assembly-override=//local:enable_virtualization --with //src/virtualization/bundles:termina`

Finally, build Fuchsia:

```
fx build
```

## Launching a Guest from the CLI

You can launch a guest using the `guest` CLI tool. The tool will launch the
guest and then provide access to a virtio-console over stdio.

Note: Running a guest via QEMU on x64 requires KVM. You may also need to enable
nested KVM on your host machine. For Googlers working on Cloudtop environments,
note that you may need a specialist Cloudtop instance for nested virtualization.

### Configure your Host

Note: The following instructions assume a Linux host machine with an Intel
processor. This will not work if your host machine has an AMD processor.

If you are running Fuchsia directly on dedicated hardware (e.g. Sherlock), host
configuration is not necessary. However, most Googlers will want to run via QEMU
on a host workstation. For this use case, you'll need to check if nested
virtualization is enabled with the following command:

```posix-terminal
cat /sys/module/kvm_intel/parameters/nested
```

An output of `Y` indicates nested virtualization is enabled, `0` or `N`
indicates not enabled.

If needed, you can enable nested virtualization until the next reboot:

```posix-terminal
modprobe -r kvm_intel
modprobe kvm_intel nested=1
```

Or you can make the change permanent by adding `options kvm_intel nested=1` to
`/etc/modprobe.d/kvm.conf`.

### Start QEMU

One you have your host machine setup, you can start the Fuchsia Emulator:

```posix-terminal
ffx emu start
```

### Launch Debian Guest:

```none
(fuchsia) $ guest launch debian
Starting Debian
$ uname -a Linux machina-guest 5.10.0-13-amd64 #1 SMP Debian 5.10.106-1 (2022-03-17) x86_64 GNU/Linux
```

{# Allow the '{{{' below: #}

{% verbatim %}

### Launch Zircon Guest

```none
(fuchsia) $ guest launch zircon
Starting zircon
physboot: {{{reset}}}
physboot: {{{module:0:physboot:elf:9f2c4d6615bd603d}}}
physboot: {{{mmap:0x100000:0x14a100:load:0:rwx:0x0}}}
physboot: | Physical memory range                    | Size    | Type
physboot: | [0x0000000000008000, 0x0000000000080000) |    480K | free RAM
physboot: | [0x0000000000100000, 0x00000000001cd000) |    820K | phys kernel image
physboot: | [0x00000000001cd000, 0x000000000024a000) |    500K | free RAM
physboot: | [0x000000000024a000, 0x000000000024a100) |    256B | phys kernel image
physboot: | [0x000000000024a100, 0x000000000024b000) |   3840B | free RAM
…
```

{# Re-enable variable substitution #}

{% endverbatim %}

### Launch Termina Guest

```none
(fuchsia) $ guest launch termina
Starting Termina
…
```

On products with a UI Stack, the debian and zircon guests will also create a
window that displays a virtual framebuffer powered by a virtio-gpu. Input from
that window will also be sent to the guest as a virtual keyboard.

## Integration tests

Machina has a set of integration tests that launch Zircon and Debian guests to
test the VMM, hypervisor, and each of the virtio devices. To run the tests,
first add them to your build:

```posix-terminal
`fx set {{ '<var>' }}PRODUCT{{ '</var>' }}.{{ '<var>' }}BOARD{{ '</var>' }} --with //src/virtualization:tests`
fx build
```

Then run any of the following tests:

```sh
# Tests of the low level hypervisor syscalls:
$ fx test hypervisor_tests

# Basic tests that verify OS boot and basic functionality:
$ fx test virtualization-core-tests

# Test suites focused on specific devices:
$ fx test virtualization-block-tests
$ fx test virtualization-net-tests
$ fx test virtualization-sound-tests
$ fx test virtualization-vsock-tests
$ fx test virtualization-gpu-tests
$ fx test virtualization-input-tests
```

[ref.bug467349022]:
    https://b.corp.google.com/issues/467349022
[ref.debian_guest_readme]:
    https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/src/virtualization/packages/debian_guest/README.md
[ref.termina]:
    https://chromium.googlesource.com/chromiumos/overlays/board-overlays/+/master/project-termina/
