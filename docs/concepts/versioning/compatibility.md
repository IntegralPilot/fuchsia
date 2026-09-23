## Compatibility

To ensure platform compatibility, Fuchsia provides the following guarantees and
restrictions:

* [API compatibility guarantee](#api-compatibility-guarantee)
* [ABI compatibility guarantee](#abi-compatibility-guarantee)
* [Unsupported configurations are forbidden](#abi-revision-checks)

### API compatibility guarantee {#api-compatibility-guarantee}

Fuchsia makes the following API build-time compatibility guarantee for numerical
API levels:

#### Policy

If an end-developer can successfully build their component targeting a
numerical API level `N` using some release of the SDK, they will be able to
successfully build that same source code using any release of the SDK with `N`
in the supported phase.

#### Explanation of the policy

An end-developer can upgrade or roll back their SDK without breaking their
build, until their chosen target API level is sunset. If updating to a different
SDK without changing their target API level causes their build to break
(for example, if an API element or header file they were using no longer exists
or has been moved), that will be treated as a Fuchsia platform bug.

Note: For more information about special API levels, see
[special API levels][special-api-levels].

On the other hand, there is **no** API compatibility guarantee for the mutable
special API levels `NEXT` and `HEAD`. When targeting `NEXT` or `HEAD`, an
end-developer may find that updating their SDK causes their code to no longer
compile.

Our current partners work in close coordination with the Fuchsia team, and
Fuchsia will avoid breaking the builds of end-developers that target `NEXT` on
a "best effort" basis. Fuchsia will make little-to-no effort to avoid breaking
the builds of end-developers targeting `HEAD`.

### ABI compatibility guarantee {#abi-compatibility-guarantee}

Fuchsia makes the following ABI run-time compatibility guarantee for numerical
API levels:

#### Policy

A component built targeting a numerical API level `N` will successfully run on
any release of Fuchsia with `N` in the supported or sunset phase.

#### Explanation of the policy

An end-developer does not need to change or recompile their code in order to
run on newer versions of Fuchsia, up until the point when their chosen target
API level is retired. If the platform behaves differently in a way that
interferes with their component's functionality on a different version of
Fuchsia, that will be treated as a Fuchsia platform bug.

Note: For more information about special API levels, see
[special API levels][special-api-levels].

On the other hand, there is **no** ABI compatibility guarantee for components
that target the mutable special API levels `NEXT` or `HEAD`. A component built
targeting `NEXT` or `HEAD` is permitted to run if and only if the version
of Fuchsia on which it is running exactly matches the version of the SDK that
built it. For example, a component targeting `NEXT` built with SDK version
`16.20231103.1.1` can run on Fuchsia version `16.20231103.1.1`, but cannot
run on a device with Fuchsia version `16.20231103.2.1`.

In most circumstances, ensuring the version of the SDK that builds a component
exactly matches the version of the OS on which it runs is not feasible.
Noteworthy exceptions are integration testing and local experimentation.
Out-of-tree repositories generally _do_ have control over which version of the
SDK they use for compilation and which version of the OS they use for testing.
If they want to be able to test functionality in `NEXT` or `HEAD`, they should
keep the two in sync.

### Unsupported configurations are forbidden {#abi-revision-checks}

Note: For more information about the component manager, see
[Component manager][component-manager].

By default, `component_manager` refuses to launch components whose ABI revision
stamp indicates that they are not covered by the ABI compatibility guarantee.
Specifically:

- A given release of Fuchsia will only run a component that targets API level
  `N` if `N` is either supported or sunset as of that release.
- A given release of Fuchsia will only run a component that targets `NEXT` or
  `HEAD` if it was built using an SDK from the exact same release.

Likewise, [Software Assembly](/docs/glossary/README.md#software-assembly) cannot assemble
products with components that are not compatible with the platform release
being assembled.

Product owners will have the ability to selectively disable these checks. For
instance, they will be able to allowlist an individual component to target
`NEXT` or `HEAD`, even if the SDK that built it came from a different Fuchsia
release. Product owners disable these checks at their own risk.

[special-api-levels]: /docs/concepts/versioning/api_levels.md#special-api-levels
[component-manager]: /docs/concepts/components/v2/component_manager.md