# Troubleshoot network connectivity: Interface configuration

When diagnosing Fuchsia device network connectivity issues, first check to ensure the following:

- The host machine is correctly enumerating the connected Fuchsia CDC device.
- The host machine is configured correctly.

## Does the host machine see the device? {:#does-the-host-machine-see-the-device}

Determine if the host’s USB stack is aware of the Fuchsia device. If not, there’s no network
interface to configure.

For example, Google’s public USB vendor identifier (VID) is `0x18d1`, which is used by all
Fuchsia USB devices (CDC or otherwise). And Fuchsia devices have a product identifier (PID) in
the range of` 0xa000-0xafff`.

Use the `lsusb` command to determine if the host sees any USB-attached Fuchsia device(s),
which reports a VID:PID tuple for any attached device:

```posix-terminal
lsusb
```

Look for an entry in the output mentioning `Google Inc. CDC Ethernet`.

You may see other results corresponding to other Google products whose PID is outside the range
of Fuchsia PIDs.

### Caveat: PID=0xa025 {:#caveat}

Some product configurations do not expose a CDC interface in the enumerated Fuchsia USB device.
An `adb`-only configuration will have a PID of `0xa025`. If you see a Fuchsia device whose PID is
`0xa025`, it intentionally contains no CDC interface, and thus there’s no corresponding network
interface to configure.

## Is the CDC interface named correctly? {:#is-the-cdc-interface-named-correctly}

Fuchsia CDC network interfaces are named similar to `zx-XXXXXXXXXXXX`, with the actual interface
name representing the interface’s MAC address.

You must have a setup that will detect and name the Fuchsia CDC interfaces properly. Currently the
Fuchsia source tree contains a [Puppet script][puppet-script] that can be run for machines that
support it. Running this script requires that you're using **NetworkManager, Puppet, and
a Debian-like Linux distribution.** If all of this applies to you, you can run this Puppet script
using the following command:

```posix-terminal
curl -s 'https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/scripts/puppet/networkmanager-cfg.pp?format=TEXT' | base64 -d | sudo puppet apply
```

Then you can check the interfaces using the following command:

```posix-terminal
ip -6 --oneline addr | grep 'zx-'
```

This command looks for IPv6 interfaces named with a prefix of `zx-` and print output similar to
the following:

```none {:.devsite-disable-click-to-copy}
$ ip -6 --oneline addr | grep 'zx-'
7: zx-c863147051da    inet6 fe80::ca63:14ff:fe70:51da/64 <snip>
```

## Was the interface assigned the Fuchsia CDC profile? {:#was-the-interface-assigned-the-fuchsia-cdc-profile}

In a nutshell, the local NetworkManager service needs to configure the interface with a link-local
IPv6 address and not attempt DHCP lease acquisition. The configuration exists in the Fuchsia CDC
NetworkManager connection profile. This means that we need to ensure the CDC interface is being
applied the profile.

Use the `nmcli conn show` command to verify that the correct profile was assigned:

```posix-terminal
nmcli conn show
```

In the Fuchsia-CDC profile row, check for the CDC interface:

```none {:.devsite-disable-click-to-copy}
$ nmcli conn show
NAME         UUID                                  TYPE      DEVICE
...
Fuchsia CDC  25b512f5-70e0-4fd3-9bc2-34432a01bc80  ethernet  zx-c863147051da
...
```

You can ignore the UUID field since it is an unimportant opaque identifier.

The Fuchsia CDC profile configures the host-side interface with an IPv6 link-local address,
which are addresses are on the `fe80::/10` network. Verify with the following `ip `command:

```none {:.devsite-disable-click-to-copy}
$ ip -6 --oneline addr | grep 'zx-'
7: zx-c863147051da    inet6 fe80::ca63:14ff:fe70:51da/64 <snip>
```

Here, the address `fe80::ca63:14ff:fe70:51da` is the link-local address of the host-side network
interface. Notably, it is not the address of the connected Fuchsia CDC device.

<!-- Reference links -->

[puppet-script]: https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/scripts/puppet/networkmanager-cfg.pp
