# Display Driver Utilities

This library contains building blocks that have proven useful across multiple
display drivers.

New functionality should only be added after it is used by multiple display
drivers. New functionality must have a narrowly scoped API and thorough test
coverage.

## Testing

We aim to have full test coverage for this library, since it will be used by
multiple drivers. The following command runs the tests.

```sh
fx test //src/graphics/display/lib/driver-utils
```
