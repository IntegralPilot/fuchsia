# bt-rootcanal

bt-rootcanal proxies HCI traffic between bt-host and an external virtual piconet provided by
[Rootcanal](https://source.android.com/docs/setup/create/cuttlefish-connectivity-bluetooth).

When executed, bt-rootcanal will attempt to connect to a host given a specified IP address and
optional port number (default 6402). After a successful connection, bt-rootcanal will spin up
a virtual Bluetooth controller made available to the system and proxy Bluetooth traffic between
the virtual controller and the Rootcanal server.

## How to use

To use this tool, include bt-rootcanal in the build. If testing with Pandora,
include the Pandora interfaces server as well:
```
$ fx set --with //src/connectivity/bluetooth/tools/bt-rootcanal
         --with //src/connectivity/bluetooth/testing/pandora/bt-pandora-server
```

If using FEMU, start with TUN/TAP networking:
```
$ ffx emu start --net tap
$ fx serve
```

Add a test node on which the bt_hci_virtual driver can bind:
```
$ ffx driver test-node add bt-hci-emulator fuchsia.COMPATIBLE=bt
```

Navigate to the root of your Pigweed checkout and activate the Pigweed environment.
E.g.
```
$ cd third_party/pigweed/src
$ source bootstrap.sh
```

Launch the bt_hci_virtual driver:
```
$ bazel run --config=fuchsia pw_bluetooth_sapphire/fuchsia/bt_hci_virtual:pkg.x64.repo.disabled.component
```

Make sure you have a Rootcanal server running. If the Rootcanal server is running
on the same machine as FEMU, by default you can start bt-rootcanal and begin proxying
data with this command:
```
ffx bluetooth pandora start --rootcanal-ip 172.16.243.1
```

Visit go/configure-sapphire-for-pandora for details, including on how to run PTS-bot
tests.
