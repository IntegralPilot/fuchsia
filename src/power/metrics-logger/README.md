The Metrics Logger component collects power samples
and exposes them as logs, Inspect, and tracing.

On Nelson devices, Metrics Logger collects power and voltage
sensor data from the fleet, via fuchsia.hardware.power.sensor.Device.

It is also the underlying business logic for the command
line tools `ffx profile power logger`,
`ffx profile cpu`, and `ffx profile temperature`.
