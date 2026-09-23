# Platform versioning

This document provides a high-level overview of platform versioning in Fuchsia,
focusing on the key concepts that platform developers need to understand.

## API Levels

The core of Fuchsia's versioning system lies in **API levels**. These levels
represent snapshots of the Fuchsia API surface at a specific point in time. The
API surface encompasses various elements, including FIDL interfaces, system
calls, and libraries. When using the SDK to develop a component for Fuchsia, you
specify a target API level. This target API level signifies the specific version
of the Fuchsia API surface that your component requires and ensures
compatibility with devices supporting that level. Just like other platforms,
Fuchsia regularly publishesnew API levels.

Fuchsia devices support a specific set of API levels. If a device supports a
particular API level, it guarantees that all functionalities associated with
that level are available and functional. Conversely, if a device doesn't support
the target API level of a component, the component won't run on that device.
Fuchsia can also drop support for older API levels, a process referred to as
**retiring**. This mechanism allows the platform to evolve while maintaining
backward compatibility.

For more information on how API levels and a Fuchsia release work, see
[API levels][api-levels] and [Release][fuchsia-release], respectively.

### Special API levels

While the primary focus is on stable, numbered API levels, Fuchsia also
incorporates special API levels for development purposes. These special API
levels, `NEXT` and `HEAD`, allow platform developers to introduce and test new
features and API changes before they are integrated into a stable release.
However, targeting special API levels imposes additional constraints, and they
are not intended for general component development.

For more information on special API levels, see
[Special API levels][special-api-levels].

## Key considerations for platform developers

As a platform developer, understanding how API levels impact development is
crucial. When making changes to the API surface, these modifications should be
made at `HEAD` while you are developing. Once the changes are ready to be
published, you should change `HEAD` to `NEXT`. Since the `NEXT` API level,
represents the next version to be published, any change will be incorporated
into the next published API level. For example, if the current published API level
is 25, any changes made in `NEXT` will be applied to API level 26.

Platform components must maintain support for all API levels currently supported
by Fuchsia. Fuchsia supports all API levels with a phase of Supported and
Sunset. This requirement ensures backward compatibility. The
[`sdk/version_history.json`] file lists the phase of each API level. Even
if a method is removed in a newer API level, platform components must continue
to implement it if older, still-supported API levels include that method.
Bindings and tools often assist in this process, ensuring necessary interfaces
remain available for older API level targets.

For more information about the phases of an API level, see
[Phases][api-levels-phases].

Even with the help these tools provide, maintaining compatibility is difficult,
and platform developers must think carefully about it. When in doubt, remember
this: a component built against a given API level should behave the same way on
any Fuchsia device that supports that API level.

For more information on compatibility, see [Compatibility][compatibility-doc].

[compatibility-doc]: /docs/concepts/versioning/compatibility.md
[fuchsia-release]: /docs/concepts/versioning/release.md
[special-api-levels]: /docs/concepts/versioning/api_levels.md#special-api-levels
[api-levels]: /docs/concepts/versioning/api_levels.md
[api-levels-phases]: /docs/concepts/versioning/api_levels.md#phases
[`sdk/version_history.json`]: /sdk/version_history.json
