# Bluetooth Profile: DI

This component implements the Device Identification (DI) Profile as specified by the Bluetooth SIG.

## Build Configuration

Add the following to your Fuchsia build to include the component:

`--with //src/connectivity/bluetooth/profiles/bt-device-id`

## Default Device ID Record

`bt-device-id` provides a structured configuration to specify a default DI record that will be
published on component startup.

- If a `vendor_id` of `0xFFFF` is provided, then no default DI record will be published.

- See `bt-device-id.cml` for more documentation.

## Testing

Add the following to your Fuchsia build to include the component unit tests:

`--with //src/connectivity/bluetooth/profiles/bt-device-id:tests`

To run the tests:

```
fx test bt-device-id-tests
```

Add the following to your Fuchsia build to include the component integration tests:

`--with //src/connectivity/bluetooth/profiles/tests/bt-device-id-integration-tests:tests`

To run the integration tests:

```
fx test bt-device-id-integration-tests
```
