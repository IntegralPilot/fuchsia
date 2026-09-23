The Power Manager has a couple of distinct roles related
to device thermal management and discerning "power
profiles".

## Thermal Policy Owner

The Power Manager finds and loads a thermal policy
configuration for the device, and implements actions
following that policy config.

Critically, Power Manager is responsible for watching
temperature, and initiates device shutdown when the device
exceeds a threshold temperature.

It also exposes temperature readings and throttling
behaviors as stats, and may file a crash report.

## Derive and Distribute "Power Profile" from User Activity

For some devices, Power Manager collects user activities to
determine a "power profile", a high-level system status that
it vends out via fuchsia.power.profile.Watcher. It is not
used on Sorrel.
