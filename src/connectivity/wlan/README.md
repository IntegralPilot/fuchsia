# Fuchsia WLAN

The WLAN stack is split into several layers:
    - [Policy](/src/connectivity/wlan/wlancfg/README.md) implements interfaces that are used by applications running on Fuchsia.
        - [Device Monitor](/src/connectivity/wlan/wlandevicemonitor) manages WLAN devices and interfaces.
    - **Core** consists of several components and libraries:
        - [SME (Station Management Entity)](/src/connectivity/wlan/lib/sme/README.md) implements the 802.11 SME interfaces.
        - [MLME (MAC Sublayer Management Entity)](/src/connectivity/wlan/lib/mlme/README.md) implements the 802.11 MLME functions.
    - [Drivers](/src/connectivity/wlan/drivers/README.md) contains driver implementations for various hardware.

These layers communicate via FIDL protocols defined in [FIDL.md](./FIDL.md).
