# Driver FIDL multiplexing deprecation

Note: this project has been completed!

## Goal & motivation

The Driver Framework manages the connection between a hardware device and a
client. Historically, this connection has served three FIDL protocols:
- `fuchsia.io/Node`
- `fuchsia.device/Controller`
- the device's actual FIDL protocol

This multiplexing requires that the Driver Framework owns the channel, so that
it can manually serve the Node and Controller protocols. This prevents the
underlying driver from owning the channel, which limits what drivers are able to
do, and prevents drivers from using the standard FIDL wire server objects and
patterns.

This implicit multiplexing also goes against
[FIDL design principles for composition](/docs/contribute/governance/rfcs/0023_compositional_model_protocols.md).
FIDL design principles state that a protocol can be `composed` of multiple
protocols at compile time. However, the Driver Framework is doing this
composition at run time and is unaware of the FIDL protocol being spoken by the
device.

Many clients that interact with devfs will cast their channels from one FIDL
protocol to the other, removing type safety and generally making the code more
difficult to follow.

This project sets out to remove the FIDL multiplexing from the Driver Framework.
This will require updating clients that interact with drivers.

When this project is complete, drivers will be able to own the channels between
themselves and their clients.

## Technical background

Contributors should be familiar with FIDL clients.
Contributors do not need to be familiar with drivers or the driver framework.

## How to help

### Picking a task

Each line in the allowlist represents one
task. These tasks can be picked at will.

There is one allowlist for `fuchsia.io/Node` and one allowlist for
`fuchsia.device/Controller`.

### Doing a task

Unfortunately, this multiplexing is implicit, so there is no good way to
figure out which code relies on this up front. The best tool we have is removing
a line from the allowlist and seeing what breaks.

The easiest way to find components is to first make sure that QEMU boots. If it
doesn't boot, then check and fix any errors. Once QEMU is booting, you can run
the CL through CQ to try and find other errors.

A component that relies on multiplexing will first have to have access to the
devfs path in question. Searching CML files for `dev`, `dev-class`, or
`dev-class-{name}` is a good way to find relevant components.

#### Fixing fuchsia.io/Node

If this multiplexing is being removed, code will break that uses a device as a
file descriptor, or `fd`.

Code that looks like the following will be very suspect:
```
fbl::unique_fd fd;
zx_status_t status = fdio_open3(dev_path, 0, fd.reset_and_get_address());
```

This should be replaced with FIDL-oriented code:
```
zx::result client_end = component::Connect<fidl_my_protocol::Protocol>(dev_path);
```

#### Fixing fuchsia.device/Controller

If this multiplexing is removed, code that connects to `fuchsia.device/Controller` will
break. These locations might be easier to find because it is possible to grep
for uses of this protocol in any component that is broken.

Code that looks like the following will be suspect:
```
component::ConnectAt<fuchsia_device::Controller>(caller.directory(), device_path);
```

The fix is to open the path to the device controller, which is always the path to the device
plus `/device_controller`.

For example this would be the updated code:
```
std::string device_controller_path = device_path + "/device_controller";
component::ConnectAt<fuchsia_device::Controller>(caller.directory(), device_controller_path);
```

This works on both class paths and topological paths, for example:
```
/dev/class/input/000/device_controller
/dev/sys/platform/usb/usb-hid/device_controller
```

You should also be wary of code that casts a channel to the device controller:
```
fidl::WireCall<fuchsia_device::Controller>(channel)->Rebind();
```

You will have to figure out where the code receives `channel` and update that to connect to the
device controller.

Code that casts a single channel back and forth will need to be updated to carry
around two channels.

### Completing a task

Once your CL passes CQ, it should be good to merge! Thank you for your help.

## Examples

* [Updating hidbus](https://fuchsia-review.googlesource.com/c/fuchsia/+/811377)

## Debugging

If you remove a class while there is still code that relies on the multiplexing you will
see the following error message:

```
{$TOPOLOGICAL_PATH} ({$PROTOCOL_ID}): Failed to send message with ordinal={$ORDINAL} to device: FIDL endpoint was unbound due to unexpected message, status: ZX_ERR_NOT_SUPPORTED (-2), detail: unknown ordinal
It is possible that this message relied on deprecated FIDL multiplexing.
For more information see https://fuchsia.dev/fuchsia-src/contribute/open_projects/drivers/fidl_multiplexing
```

The Topological path and the protocol id are good hints as to what device and FIDL protocol are
still relying on the multiplexing.

## Sponsors

Reach out for questions or for status updates:

*   <dgilhooley@google.com>
*   <tamird@google.com>
*   <surajmalhotra@google.com>
