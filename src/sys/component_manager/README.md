# component manager

Reviewed on: 2019-07-12

Component manager is the program that implements the Fuchsia component
framework's runtime. More information about what components are, what semantics
they provide, and how to use them is available at:

-   [//docs/concepts/components](/docs/concepts/components/README.md)
-   [//docs/development/components](/docs/development/components/README.md)

## Building

Component manager should be included in all builds of Fuchsia, but if missing
can be added to builds by including `--with //src/sys/component_manager` to the
`fx set` invocation.

## Running

Component manager runs by default on all Fuchsia builds.

## Testing

To add component manager tests to your build environment, include `--with
//src/sys/component_manager:tests` to your `fx set` invocation. For example:

```sh
$ fx set core.x64 --release --with //src/sys/component_manager:tests
```

To run component manager unit tests:

```sh
$ fx test component_manager_tests
```

For integration tests, see [`tests/`](src/sys/component_manager/tests).

## Source layout

The entrypoint is located in `src/main.rs`, and the core model implementation is
under `src/model/`. Unit tests are co-located with the code, with the exception
of `src/model/` which has unit tests in `src/model/tests/`. Integration tests
live in `tests/`.

## Contributing

See
[Contributing to the Component Framework](/docs/contribute/contributing-to-cf/README.md)
for development best practices in component manager.
