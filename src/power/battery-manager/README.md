The Battery Manager component surfaces battery info to other components.

It obtains lower-level information from fuchsia.hardware.powersource.Service,
and exposes fuchsia.power.battery.BatteryManager.

For devices that don't have a real battery, Battery Manager can _simulate
battery data_, by exposing fuchsia.power.battery.test.BatterySimulator.

On Sorrel devices, the battery driver does not provide battery info through
fuchsia.hardware.powersource.Service, and instead directly exposes
fuchsia.power.battery.BatteryInfoProvider to the primary consumer.
Hence, on Sorrel, Battery Manager is bypassed and unused.
