Raw HCI factory testing tool.

# bt-fct-hci

Utility to send raw HCI test commands to the hardware on the command channel.

This utility opens the bt-hci device's HciTransport protocol, sends the passed
in HCI command, and waits until either an error, event, or expected result code
is returned.

This tool will take over the HciTransport protocol of the bt-transport driver,
which will break bt-host if it is running.

If the bt-host driver isn't disabled or excluded
in the image, it can be disabled by passing `driver.bt_host.disable` to the
[kernel command-line](https://fuchsia.dev/fuchsia-src/reference/kernel/kernel_cmdline?hl=en#drivernamedisable).

On the host machine while configuring set options add:
```
$ fx set --args=dev_bootfs_labels=\[\"//src/connectivity/bluetooth:disable-bt-host\"\] ...
```

Or, if you prefer `fx args`, just add the following line to your args.gn:
```
dev_bootfs_labels = [ "//src/connectivity/bluetooth:disable-bt-host" ]
```

## Instructions

1.  Add the tool to your build configuration.
```
$ fx set ... --with src/connectivity/bluetooth/tools/bt-fct-hci
```

2. Ensure you are serving packages.
```
$ fx serve
```

3. Send a raw HCI command:
```
$ fx shell bt-fct-hci raw 1c fd 01 f5
```
