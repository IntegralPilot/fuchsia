# adb function driver

This directory contains driver code for adb support. These drivers may be used with
[Fuchsia adbd](https://cs.opensource.google/fuchsia/fuchsia/+/master:src/developer/adb/bin/)
or any other adb daemons, for example a Linux-based adbd running in Starnix.

## How to include adb function driver in a Fuchsia image

To include adb support into the build, add the usb peripheral configuration shown below to platform
assembly configuration and include `adb_support` assembly input bundle.

```bzl
"usb": {
    "peripheral": {
        "functions": [
            "cdc",
            "adb",
        ],
    },
},

```

adb can only be used on boards that support USB peripheral mode.
