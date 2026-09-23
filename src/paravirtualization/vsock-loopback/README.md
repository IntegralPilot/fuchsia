# vsock-loopback

This component provides the ability to connect to VMADDR_CID_LOCAL (ie CID == 1) as well also
serve sockets using that CID. It is used in conjunction with vsock_service with will redirect
connections to it is enabled. In hermetic tests, or scenarios where fuchsia is not running as
a guest VM with a vsock device provided to it, vsock-loopback will also implement a loopback for
VMADDR_CID_HOST (ie CID == 2).

vsock-loopback provides similar functionality to a loopback netdevice implementation, such as
netemul, used in conjunction with netstack for AF_INET sockets, or simply using an AF_UNIX
socket over a differnt address space of ports.

## Building

To add this component to your build, append
`--with src/paravirtualization/vsock-loopback`
to the `fx set` invocation.

## Running

Use `ffx component run` to launch this component into a restricted realm
for development purposes:

```
$ ffx component run /core/ffx-laboratory:vsock-loopback fuchsia-pkg://fuchsia.com/vsock-loopback#meta/vsock-loopback.cm
```

Note that it's already included by default in all emulator based configurations, so it can
alternatively be started via the following command after launching an emulator:

```
$ ffx component start vsock-loopback
```

## Testing

Unit tests for vsock-loopback are available in the `vsock-loopback-tests`
package.

```
$ fx test vsock-loopback-tests
```

