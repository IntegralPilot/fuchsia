# Virtual audio drivers

Virtual audio drivers provide flexibly configurable audio devices on Fuchsia
systems for testing, validation, and development. They allow automated test
suites and developer tools to exercise the full audio subsystem without requiring
physical audio hardware.

Virtual audio devices are dynamically created, configured, and controlled at
runtime via the [`fuchsia.virtualaudio`](/sdk/fidl/fuchsia.virtualaudio/) FIDL
protocols.

## Architecture and driver versions

Fuchsia provides two virtual audio driver implementations in the source tree:
the modern **`virtual-audio`** driver and the **`virtual-audio-legacy`** driver.

| Feature | Modern Driver (`virtual-audio`) | Legacy Driver (`virtual-audio-legacy`) |
| :--- | :--- | :--- |
| **Driver Framework** | [Driver Framework v2 (DFv2)](/docs/concepts/drivers/driver_framework.md) | DFv1 / Legacy DDK |
| **Source Path** | `//src/media/audio/drivers/virtual-audio/` | `//src/media/audio/drivers/virtual-audio-legacy/` |
| **Package / Component** | `fuchsia-pkg://fuchsia.com/virtual-audio#meta/virtual-audio-driver.cm` | `fuchsia-pkg://fuchsia.com/virtual-audio-legacy#meta/virtual-audio-legacy-driver.cm` |
| **Supported Audio Protocol** | [`fuchsia.hardware.audio.Composite`](composite.md) | Deprecated [`StreamConfig`](streaming.md), [`Dai`](dai.md), and [`Codec`](codec.md) |
| **Data Endpoints** | Ring buffers, DAIs, and Packet Streams | Stream ring buffers, DAI ring buffers |
| **Signal Processing** | [`fuchsia.hardware.audio.signalprocessing`](signal-processing.md) | Gain and plug state controls |
| **Devfs / Node Location** | `/dev/sys/platform/virtual-audio` | `/dev/sys/platform/virtual-audio-legacy` |
| **Primary Clients** | Audio Device Registry, modern pipeline audio tests, DFv2 driver tests | `audio_core`, `audio-driver-ctl`, tests for deprecated protocols |

### Modern virtual audio driver (`virtual-audio`)

The modern virtual audio driver implements the [`fuchsia.hardware.audio.Composite`](composite.md)
protocol in Driver Framework v2.

Key capabilities include:
* **Composite topologies:** Exposes combinations of ring buffer endpoints, DAI
  interconnects, and packet stream endpoints within a single virtual device.
* **Signal processing:** Implements the `fuchsia.hardware.audio.signalprocessing`
  APIs, allowing tests to configure custom processing topologies, gain stages,
  mutes, and equalizer elements.
* **Clock domain configuration:** Supports configuring monotonic or external
  clock domains, as well as dynamic clock rate adjustments to test drift
  compensation and position notifications.
* **Audio Device Registry integration:** Serves as the primary mock and test
  driver for the Audio Device Registry service.

### Legacy virtual audio driver (`virtual-audio-legacy`)

The legacy virtual audio driver is implemented using the legacy DDK (DFv1) and
provides virtual devices implementing deprecated audio protocols:

* **StreamConfig:** Exposes `fuchsia.hardware.audio.StreamConfigConnector` to
  create virtual audio input and output streams (`/dev/class/audio-input` and
  `/dev/class/audio-output`).
* **DAI:** Exposes `fuchsia.hardware.audio.DaiConnector` (`/dev/class/dai`),
  supporting multiple concurrent client connections.
* **Codec:** Exposes `fuchsia.hardware.audio.CodecConnector` (`/dev/class/codec`).

This driver is retained to validate legacy audio components (such as `audio_core`
and `audio-driver-ctl`) and to verify backward compatibility across deprecated
FIDL protocols.

## Control and configuration API

Virtual audio devices are controlled via two primary FIDL protocols defined in
[`//sdk/fidl/fuchsia.virtualaudio`](/sdk/fidl/fuchsia.virtualaudio/virtual_audio.fidl):

1. **`fuchsia.virtualaudio.Control`:**
   * **`GetDefaultConfiguration`:** Retrieves the default template
     `Configuration` for a given device type (`Composite`, `StreamConfig`,
     `Dai`, or `Codec`).
   * **`AddDevice`:** Creates and activates a virtual device instance using a
     specified `Configuration` and binds a `fuchsia.virtualaudio.Device`
     control channel.
   * **`Enable` / `Disable`:** Globally enables or disables virtual audio device
     creation on the system.

2. **`fuchsia.virtualaudio.Device`:**
   * Manages the lifecycle of an active virtual device instance.
   * Receives notifications of format changes, buffer allocations, and position
     updates.
   * Allows tests to dynamically simulate hardware events such as hotplugging,
     unplugging, or adjusting clock rates.
   * Closing the `Device` channel automatically shuts down and removes the
     corresponding virtual device from the system.

### Configurable device properties

Before activating a virtual device via `Control.AddDevice`, clients can
customize properties in the `Configuration` table, including:

* **Device metadata:** `device_name`, `manufacturer_name`, `product_name`, and
  `unique_id`.
* **Format support:** Supported sample formats (e.g. PCM linear 16/24/32-bit,
  float 32-bit), channel counts, and frame rate ranges.
* **Buffer properties:** Minimum and maximum ring buffer sizes, FIFO depth, and
  external delay metrics.
* **Clock settings:** Clock domain specification and rate adjustment parameters.
* **Plug properties:** Hardwired vs. removable state, initial plug state, and
  plug time.

## Testing workflows

### Driver test integration

Test suites (such as `audio_driver_basic_tests` and `audio_driver_admin_tests`)
can use virtual audio drivers to run self-contained device tests without
requiring physical hardware or pre-installed platform bus nodes in system images.

Tests can either:
* Connect to pre-existing virtual audio controller nodes in devfs
  (`/dev/sys/platform/virtual-audio`), or
* Dynamically register ephemeral virtual audio drivers with Driver Runner during
  test fixture setup and unregister them upon completion.

### Golden enumeration considerations

Because virtual audio devices can be created dynamically by test suites running
on a target, board driver host enumeration tests (such as
`driver-host-enumeration-test-*`) specify virtual audio drivers as optional
(prefixed with `?`) in their `golden.json` definitions:

```json
{
    "drivers": [
        "?fuchsia-pkg://fuchsia.com/virtual-audio#meta/virtual-audio-driver.cm"
    ]
}
```

This ensures that enumeration tests succeed both on minimal/clean boots where
virtual audio is absent, and during test runs where virtual audio components are
dynamically active.
