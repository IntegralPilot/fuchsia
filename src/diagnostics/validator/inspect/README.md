# Inspect Validator

Reviewed on: 2023-05-24

## Puppets

The Rust and C++ puppets live here, under `lib/cpp` and `lib/rust`.

## Puppeteer

The puppeteer for the Inspect Validator is under `src/`. It exercises the various validation
puppets.

## Building

To build everything, append the following to your `fx set`:

```
 --with //src/diagnostics/validator/inspect:tests
```

This will not require an emulator restart.

## Testing

To run the test:
```
fx test inspect-validator-test
```

Or, if set as above in the Building section, `fx test` alone will work.

