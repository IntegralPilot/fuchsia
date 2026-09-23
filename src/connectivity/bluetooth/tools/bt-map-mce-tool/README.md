# bt-map-mce-tool

`bt-map-mce-tool` is a command-line front end for the [Message Access Profile MCE
API](https://cs.opensource.google/fuchsia/fuchsia/+/main:sdk/fidl/fuchsia.bluetooth.map/mce.fidl?q=mce.fidl&ss=fuchsia).

## Build

1. Define developer overrides to include this tool.

By default, this tool isn't included in the build packages for any of the platforms.
Create a `BUILD.gn` in your `//local` directory of the Fuchsia platform checkout:

e.g. In `${FUCHSIA_DIR}/local/BUILD.gn`:

```gn
import("//build/assembly/developer_overrides.gni")

assembly_developer_overrides("enable_bt_map_mce_tool") {
  shell_commands = [
    {
        package = "bt-map-mce-tool"
        components = ["bt-map-mce-tool"]
    },
  ]
}
```

2. Use `fx set` to include `bt-map-mce-tool` in the build.

Example for smart displays:

```sh
$ fx set smart_display_max_eng.sherlock --release --with //src/connectivity/bluetooth/tools/bt-map-mce-tool --assembly-override '//vendor/google-smart/products/smart/*=//local:enable_bt_map_mce_tool'
```

Example for core/terminal products (i.e. emulators):

```sh
$ fx set core.x64 --release --with //src/connectivity/bluetooth/tools/bt-map-mce-tool --assembly-override '//build/images/fuchsia/*=//local:enable_bt_map_mce_tool'
```

3. Build and ota to your device (device should eb connected to laptop and ssh tunnel should be established between the cloudtop and laptop)

```sh
$ fx build

# You should see a message similar to this during build:
# WARNING!:  Adding the following via developer overrides from: //local:enable_bt_map_mce_tool
#
#   Additional shell command stubs:
#     package: "bt-map-mce-tool"
#       bin/bt-map-mce-tool

$ fx ota
```

## Usage

When you run `bt-map-mce-tool` in `fx shell`, at first it will look like it's "hanging" until a new peer with MAP MSE role is connected. This will be done automatically when you pair the remote peer that supports MAP MSE role with the smart display device:

Before connection:

```sh
$ fx shell
$ bt-map-mce-tool

# Hanging until peer is connected basically.
```

After connection:

```sh
$ fx shell
$ bt-map-mce-tool
$ MESSAGE_ACCESSOR>
```

After which you can type in `help` command to see all the commands that are available with the tool:

```sh
$ fx shell
$ bt-map-mce-tool
MESSAGE_ACCESSOR> help
# All the commands will be listed.
```
