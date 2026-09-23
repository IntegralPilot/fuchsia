# Bundles

Bundles are GN group labels that provide common major groups of features.
They can be included into one of the [dependency sets][dep-sets].

When using the `fx set` command, bundles are most commonly added to the
`universe` dependency set by use of the `--with` flag. See [fx build
configuration][fx-build-config] for more information.

More information on the currently available bundles can be found in
[`//bundles`](/bundles/README.md).

## Key bundles

* `tools` contains a broad array of the most common developer tools. This
  includes tools for spawning components from command-line shells, tools for
  reconfiguring and testing networks, making http requests, debugging programs,
  changing audio volume, and so on.
* `tests` causes all test programs to be built. Most test programs can be
  invoked using `run-test-suite` on the device, or via `fx test`.
* `buildbot/*` are the bundles of software included in Fuchsia's infrastructure
  bot runs. These are useful to include in build configuration if you're trying
  to reproduce how Fuchsia's infrastructure builds or runs tests.
* `kitchen_sink` is a target that causes many (not all) other build targets to
  be included. It was created when Fuchsia had many fewer build configurations
  and supported boards, and was intended to be *all* software available in the
  source tree. This is no longer feasible, and so its usefulness as a
  catch-all is limited. It should not be considered maintained, but is being
  left in the tree for now to avoid breaking existing workflows. Note that
  kitchen sink will produce more than 20GB of build artifacts and requires at
  least 2GB of storage on the target device (size estimates from Q1/2019).

[fx-build-config]: /docs/development/build/fx.md#configure-a-build
[dep-sets]: /docs/development/build/software_assembly/build_configuration.md#dependency-sets
