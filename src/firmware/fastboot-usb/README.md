# fastboot-usb

The component watches for binding of the usb-fastboot-function driver, sends/receives fastboot
packets and process fastboot commands.

## Building

To add this component to your build, append
`--with-base src/firmware/fastboot-usb`
to the `fx set` invocation.

## Running

Use `ffx component run` to launch this component into a restricted realm
for development purposes:

```
$ ffx component run /core/ffx-laboratory:fastboot-usb fuchsia-pkg://fuchsia.com/fastboot-usb#meta/fastboot-usb.cm
```
