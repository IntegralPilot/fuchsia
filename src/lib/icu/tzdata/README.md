# tzdata packages and templates

This directory contains the GN templates that provide data files for packages
and underlying programs that need to load IANA timezone data as provided by
the [ICU](https://home.unicode.org) library.

## `icu_tzdata_resource`

NOTE: Most components should not use this rule to consume tzdata. Instead,
use the [tzdata directory capability][icu-data] to get the latest tzdata
provided by the platform.

This rule adds the files `metaZones.res`, `timezoneTypes.res`, and
`zoneinfo64.res` to a package under the path 
`/pkg/data/tzdata/icu/{data_version}/{format}/`.

There will also be a file at `/pkg/data/tzdata/revision.txt` containing the
time zone database revision ID, e.g. `2019c`.

[icu-data]: /docs/development/internationalization/icu_data.md
