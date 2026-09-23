# Fuchsia WLAN Architecture

This directory contains the WLAN stack for Fuchsia. The stack follows a layered architecture, with components interconnected via FIDL.

## Layers

1. **Policy Layer (`wlancfg` or `wlanix`)**:
   - **Role**: High-level management, auto-connect logic, network selection, and saved network storage.
   - **Exclusivity**: Only one of these is present on a given product.
     - **`wlancfg`**: Standard Fuchsia WLAN policy. Implements `fuchsia.wlan.policy`.
     - **`wlanix`**: Provides Android-compatible WiFi APIs for Starnix. Implements `fuchsia.wlan.wlanix`.
   - **Interactions**: Both interact with `wlandevicemonitor` to manage interfaces and access the SME.

2. **Device Management Layer (`wlandevicemonitor`)**:
   - **Role**: Monitors hardware (PHYs), manages the lifecycle of WLAN interfaces, and bridges policy requests to the appropriate SME.
   - **Protocols**: Implements `fuchsia.wlan.device.service.DeviceMonitor`.

3. **Core / Stack Layer (SME & MLME)**:
   - **SME (`src/connectivity/wlan/lib/sme`)**: Shared logic across all hardware. Manages connection state, scanning, and security (RSN).
   - **MLME (`src/connectivity/wlan/lib/mlme`)**:
     - **SoftMAC**: Implemented in host code (`mlme/rust`). Manages the 802.11 state machine.
     - **FullMAC**: Implemented as a translation layer (`mlme/fullmac`). Converts standard MLME commands to vendor-specific `fuchsia.wlan.fullmac` calls.

4. **Driver Layer (`drivers/`)**:
   - **`wlanphy`**: Management of the physical device.
   - **`wlansoftmac`**: Driver that hosts the SoftMAC SME/MLME and implements lower MAC functions.
   - **`wlanif`**: Interface protocol between the stack and the driver.

## Command Flow: Adding a Feature (e.g., APF)

To plumb a new feature through the stack, follow this flow (using Android Packet Filter (APF) as an example):

### 1. FIDL Definitions

Define the new types and methods in the appropriate FIDL libraries. See the classification in [FIDL.md](./FIDL.md) to determine where your changes belong based on stability and layer boundaries.

### 2. Policy Layer (`wlanix` or `wlancfg`)

- **`wlanix`**: Handle requests in `src/connectivity/wlan/wlanix/src/main.rs`. Use an `IfaceManager` trait to forward calls to the SME.
- **`wlancfg`**: Add logic to state machines (e.g., `client/state_machine.rs`) to trigger the feature.

### 3. SME Layer (`src/connectivity/wlan/lib/sme`)

- Update `MlmeRequest` enum in `lib.rs` to include the new command.
- Add methods to `ClientSme` (e.g., `src/client/mod.rs`) to send the `MlmeRequest`.
- Update FIDL request handlers in `src/serve/client.rs` to call the new SME methods.

### 4. MLME Layer (`src/connectivity/wlan/lib/mlme`)

- **FullMAC Translation (`mlme/fullmac`)**:
  - Add conversion functions in `src/convert/mlme_to_fullmac.rs` (MLME -> FullMAC) and `src/convert/fullmac_to_mlme.rs` (FullMAC -> MLME).
  - Update `DeviceOps` trait and `FullmacDevice` in `src/device.rs` to include the new driver calls.
  - Handle the `MlmeRequest` in the main loop (`src/mlme_main_loop.rs`), perform the conversion, and call the driver via `DeviceOps`.

### 5. Driver Layer

- **FullMAC**: Implement the new methods in the vendor driver (e.g., `brcmfmac`).
- **SoftMAC**: Implement logic in the host-side MLME and `wlansoftmac`.

## Key FIDL Protocols

For a detailed list of key FIDL protocols and their stability guarantees, see [FIDL.md](./FIDL.md).

## Testing

- **`src/connectivity/wlan/testing`**: Infrastructure like `wlantap` (virtual driver) and `hw-sim`.
- **`src/connectivity/wlan/tests`**: Integration and E2E tests.
