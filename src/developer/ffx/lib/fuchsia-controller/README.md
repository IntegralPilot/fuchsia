# Fuchsia Controller

The fuchsia-controller Python library is a host library for interacting with
target devices from a host.

## Example

The [example.py](/src/developer/ffx/lib/fuchsia-controller/python/example.py)
program illustrates how to get information from a target with
fuchsia-controller. You can build and run the program with the following
commands:

```sh
fx set minimal.x64 --with-host "//src/developer/ffx/lib/fuchsia-controller:example"
fx build
ffx emu start -H --net user --name 'emulator-a'
ffx emu start -H --net user --name 'emulator-b'
ffx emu start -H --net user --name 'emulator-c'
ffx target list
fx run-in-build host_x64/obj/src/developer/ffx/lib/fuchsia-controller/example.pyz \
  '127.0.0.1:<emulator-a-port>' \
  '127.0.0.1:<emulator-b-port>' \
  '127.0.0.1:<emulator-c-port>'
```