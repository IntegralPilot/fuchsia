# bt-snoop-cli

`bt-snoop-cli` uses the `fuchsia.bluetooth.snoop.Snoop` capability to monitor packets sent to &
received from the local controller.

## Build

Include the `bt-snoop-cli` package in your build. For example, if using `fx set`, add:
```
--with //src/connectivity/bluetooth/tools/bt-snoop-cli
```

`bt-snoop-cli` requires `fuchsia.bluetooth.snoop.Snoop` which is provided by the `bt-snoop`
[component](//src/connectivity/bluetooth/tools/bt-snoop/). Make sure it is also included in your
build.

## Run

The tool can be started several ways:

1. In `fx shell`: `fx shell bt-snoop-cli`

2. From the sdk: `fssh bt-snoop-cli`

It is sometimes useful to view snoop logs in a structured protocol viewier like Wireshark. To
forward snoop packets to Wireshark in real time, run:
```
fx shell bt-snoop-cli --format pcap | wireshark -k -i -
```
