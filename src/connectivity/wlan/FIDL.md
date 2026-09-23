
# WLAN FIDL Protocols and API Guidelines

## Protocol Classification and Stability

These FIDL definitions are located in `//sdk/fidl` (except where noted).

### Shared by Policy <-> Core <-> Drivers (stable and versioned)
- `fuchsia.wlan.ieee80211`: Shared types that match IEEE 802.11 types.
- `fuchsia.wlan.common`: Shared types for the entire stack.
    - *Caution*: Only types which truly must span the entire stack should be here. We strongly prefer having separate types for our public vs internal interfaces, so we can modify our internal interfaces without breaking clients.

### Shared by Policy <-> Core <-> Drivers (unstable and not versioned)
- `fuchsia.wlan.stats`: Metrics and statistics definitions used across the stack.

### Android <-> Policy (wlanix) (unstable and not versioned)
- `fuchsia.wlan.wlanix`: Android-compatible WiFi API used by Starnix (located in `wlanix` directory).

### Apps <-> Policy (wlancfg) (stable and versioned)
- `fuchsia.wlan.policy`: Policy-level API for Fuchsia applications.
- `fuchsia.wlan.product.deprecatedclient`: Deprecated product-specific functionality, no new usage expected.
- `fuchsia.wlan.product.deprecatedconfiguration`: Deprecated product-specific functionality, no new usage expected.

### Policy (wlancfg) <-> Core (unstable and not versioned)
- `fuchsia.wlan.sme`: APIs for controlling interfaces and other resources owned by Core.
- `fuchsia.wlan.internal`: Internal types used between Policy and Core.
- `fuchsia.wlan.device.service`: Internal service for device monitoring and SME access. Implements `DeviceMonitor`.

### Core Internal (unstable and not versioned)
- `fuchsia.wlan.mlme`: Common interface used by the SME for both SoftMAC and FullMAC.
- `fuchsia.wlan.minstrel`: Minstrel rate selection algorithm.

### Core <-> Drivers (stable and versioned)
- `fuchsia.wlan.driver`: Types which are shared by SoftMAC and FullMAC drivers.

### Core <-> Drivers (unstable and not versioned)
- `fuchsia.wlan.fullmac`: Vendor driver interface for FullMAC devices.
- `fuchsia.wlan.phy`: Vendor driver interface for PHY control.
- `fuchsia.wlan.softmac`: Vendor driver interface for SoftMAC devices.

### Test Only (unstable and not versioned)
- `fuchsia.wlan.tap`: Interface for mocking driver events in hw-sim tests, e.g. sending and receiving packets.

## Error Handling Strategy

Default to `zx.status` for FIDL errors, but never rely on `zx.status` for control flow. When control flow is impacted, introduce custom error types instead, for example like `ConnectResult` in `fuchsia.wlan.sme.fidl`.

It's hard to know (and keep synchronized in code and docstrings) when a return value is intended to have control flow implications. We explicitly address that need by introducing custom error types for the methods that have control flow implications.
