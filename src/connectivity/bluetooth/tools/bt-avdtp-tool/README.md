# bt-avdtp-tool

`bt-avdtp-tool` sends AVDTP commands to a peer that is connected using the
`fuchsia.bluetooth.avdtp.test.PeerManager` protocol.

The primary use of this tool is to provide user prompted commands to a
Fuchsia device under test for passing PTS certification tests.

## Build

Include the A2DP profile and tool in your build. For example, if using `fx set`, add:
```
--with //src/connectivity/bluetooth/profiles/bt-a2dp
--with //src/connectivity/bluetooth/tools/bt-avdtp-tool
```

Look at the [bt-a2dp README](/src/connectivity/bluetooth/profiles/bt-a2dp/README.md) for any additional dependencies that
may be needed.

## PTS Testing

1) Launch `$ bt-cli` and make sure the controller is discoverable using `discoverable`.
2) In a different shell, run: `$ bt-avdtp-tool`.
3) On the PTS machine, run a test. Make sure the device address entered in PTS matches
the device address shown in the `bt-cli` tool.

* To see the available commands and their descriptions, type `help` in the CLI.
* To see how to use a specific command, type `help _CommandName_` in the CLI.
* Note that each avdtp-tool command must be proceeded by a peer id.
* To change program arguments to the A2DP component, update the [default structured configuration file](/src/connectivity/bluetooth/profiles/bt-a2dp/config/default.json5).
For example, to disable initiating outbound connections, set the `initiator_delay` to 0.

This tool is not meant to be used in a production environment; it is sending out-of-band
AVDTP commands to the A2DP component. This can cause A2DP to get into a bad or irrecoverable state.
It is recommended to restart your device regularly to avoid errors.
