# Start the Fuchsia emulator

This guide provides instructions on how to set up and launch the
Fuchsia emulator (FEMU) on your machine.

The steps are:

1. [Prerequisites](#prerequisites).
1. [Build Fuchsia for FEMU](#build-fuchsia-for-femu).
1. [Enable VM acceleration (Optional)](#enable-vm-acceleration).
1. [Start FEMU](#start-femu).
1. [Discover FEMU](#discover-femu).

## 1. Prerequisites {#prerequisites}

Running FEMU requires that you've completed the following guides:

 * [Download the Fuchsia source code][get-fuchsia-source]
 * [Configure and build Fuchsia][build-fuchsia]

## 2. Build Fuchsia for FEMU {#build-fuchsia-for-femu}

To run FEMU, you first need to build a Fuchsia system image that supports
the emulator environment. This guide uses `x64` for the board and
`workbench_eng` for the product as an example. (For more information on
supported boards and products, see the [Fuchsia emulator][femu-overview]
page.)

Important: The generic `arm64` board configuration is intended for physical
target hardware and does not support running in the emulator. If you are
building for an ARM64 emulator, you must use the `qemu-arm64` board
configuration (for example, `terminal.qemu-arm64`).

To build a FEMU Fuchsia image, do the following:

1. Set the Fuchsia build configuration:

   ```posix-terminal
   fx set workbench_eng.x64 --release
   ```

2. Build Fuchsia:

   ```posix-terminal
   fx build
   ```

## 3. Enable VM acceleration (Optional) {#enable-vm-acceleration}

(**Linux only**) Most Linux machines support VM acceleration through
KVM, which greatly improves the performance and usability of the emulator.

1. If KVM is available on your machine, add yourself to the `kvm` group
   on your machine:

   ```posix-terminal
   sudo usermod -a -G kvm ${USER}
   ```

1. Log out of all desktop sessions to your machine and then log in again.

1. To verify that KVM is configured correctly, run the following command:

   ```posix-terminal
   if [[ -r /dev/kvm ]] && grep '^flags' /proc/cpuinfo | grep -qE 'vmx|svm'; then echo 'KVM is working'; else echo 'KVM not working'; fi
   ```

   Verify that this command prints the following line:

   ```none {:.devsite-disable-click-to-copy}
   KVM is working
   ```

   If you see `KVM not working`, you may need to reboot your machine for
   the permission change to take effect.

## 4. Start FEMU {#start-femu}

### Start the package server

Prior to starting the emulator, start the package server.

To start the package server, run the following command:

```posix-terminal
fx serve
```

Once this command is run, it does not terminate and keeps the
Fuchsia package server running on the host machine (unless the
terminal is closed or `Ctrl+C` is pressed on the terminal,
which kills the command).

Alternatively you can background the `fx serve` process.

### Start the emulator

Start the Fuchsia emulator on your Linux machine.

If your machine is behind a firewall, you may need to apply some
additional configuration to allow the emulator to access the network.
This is typically accomplished by running an "upscript", which sets
up the interfaces and firewall access rules for the current process.
If you're on a corporate network, check with your internal networking
team to see if they have an existing upscript for you to use.

However, even if you're not behind a firewall, there's still some
configuration needed to enable tun/tap networking. The example
upscript at `//scripts/start-unsecure-internet.sh` should work for
the majority of non-corporate users.

To start the emulator, do the following:

1. Start a new terminal.

1. To configure the upscript, run the following command:

   ```posix-terminal
   ffx config set emu.upscript {{ '<var>' }}FUCHSIA_ROOT{{ '</var>' }}/scripts/start-unsecure-internet.sh
   ```

   * `start-unsecure-internet.sh` is an example upscript.
   * `FUCHSIA_ROOT` is the path to your Fuchsia directory.

1. Start the emulator with access to external networks:

   ```posix-terminal
   ffx emu start --net tap
   ```

   `--net` specifies the networking mode for the emulator.
   `--net tap` attaches to a Tun/Tap interface.

   Or, you can start the emulator without access to external networks:

   ```posix-terminal
   ffx emu start --net none
   ```

   Starting the emulator opens a new window with the title
   **Fuchsia Emulator**.

## 5. Discover FEMU {#discover-femu}

To discover the Fuchsia emulator as a running Fuchsia device, run the
following command:

```posix-terminal
ffx target list
```

This command prints output similar to the following:

```none {:.devsite-disable-click-to-copy}
$ ffx target list
NAME                SERIAL       TYPE                 STATE      ADDRS/IP                            RCS
fuchsia-emulator    <unknown>    workbench_eng.x64    Product    [fe80::866a:a5ea:cd9e:69f6%qemu]    N
```

`fuchsia-emulator` is the default node name of the Fuchsia emulator.

The output of `ffx target list` is influenced by the `--net` option in
the following ways:

* `--net none` disables networking, which causes the device to be not
  discoverable by `ffx target list`.

* `--net tap` and `--net user` allow the device to be discoverable
  when running `ffx target list`.

## Next steps

To learn more about Fuchsia device commands and Fuchsia workflows, see
[Explore Fuchsia][explore-fuchsia].

## Appendix

This section provides additional FEMU options.

### Add target manually

If no target is found after running `ffx target list`, you may add the
target manually by running the following `ffx` command:

```posix-terminal
ffx target add {{ "<var>" }}device-ip{{ "</var>" }}:{{ "<var>" }}device-port{{ "</var>" }}
```

### See all available flags

To see a [full list][ffx-emu-reference] of the emulator's supported flags,
run the following command:

```posix-terminal
ffx emu start --help
```

### Run FEMU without GUI support

If you don't need graphics or working under the remote workflow,
you can run FEMU in headless mode:

```posix-terminal
ffx emu start --headless
```
### Reboot FEMU {#reboot-femu}

To reboot FEMU, run the following `ffx` command:

```posix-terminal
ffx target reboot
```

### Stop FEMU {#stop-femu}

To stop FEMU, run the following `ffx` command:

```posix-terminal
ffx emu stop
```

### Configure emulator networking {#configure-emulator-networking}

This section provides instructions on how to configure an IPv6 network
for FEMU on Linux machine using [TUN/TAP][tuntap]{: .external}.

Note: You only need to do this once per machine.

To enable networking in FEMU using
[tap networking][tap-networking]{: .external}, do the following:

1. Set up `tuntap`:

   ```posix-terminal
   sudo ip tuntap add dev qemu mode tap user $USER
   ```

1. Enable the network for `qemu`:

   ```posix-terminal
   sudo ip link set qemu up
   ```

### Specify GPU mode for FEMU (Experimental)

By default, the FEMU launcher uses [SwiftShader][swiftshader]{:.external}
Vulkan ICD for host graphics rendering. With the `--gpu` flag, you can set the
emulator to use the host GPU hardware for rendering. See the following options:

<table><tbody>
  <tr>
   <th>GPU emulation method</th>
   <th>Description</th>
   <th>Flag</th>
  </tr>
  <tr>
   <td>SwiftShader</td>
   <td>Use SwiftShader libraries to simulate GPU processing.
       This is the default mode.</td>
   <td><code>ffx emu start --gpu swiftshader_indirect</code></td>
  </tr>
  <tr>
   <td>Hardware (host GPU)</td>
   <td>Use the host machine's GPU directly to perform GPU processing.</td>
   <td><code>ffx emu start --gpu host</code></td>
  </tr>
  <tr>
   <td>Auto</td>
   <td>Resolve to <code>host</code> if there is a hardware GPU available or
       <code>swiftshader_indirect</code> if there isn't a hardware GPU available.</td>
   <td><code>ffx emu start --gpu auto</code></td>
  </tr>
</tbody></table>

The `host` and `auto` GPU emulation modes are for **experimental use only** and
are not officially supported at the moment. You may see graphics artifacts, testing
failures or emulator crashes when using these two modes.

### Input interaction profiles (mouse versus touchscreen)

Fuchsia virtual device profiles configure different input interaction modes
depending on the target hardware product characteristics:

*   **Mouse Mode (`mouse`)**: Standard desktop mouse emulation pointer layout.
*   **Touchscreen Mode (`touch`)**: Emulates interactive touchscreen input.

**Engine compatibility**:
The choice of the emulator background virtualization engine restricts the
input options:

*   **FEMU (`--engine femu`)**: Fully supports both `mouse` and `touch` input
    profiles. This is the default and is required for testing smart displays or
    touchscreen devices.
*   **QEMU (`--engine qemu`)**: Only supports `mouse` pointer emulation inputs.
    If the selected product bundle defaults to a touchscreen virtual device
    profile layout, launching it under `--engine qemu` triggers capability
    warnings or execution errors.

<!-- Reference links -->

[get-fuchsia-source]: /docs/get-started/get_fuchsia_source.md
[build-fuchsia]: /docs/get-started/build_fuchsia.md
[femu-overview]: /docs/development/build/emulator.md
[ffx-emu-reference]: https://fuchsia.dev/reference/tools/sdk/ffx#emu
[hypervisor-framework]: https://developer.apple.com/documentation/hypervisor
[explore-fuchsia]: /docs/get-started/explore_fuchsia.md
[swiftshader]: https://swiftshader.googlesource.com/SwiftShader/
[tuntap]: https://en.wikipedia.org/wiki/TUN/TAP
[tap-networking]: https://wiki.qemu.org/Documentation/Networking#Tap
[slirp]: https://wiki.qemu.org/Documentation/Networking#User_Networking_.28SLIRP.29
[enable-kvm-acceleration]: /docs/get-started/get_fuchsia_source.md#enable-kvm-acceleration
[configure-emulator-networking]: /docs/get-started/get_fuchsia_source.md#configure-emulator-networking
[get-fuchsia-source-prerequisites]: /docs/get-started/get_fuchsia_source.md#install-prerequisite-packages

