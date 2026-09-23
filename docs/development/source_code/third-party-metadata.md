# README.fuchsia file syntax

`README.fuchsia` files are used to annotate third-party source libraries
with some useful metadata, such as code origin, version, license, and security
critical label.

The format of these files consists of one or more directive lines,
followed by unstructured description and notes.

Directives consist of a directive keyword at the beginning of the line,
immediately followed by a colon and a value that extends to the end of
the line. The value may have surrounding whitespace, and blank lines may
appear before or between directives.

Several directives are described below, but other directives may appear
in `README.fuchsia` files and software that consumes them should not
treat the appearance of an unknown directive as an error. Similarly,
such software should match directive keywords case-insensitively.

Description lines are optional and follow a `Description` directive
that must appear on a line by itself prior to any unstructured
description text.

## Syntax

```
file                  := directive-line* description?
directive-line        := directive | blank-line
directive             := keyword ":" SPACE* value SPACE* EOL
value                 := NONBLANK ANYCHAR*
description           := description-directive description-line*
description-directive := "Description:" SPACE* EOL
description-line      := ANYCHAR* EOL
keyword               := [A-Za-z0-9][A-Za-z0-9 ]*
blank-line            := SPACE* EOL
SPACE                 := any whitespace character
EOL                   := end of line character
NONBLANK              := any non-whitespace, non-EOL character
ANYCHAR               := any character but EOL
```

## Requirements

Directive keywords and their definitions are defined below. This sections serves as synthesis of what each README.fuchsia needs to have and keep updated as time goes on:

* For vulnerability scanning purposes, each README.fuchsia needs to keep updated information on:
  * `URL` and `Revision`: This is the required option if it is a git repository.
  OR
  * `CPEPrefix` and `Version`

This information gives vulnerability scanners enough information to accurately scan these dependencies.

* For licensing purposes, each README.fuchsia needs to keep updated information on:
  * `License` and `License File`

## Common directive keywords

Common directive keywords include:

### `Name` {:#name}

Descriptive name of the package.

```
Name: OpenSSH
```

### `Short Name` {:#short-name}

*(Optional)* Name the package is distributed under (ex. libxml, openssl, etc).

```
Short Name: openssh
```

### `URL` {:#url}

*(REQUIRED)* The URL where the package lives i.e. a clonable url for git
repositories, a package manager url for packages from package managers, or
[a URL type listed here as per AutoVM's metadata proto](https://source.corp.google.com/piper///depot/google3/third_party/metadata.proto;l=165;rcl=670837485).
If there is no upstream, use 'This is the canonical public repository'. For
packages coming from Google internal repositories, use 'Google Internal'.
This directive may be repeated to include multiple URLs if necessary.

Examples:
```
URL: https://github.com/openssh/openssh-portable
```

```
URL: https://chromium.googlesource.com/chromium/src/
```

### `Revision` {:#revision}

*(REQUIRED for dependencies which have a git repository as an upstream,
OPTIONAL if the upstream is not a git repository and Version or Date is
supplied)*. Revision is typically a git hash. If the dependency is managed by
an autoroller or a script, you must ensure the uprev process also updates the
`README.fuchsia` file with the correct Revision.

```
Revision: 8950d99ba1ba67280fbd1e5445214d2cebe966bb
```

### `Date` {:#date}

The date that the package was updated, in format YYYY-MM-DD.

```
Date: 2018-02-14
```

### `License` {:#license}

The license/s under which the package is distributed. See
[Fuchsia Open Source Licensing Policies](../../contribute/governance/policy/open-source-licensing-policies.md)
for the policies around which licenses are allowed and other guidance.

```
License: BSD
```

### `License File` {:#license-file}

A relative path from the `README.fuchsia` file to the license file. The file
should contain a copy of the package's license and correspond to the License
provided above. All packages should contain a valid license, regardless of
whether it is shipped or not.
This directive may be repeated to include multiple files if necessary.

```
License File: LICENSE
```

### `Security Critical` {:#security-critical}

A `yes` or `no` label indicating whether the package is security critical,
useful for assessing the impact security bugs in the package have on Fuchsia.

A package is security critical if it is for production use, and does any of
the following:

*   Accepts untrustworthy inputs from the internet
*   Parses or interprets complex input formats
*   Sends data to internet servers
*   Collects new data
*   Influences or sets security-related policy (including the user experience)
*   Is written in a memory-unsafe language (e.g.: C/C++, Rust with unsafe
    blocks)

This directive is required.

```
Security Critical: yes
```

### `License Android Compatible` {:#license-android-compatible}

*(Optional if the package is not shipped or uses a standard form license)*
Either `yes` or `no` depending on whether the package uses a license compatible
with Android.

```
License Android Compatible: yes
```

### `CPEPrefix` {:#cpe-prefix}

*(Optional, but REQUIRED if URL and Revision are not provided)* A 'common
platform enumeration' version 2.3 (preferred) or 2.2, as per
[search](https://nvd.nist.gov/products/cpe/search), which represents the
upstream package. This will be used to report known vulnerabilities in the
upstream software package, such that we can be sure to merge fixes for those
vulnerabilities. Please ensure you're using the closest applicable upstream
version, according to the standard format for the CPE for that package. For
example, `cpe:/a:xmlsoft:libxslt:1.0.10`. If no CPE is available for the
package, please specify "unknown". If you're using a patched or modified
version which is halfway between two public versions, please "round downwards"
to the lower of the public versions.

### `Version` {:#version}

*(REQUIRED if using CPEPrefix for vuln scanning)* This is often a git tag. If
not git, it should be a searchable version number for the package (if the
package does not version or is versioned by date or revision this field should
be "N/A" and the revision, or date should be enumerated in the appropriate
field). If the dependency is managed by an autoroller or a script, you must
ensure the uprev process also updates the `README.fuchsia` file with the
correct Version.

```
Version: 7.6
```

### `Description` {:#description}

A short description of what the package is and is used for.

```
Description:

This package does x, y, and z.
```

### `Local Modifications` {:#local-modifications}

Enumerate any changes that have been made locally to the package from the
shipping version listed above.

If the files from the third party package (e.g. fetched during a git checkout)
aren't modified, put "None" here (without enclosing quotes).

Note: Files required for tooling integration don't count as local
modifications. Examples include: BUILD.gn, OWNERS file, DIR_METADATA, LICENSE,
README.fuchsia.

```
Local Modifications:

Added README.fuchsia.
Ported build rules from CMake to GN.
```

## References

The `README.fuchsia` format is based on Chromium's `README.chromium` format.
See [Chromium's adding_to_third_party.md](https://chromium.googlesource.com/chromium/src/+/HEAD/docs/adding_to_third_party.md#readme_chromium)
as a supplementary reference.
