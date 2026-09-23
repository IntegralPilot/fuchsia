# `wlanix` integration tests

These tests exercise `wlanix` on real hardware to provide coverage for Fuchsia's
implementation of the Android  WiFi HAL. Basic tests verify functionality that
does not require an access point, and those tests run in
[CI/CQ](https://luci-milo.appspot.com/ui/p/turquoise/test-search?q=vim3_vg_wlanix.sh).
More complex tests require an access point (or multiple). Generally, more
complex tests do not run in CQ, but quick ones run in non-blocking CI.

Each test connects to the `fuchsia.wlan.wlanix/Wlanix` protocol (defined in
//src/connectivity/wlan/wlanix/wlanix.fidl) and makes a series of requests to
drive the behavior of the WLAN system on the device. Since `wlanix` implements
the Android WiFi HAL, these tests generally focus on the types of operations
Android WLAN performs. However, we designed some tests to stress `wlanix` in
ways that Android WLAN may not, e.g., rapidly and repetivitely creating and
destroying an iface, or connecting and disconnecting from the same network.

Failures in more complex cases should refer to these tests as a reference for
expected platform behavior.

> Note: If you have `//vendor/google` in your source tree, you may find
> non-public test targets built from sources in this directory.