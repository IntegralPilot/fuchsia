# Target List

`ffx target list` lists the visible targets.  Four kinds of targets are listed:

* targets that implement `mDNS`
* targets that are visible on `USB`
* `user`-mode emulator targets
* manual targets, added via `ffx target add`

## Discovery

The fundamental behavior of `ffx target list` is to discover targets. Depending
on configuration options, it gathers the information about targets in one of two
ways, via the daemon, and directly:

### Daemon-based Discovery

When `mDNS` and/or `USB` discovery is enabled (via the
`discovery.mdns.autoconnect` and `fastboot.usb.disabled` configuration options,
respectively), `ffx target list` will ask the `ffx` daemon what targets it has
discovered, and report that information. Whatever is cached by the daemon will
be reported to the user.

### Local Discovery

When both the above discovery options are disabled, `ffx target list` will
perform _local_ discovery: it will do its own `mDNS`, `USB`, emulator, and
manual target discovery. It will actively broadcast `mDNS` requests, and
scan `USB` devices.  Because targets don't always respond to `mDNS` requests
immediately, in this mode `ffx target list` waits for a period (by default, 2000
milliseconds, configurable via `discovery.timeout`), in order to give time for
targets to respond.

## Target Information

The output of `ffx target list` includes information about each discovered
target, including:

* Name
* Serial number
* Type (e.g. `core.x64`)
* State (`Product` or `Fastboot`)
* Addresses (a list of IP addresses)
* Remote control status (whether the Remote Control Service is available)

Depending on the information available, any of these may be listed as "unknown".

### Local Discovery and State Information

When performing local discovery, `ffx target list` must actively probe each
discovered target to determine its state and remote control status. Depending
on various factors, this probe may take multiple seconds, but see below for
controlling this behavior.

## Options

### Nodename

If a nodename is provided, the information given will be restricted to that
device. Note that when local discovery is used, a full `mDNS` query and `USB`
scan is performed to find the named device, but see below for controlling this
behavior.

### Local Discovery Options

The following options only have an effect when performing local discovery:

* `--no-mdns`: do not do an `mDNS` broadcast
* `--no-usb`: do not do a `USB` scan
* `--no-probe`: do not make a connection to targets to probe for their type,
  state, and remote control status

### Filter Options

The output can be restricted by address type:

* `--no-ipv4`: do not return `IPv4` addresses
* `--no-ipv6`: do not return `IPv6` addresses

### Format Options

`ffx target list` can provide the information in a variety of formats. By
default it produces a formatted table. However, the format can be controlled
with the following options:

* `--format simple|s`: tabular format
* `--format tabular|table|tab|t`: tabular format
* `--format addresses|addrs|addr|a`: addresses only
* `--format name-only|n`: names only
* `--format json|JSON|j`: `JSON` format
