# WLAN MLME

This library implements IEEE 802.11 MLME functions for hardware with SoftMAC
capabilities.

Our SoftMAC MLME is generally instantiated inside the
['wlansoftmac'](../../drivers/wlansoftmac) driver.

## Layout

MLME is divided into two separate implementations, one for
['client STAs'](./rust/src/client/mod.rs) and one for
['APs'](./rust/src/ap/mod.rs). Each implementation is primarily a state
machine (or in the case of AP, a state machine per-client).
