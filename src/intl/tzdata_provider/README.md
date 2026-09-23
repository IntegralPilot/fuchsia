# tzdata-provider

`tzdata-provider` is a component that provides [ICU timezone data][icu-data]
to other components via [directory capabilities][directory-capability]
that contain data files used by the ICU library. The user documentation
is provided on the [Fuchsia website][icu-data], while here we document the
provider side.

`tzdata-provider` is a component without an included executable program. Its
purpose is only to provide capability routing for ICU timezone data files.

Providing these capabilities is achieved via multiple steps.

1. The `tzdata-provider` is built with time zone resources [baked into its
   package][1]. Any system that includes `tzdata-provider` will have these
   files available. For the time being only one provider is run as the data
   schema version of the ICU tzdata has not changed over many years. The
   exposed resources are files, and are as follows:
      * `revision.txt`: a text file containing the ICU data version, such as
        `2023c`. Used for version verification.
      * `metaZones.res`, `timezoneTypes.res` and `zoneinfo64.res`: the ICU
        library resource files containing the time zone information that we
        need.
2. The `tzdata-provider` component manifest is [configured to expose][2] the
   directory capabilities that correspond to the resources exposed.
3. A [core shard][cs] is provided for products to include in their [assembly
   configuration][ac].

**Note**: in the past, time zone data files used to be provided via the
`config-data` mechanism. This is no longer the case, and if you come across a
place where `config-data` is mentioned when talking about time zone provision,
feel free to file a bug.

# zoneinfo-provider

`zoneinfo-provider` is a component that provides TZIF zoneinfo files to other
to other components via [directory capabilities][directory-capability].

`zoneinfo-provider` is a component without an included executable program. Its
purpose is only to provide capability routing.

Resource provision here is similar to the routing used by `tzdata-provider`
above.

[icu-data]: /docs/development/internationalization/icu_data.md
[directory-capability]: /docs/concepts/components/v2/capabilities/directory.md
[1]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/intl/tzdata_provider/BUILD.gn;l=24;drc=45f3d3be4e5cb08803335fec520f04d1e24441ba
[2]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/intl/tzdata_provider/meta/tzdata_provider.cml;l=1;drc=70a76aef047b999d3f3ce08f282cfb42f8dc8019
[cs]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/intl/tzdata_provider/meta/tzdata_provider.core_shard.cml
[ac]: https://source.corp.google.com/h/turquoise-internal/turquoise/+/main:bundles/assembly/BUILD.gn;l=1414;drc=f07441f4bce3627a03cfbba9491fb43afcc47269

