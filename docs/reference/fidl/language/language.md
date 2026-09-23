# FIDL language specification

This document is a specification of the Fuchsia Interface Definition Language
(**FIDL**).

For more information about FIDL's overall purpose, goals, and requirements,
see [Overview][fidl-overview].

Also, see a modified [EBNF description of the FIDL grammar][fidl-grammar].

[TOC]

## Syntax

FIDL provides a syntax for declaring data types and protocols. These
declarations are collected into libraries for distribution.

FIDL declarations are stored in UTF-8 text files. Each file contains a sequence
of semicolon-delimited declarations. The order of declarations within a FIDL
file, or among FIDL files within a library, is irrelevant.

### Comments

FIDL comments start with two forward slashes (`//`) and continue to the end of
the line. Comments that start with three forward slashes (`///`) are called
documentation comments, and get emitted as comments in the generated bindings.

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="comments" %}
```

### Keywords

The following words have special meaning in FIDL:

```
ajar, alias, as, bits, closed, compose, const, enum, error, false, flexible,
library, open, optional, protocol, resource, service, strict, struct, table,
true, type, union, using.
```

However, FIDL has no reserved keywords. For example:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="keywords" %}
```

### Identifiers {#identifiers}

#### Library names

FIDL _library names_ label [FIDL libraries](#libraries). They consist of one or
more components separated by dots (`.`). Each component must match the regex
`[a-z][a-z0-9]*`. In words: library name components must start with a lowercase
letter, can contain lowercase letters, and numbers.

```fidl
// A library named "foo".
library foo;
```

#### Identifiers

FIDL _identifiers_ label declarations and their members. They must match the
regex `[a-zA-Z]([a-zA-Z0-9_]*[a-zA-Z0-9])?`. In words: identifiers must start
with a letter, can contain letters, numbers, and underscores, but cannot end
with an underscore.

```fidl
// A struct named "Foo".
type Foo = struct {};
```

FIDL identifiers are case sensitive. However, identifiers must have unique
_canonical forms_, otherwise the FIDL compiler will fail with [fi-0035:
Canonical name collision](/reference/fidl/language/errcat#fi-0035). The
canonical form of an identifier is obtained by converting it to `snake_case`.

#### Qualified identifiers {#qualified-identifiers}

FIDL always looks for unqualified symbols within the scope of the current
library. To reference symbols in other libraries, you must qualify them with the
library name or alias.

**objects.fidl:**

```fidl
library objects;
using textures as tex;

protocol Frob {
    // "Thing" refers to "Thing" in the "objects" library
    // "tex.Color" refers to "Color" in the "textures" library
    Paint(struct { thing Thing; color tex.Color; });
};

type Thing = struct {
    name string;
};
```

**textures.fidl:**

```fidl
library textures;

type Color = struct {
    rgba uint32;
};
```

#### Resolution algorithm {#resolution-algorithm}

FIDL uses the following algorithm to resolve identifiers. When a "try resolving"
step fails, it proceeds to the next step below. When a "resolve" step fails, the
compiler produces an error.

* If it is unqualified:
    1. Try resolving as a declaration in the current library.
    2. Try resoving as a builtin declaration, e.g. `bool` refers to `fidl.bool`.
    3. Resolve as a contextual bits/enum member, e.g. the `CHANNEL` in
       `zx.handle:CHANNEL` refers to `zx.ObjType.Channel`.
* If it is qualified as `X.Y`:
    1. Try resolving `X` as a declaration within the current library:
        1. Resolve `Y` as a member of `X`.
    2. Resolve `X` as a library name or alias.
        1. Resolve `Y` as a declaration in `X`.
* If it is qualified as `x.Y.Z` where `x` represents one or more components:
    1. Try resolving `x.Y` as a library name or alias:
        1. Resolve `Z` as a declaration in `x.Y`.
    2. Resolve `x` as a library name or alias:
        1. Resolve `Y` as a declaration in `x`:
            1. Resolve `Z` as a member of `x.Y`.

Note: If you import libraries `X` and `X.Y`, and library `X` defines an enum
named `Y`, you cannot refer to a member of that enum since `X.Y.Z` would be
interpreted as the declaration `Z` from library `X.Y`, even if no such
declaration exists. To refer to the enum member, you would have to remove or
alias one of the imports. This should not come up in practice when following the
[FIDL Style Guide][naming-style], which mandates `lowercase` library names and
`UpperCamel` declaration names.

#### Fully qualified names {#fqn}

FIDL uses fully qualified names (abbreviated "FQN") to refer unambiguously to
declarations and members. An FQN consts of a library name, a slash `/`, a
declaration identifier, and optionally a dot `.` and member identifier. For
example:

* `fuchsia.io/MAX_BUF` refers to the `MAX_BUF` constant in library `fuchsia.io`.
* `fuchsia.process/Launcher.Launch` refers to the `Launch` method in the
  `Launcher` protocol of library `fuchsia.process`.

FQNs are used in error messages, the FIDL JSON intermediate representation,
method selectors, and documentation comment cross references.

### Literals

FIDL supports the following kinds of literals:

* Boolean: `true`, `false`
* Integer: decimal (`123`), hex (`0xA1B2`), octal (`0755`), binary (`0b101`).
    * Only decimal literals can be negative.
* Floating point: `1.23`, `-0.01`, `1e5`, `2.0e-3`
    * Only `e` and `e-` are allowed, not `e+`.
* String: `"hello"`, `"\\ \" \n \r \t \u{1f642}"`

All letters in numeric literals (e.g. hex digits) are case insensitive.

### Constants {#constants}

FIDL allows defining constants for all types that support literals (boolean,
integer, floating point, and string), and bits and enums. For example:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="consts" %}
```

Constant expressions are either literals, references to other constants,
references to bits or enum members, or a combination of bits members separated
by the pipe character (`|`). FIDL does not support any other arithmetic
expressions such as `1 + 2`.

### Declaration separator

FIDL uses the semicolon (`;`) to separate adjacent declarations within the
file, much like C.

## Libraries {#libraries}

Libraries are named containers of FIDL declarations.

```fidl
// library identifier separated by dots
library fuchsia.composition;

// "using" to import library "fuchsia.mem"
using fuchsia.mem;

// "using" to import library "fuchsia.geometry" under the alias "geo"
using fuchsia.geometry as geo;
```

Libraries import other libraries with `using` declarations. You can refer to
symbols in an imported library by qualifying them with the library name, as in
`fuchsia.mem.Range`. With the `using ... as` syntax you must qualify symbols
with the alias, as in `geo.Rect` (`fuchsia.geometry.Rect` would not work).

The scope of `library` and `using` declarations is limited to a single file.
Each file in a FIDL library must restate the `library` declaration and any
`using` declarations that the file needs.

The FIDL compiler does not require any particular directory structure, but each
FIDL library is usually organized in its own directory named after the library.
Libraries with more than one file [conventionally have an overview.fidl
file][library-overview] containing only a `library` declaration along with
attributes and a documentation comment.

The library's name may be used in language bindings as a namespace. For example,
the C++ bindings generator places declarations for the FIDL library `fuchsia.ui`
within the C++ namespace `fuchsia_ui`. Similarly, the Rust bindings generator
would generate a crate named `fidl_fuchsia_ui`.

## Types and type declarations

FIDL supports a number of builtin types as well as declarations of new types
(e.g. structs, unions, type aliases) and protocols.

### Primitives

*   Simple value types.
*   Cannot be optional.

FIDL supports the following primitive types:

*    Boolean                 **`bool`**
*    Signed integer          **`int8 int16 int32 int64`**
*    Unsigned integer        **`uint8 uint16 uint32 uint64`**
*    IEEE 754 Floating-point **`float32 float64`**

Numbers are suffixed with their size in bits. For example, **`int8`** is 1 byte.

#### Use

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="primitives" %}
```

### Bits {#bits}

* Named bit types.
* Discrete subset of bit values chosen from an underlying integer type.
* Cannot be optional.
* Bits can either be [`strict` or `flexible`](#strict-vs-flexible).
* Bits default to `flexible`.
* `strict` bits must have at least one member, `flexible` bits can be empty.

#### Operators

`|` is the bitwise OR operator for bits.

#### Use

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="bits" %}
```

### Enums {#enums}

* Proper enumerated types.
* Discrete subset of named values chosen from an underlying integer type.
* Cannot be optional.
* Enums can be [`strict` or `flexible`](#strict-vs-flexible).
* Enums default to `flexible`.
* `strict` enums must have at least one member, `flexible` enums can be empty.

#### Declaration

The ordinal is **required** for each enum element. The underlying type of
an enum must be one of: **int8, uint8, int16, uint16, int32, uint32, int64,
uint64**. If omitted, the underlying type defaults to **uint32**.

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="enums" %}
```

#### Use

Enum types are denoted by their identifier, which may be qualified if needed.

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="enum-use" %}
```

<<../../../development/languages/fidl/widgets/_enum.md>>

### Arrays

*   Fixed-length sequences of homogeneous elements.
*   Elements can be of any type.
*   Cannot be optional themselves; may contain optional types.

#### Use

Arrays are denoted **`array<T, N>`** where _T_ can be any FIDL type (including
an array) and _N_ is a positive integer constant expression that specifies the
number of elements in the array.

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="arrays" %}
```

Note that _N_ appears as a layout parameter, which means that it affects the ABI
of the type. In other words, changing the parameter `_N_` is an
[ABI-breaking][compat] change.

### Strings

*   Variable-length sequence of bytes representing text in UTF-8 encoding.
*   Can be optional; absent strings and empty strings are distinct.
*   Can specify a maximum size, e.g. **`string:40`** for a maximum 40 byte
    string. By default, `string` means `string:MAX`, i.e unbounded.
*   String literals support the escape sequences `\\`, `\"`, `\n`, `\r`, `\t`,
    and `\u{X}` where the `X` is 1 to 6 hex digits for a Unicode code point.
*   May contain embedded `NUL` bytes, unlike traditional C strings.

#### Use

Strings are denoted as follows:

*   **`string`** : required string ([validation error][lexicon-validate]
    occurs if absent)
*   **`string:optional`** : optional string
*   **`string:N, string:<N, optional>`** : string, and optional string, respectively,
    with maximum length of _N_ bytes

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="strings" %}
```

Note that _N_ appears as a constraint (it appears after the `:`), which means
that it does not affect the ABI of the type. In other words, changing the
parameter `_N_` is not an [ABI-breaking][compat] change.

> Strings should not be used to pass arbitrary binary data since bindings enforce
> valid UTF-8. Instead, consider `bytes` for small data or
> [`fuchsia.mem.Buffer`](/docs/development/api/fidl.md#consider-using-fuchsia_mem_buffer)
> for blobs. See
> [Should I use string or vector?](/docs/development/api/fidl.md#should-i-use-string-or-vector)
> for details.

### Vectors

*   Variable-length sequence of homogeneous elements.
*   Can be optional; absent vectors and empty vectors are distinct.
*   Can specify a maximum size, e.g. **`vector<T>:40`** for a maximum 40 element
    vector. By default, `vector<T>` means `vector<T>:MAX`, i.e. unbounded.
*   There is no special case for vectors of bools. Each bool element takes one
    byte as usual.

#### Use

Vectors are denoted as follows:

*   **`vector<T>`** : required vector of element type
    _T_ ([validation error][lexicon-validate] occurs if absent)
*   **`vector<T>:optional`** : optional vector of element type
    _T_
*   **`vector<T>:N` and `vector<T>:<N, optional>`** : vector, and optional vector,
    respectively, with maximum length of _N_ elements

_T_ can be any FIDL type.

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="vectors" %}
```

### Handles {#handles}

*   Transfers a Zircon capability by handle value.
*   Stored as a 32-bit unsigned integer.
*   Can be optional; absent handles are encoded as a zero-valued handle.
*   Handles may optionally be associated with a type and set of required Zircon
    rights.

#### Use

Handles are denoted:

*   **`zx.Handle`** : required Zircon handle of unspecified type
*   **`zx.Handle:optional`** : optional Zircon handle of unspecified type
*   **`zx.Handle:H`** : required Zircon handle of type _H_
*   **`zx.Handle:<H, optional>`** : optional Zircon handle of type _H_
*   **`zx.Handle:<H, R>`** : required Zircon handle of type _H_ with rights
    _R_
*   **`zx.Handle:<H, R, optional>`** : optional Zircon handle of type _H_ with
    rights _R_

_H_ can be any [object](/docs/reference/kernel_objects/objects.md) supported by
Zircon, e.g. `channel`, `thread`, `vmo`. Please refer to the
[grammar](grammar.md) for a full list.

_R_ can be any [right](/docs/concepts/kernel/rights.md) supported by Zircon.
Rights are bits-typed values, defined in the [`zx`](/zircon/vdso/rights.fidl)
FIDL library, e.g. `zx.Rights.READ`. In both the incoming and outgoing
directions, handles are validated to have the correct Zircon object type and at
least as many rights as are specified in FIDL. If the handle has more rights
than is specified in FIDL, then its rights will be reduced by a call to
`zx_handle_replace`. See [Life of a handle] for an example and [RFC-0028: Handle
rights](/docs/contribute/governance/rfcs/0028_handle_rights.md) for further
details.

Structs, tables, and unions containing handles must be marked with the
[`resource` modifier](#value-vs-resource).

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="handles" %}
```

### Structs {#structs}

*   Record type consisting of a sequence of typed fields.
*   Adding or removing fields or changing their types is generally
    not ABI compatible.
*   Declaration can have the [`resource` modifier](#value-vs-resource).
*   References may be `box`ed.
*   Structs contain zero or more members.

#### Declaration

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="structs" %}
```

#### Use

Structs are denoted by their declared name (e.g. **Circle**):

*   **`Circle`** : required Circle
*   **`box<Circle>`** : optional Circle, stored [out-of-line][wire-format].

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="structs-use" %}
```

### Tables {#tables}

*   Record type consisting of a sequence of typed fields with ordinals.
*   Declaration is intended for forward and backward compatibility in the face of schema changes.
*   Declaration can have the [`resource` modifier](#value-vs-resource).
*   Tables cannot be optional. The semantics of "missing value" is expressed by an empty table
    i.e. where all members are absent, to avoid dealing with double optionality.
*   Tables contain zero or more members.

#### Declaration

```fidl
type Profile = table {
    1: locales vector<string>;
    2: calendars vector<string>;
    3: time_zones vector<string>;
};
```

#### Use

Tables are denoted by their declared name (e.g. **Profile**):

*   **`Profile`** : required Profile

Here, we show how `Profile` evolves to also carry temperature units.
A client aware of the previous definition of `Profile` (without temperature units)
can still send its profile to a server that has been updated to handle the larger
set of fields.

```fidl
type TemperatureUnit = enum {
    CELSIUS = 1;
    FAHRENHEIT = 2;
};

type Profile = table {
    1: locales vector<string>;
    2: calendars vector<string>;
    3: time_zones vector<string>;
    4: temperature_unit TemperatureUnit;
};
```

### Unions {#unions}

* Record type consisting of an ordinal and an envelope.
* Ordinal indicates member selection, envelope holds contents.
* Declaration can be modified after deployment, while maintaining ABI
  compatibility. See the [Compatibility Guide][union-compat] for
  source-compatibility considerations.
* Declaration can have the [`resource` modifier](#value-vs-resource).
* Reference may be optional.
* Unions can either be [`strict` or `flexible`](#strict-vs-flexible).
* Unions default to `flexible`.
* `strict` unions must contain one or more members. A union with no members
  would have no inhabitants and thus would make little sense in a wire format.
  However, memberless `flexible` unions are allowed, as it is still possible to
  decode an memberless union (the contained data is always "unknown").

#### Declaration

```fidl
{% includecode gerrit_repo="fuchsia/samples" gerrit_path="src/calculator/fidl/calculator.fidl" region_tag="union" %}
```

#### Use {#unions-use}

Unions are denoted by their declared name (e.g. **Result**) and optionality:

*   **`Result`** : required Result
*   **`Result:optional`** : optional Result

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="unions-use" %}
```

### Strict vs. flexible {#strict-vs-flexible}

FIDL type declarations can either have **strict** or **flexible** behavior:

*   Bits, enums, and unions are flexible unless declared with the `strict`
    modifier.
*   Structs always have strict behavior.
*   Tables always have flexible behavior.

For strict types only, serializing or deserializing a value that contains data
not described in the declaration is a [validation error][lexicon-validate].

In this example:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="strict-vs-flexible" %}
```

By virtue of being flexible, it is simpler for `FlexibleEither` to evolve to
carry a third variant. A client aware of the previous definition of
`FlexibleEither` without the third variant can still receive a union from a
server that has been updated to contain the larger set of variants. If the
union is of the unknown variant, bindings may expose it as unknown data (i.e. as
raw bytes and handles) to the user and allow re-encoding the unknown union (e.g.
to support proxy-like use cases). The methods provided for interacting with
unknown data for flexible types are described in detail in the [bindings
reference][bindings-reference].

More details are discussed in
[RFC-0033: Handling of Unknown Fields and Strictness][rfc-0033].

Note: A type that is both flexible and a [value type](#value-vs-resource) will
not allow deserializing unknown data that contains handles.

### Value vs. resource {#value-vs-resource}

Every FIDL type is either a **value type** or a **resource type**. Resource
types include:

*   [handles](#handles)
*   [protocol endpoints](#protocols-use)
*   [aliases](#aliasing) of resource types
*   arrays and vectors of resource types
*   structs, tables, and unions marked with the `resource` modifier
*   optional (or boxed) references to any of the above types

All other types are value types.

Value types must not contain resource types. For example, this is incorrect:

```fidl
type Foo = struct { // ERROR: must be "resource struct Foo"
    h zx.Handle;
};
```

Types can be marked with the `resource` modifier even if they do not contain
handles. You should do this if you intend to add handles to the type in the
future, since adding or removing the `resource` modifier requires
[source-compatibility considerations][resource-compat]. For example:

```fidl
// No handles now, but we will add some in the future.
type Record = resource table {
    1: str string;
};

// "Foo" must be a resource because it contains "Record", which is a resource.
type Foo = resource struct {
    record Record;
};
```

More details are discussed in [RFC-0057: Default No Handles][rfc-0057].

### Protocols {#protocols}

*   Describe methods that can be invoked by sending messages over a channel.
*   Methods are identified by their ordinal. The compiler calculates it by:
    * Taking the SHA-256 hash of the method's [fully qualified name](#fqn).
    * Extracting the first 8 bytes of the hash digest,
    * Interpreting those bytes as a little endian integer,
    * Setting the upper bit (i.e. last bit) of that value to 0.
    * To override the ordinal, methods can have a `@selector` attribute. If the
      attribute's argument is a valid FQN, it will be used in place of the FQN
      above. Otherwise, it must be a valid identifier, and will be used in place
      of the method name when constructing the FQN.
*   Each method declaration states its arguments and results.
    *   If no results are declared, then the method is one-way: no response will
        be generated by the server.
    *   If results are declared (even if empty), then the method is two-way:
        each invocation of the method generates a response from the server.
    *   If only results are declared, the method is referred to as an
        *event*. It then defines an unsolicited message from the server.
    *   Two-way methods may declare an error type that a server can send
        instead of the response. This type must be an `int32`, `uint32`, or an
        `enum` thereof.

*   When a server of a protocol is about to close its side of the channel, it
    may elect to send an **epitaph** message to the client to indicate the
    disposition of the connection. The epitaph must be the last message
    delivered through the channel. An epitaph message includes a 32-bit int
    value of type **zx_status_t**.  Negative values are reserved for system
    error codes. The value `ZX_OK` (0) indicates an operation was successful.
    Application-defined error codes (previously defined as all positive
    `zx_status_t` values) are deprecated. For more details about epitaphs, see
    rejection of  [RFC-0031: Typed Epitaphs][rfc-0031]. For more details about
    `zx_status_t` see [RFC-0085: Reducing the zx_status_t space][rfc-0085].

#### Declaration

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="calculator" %}
```

#### Use {#protocols-use}

Protocols are denoted by their name, directionality of the channel, and
optionality:

*   **`client_end:Protocol`** : client endpoint of channel communicating over the FIDL protocol
*   **`client_end:<Protocol, optional>`** : optional version of the above
*   **`server_end:Protocol`** : server endpoint of a channel communicating over the FIDL protocol
*   **`server_end:<Protocol, optional>`** : optional version of the above

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="endpoints" %}
```

### Protocol composition {#protocol-composition}

A protocol can include methods from other protocols.
This is called composition: you compose one protocol from other protocols.

Composition is used in the following cases:

1. you have multiple protocols that all share some common behavior(s)
2. you have varying levels of functionality you want to expose to different audiences

#### Common behavior

In the first case, there might be behavior that's shared across multiple protocols.
For example, in a graphics system, several different protocols might all share a
common need to set a background and foreground color.
Rather than have each protocol define their own color setting methods, a common
protocol can be defined:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="composition-base" %}
```

It can then be shared by other protocols:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="composition-inherit" %}
```

In the above, there are three protocols, `SceneryController`, `Drawer`, and `Writer`.
`Drawer` is used to draw graphical objects, like circles and squares at given locations
with given sizes.
It composes the methods **SetBackground()** and **SetForeground()** from
the `SceneryController` protocol because it includes the `SceneryController` protocol
(by way of the `compose` keyword).

The `Writer` protocol, used to write text on the display, includes the `SceneryController`
protocol in the same way.

Now both `Drawer` and `Writer` include **SetBackground()** and **SetForeground()**.

This offers several advantages over having `Drawer` and `Writer` specify their own color
setting methods:

*   the way to set background and foreground colors is the same, whether it's used
    to draw a circle, square, or put text on the display.
*   new methods can be added to `Drawer` and `Writer` without having to change their
    definitions, simply by adding them to the `SceneryController` protocol.

The last point is particularly important, because it allows us to add functionality
to existing protocols.
For example, we might introduce an alpha-blending (or "transparency") feature to
our graphics system.
By extending the `SceneryController` protocol to deal with it, perhaps like so:

```fidl
protocol SceneryController {
    SetBackground(struct { color Color; });
    SetForeground(struct { color Color; });
    SetAlphaChannel(struct { a int; });
};
```

we've now extended both `Drawer` and `Writer` to be able to support alpha blending.

#### Multiple compositions

Composition is not a one-to-one relationship &mdash; we can include multiple compositions
into a given protocol, and not all protocols need be composed of the same mix of
included protocols.

For example, we might have the ability to set font characteristics.
Fonts don't make sense for our `Drawer` protocol, but they do make sense for our `Writer`
protocol, and perhaps other protocols.

So, we define our `FontController` protocol:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="composition-multiple-1" %}
```

and then invite `Writer` to include it, by using the `compose` keyword:

```fidl
protocol Writer {
    compose SceneryController;
    compose FontController;
    Text(struct { x int; y int; message string; });
};
```

Here, we've extended the `Writer` protocol with the `FontController` protocol's methods,
without disturbing the `Drawer` protocol (which doesn't need to know anything about fonts).

Protocol composition is similar to [mixin].
More details are discussed in [RFC-0023: Compositional Model][rfc-0023].

#### Layering

At the beginning of this section, we mentioned a second use for composition, namely
exposing various levels of functionality to different audiences.

In this example, we have two protocols that are independently useful, a `Clock` protocol
to get the current time and timezone:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="layering-clock" %}
```

And an `Horologist` protocol that sets the time and timezone:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="layering-horologist" %}
```

We may not necessarily wish to expose the more privileged `Horologist` protocol to just
any client, but we do want to expose it to the system clock component.
So, we create a protocol (`SystemClock`) that composes both:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="layering-systemclock" %}
```

### Unknown interactions {#unknown-interactions}

Protocols can define how they react when they receive a method call or event
which has an ordinal which isn't recognized. Unrecognized ordinals occur
primarily when a client and server were built using different versions of a
protocol which may have more or fewer methods, though it can also occur if
client and server are mistakenly using different protocols on the same channel.

To control the behavior of the protocol when these unknown interactions occur,
methods can be marked as either `strict` or `flexible`, and protocols can be
marked as `closed`, `ajar`, or `open`.

The method strictness modifiers, `strict` and `flexible`, specify how the
sending end would like the receiving end to react to the interaction if it does
not recognize the ordinal. For a one-way or two-way method, the sending end is
the client, and for an event the sending end is the server.

*   `strict` means that it should be an error for the receiving end not to know
    the interaction. If a `strict` unknown interaction is received, the receiver
    should close the channel.
*   `flexible` means that the unknown interaction should be handled by the
    application. If the protocol allows for that type of unknown interaction,
    the ordinal is passed to an unknown interaction handler which can then
    decide how to react to it. What types of unknown interactions a protocol
    allows is determined by the protocol modifier. `flexible` is the default
    value if no strictness is specified.

The protocol openness modifiers, `closed`, `ajar` and `open` control how the
receiving end reacts to `flexible` interactions if it does not recognize the
ordinal. For a one-way or two-way method, the receiving end is the server, and
for an event, the receiving end is the client.

*   `closed` means the protocol does not accept any unknown interactions. If any
    unknown interaction is received, the bindings report an error and end
    communication, regardless of whether the interaction is `strict` or
    `flexible`.
    *   All methods and events must be declared as `strict`.
*   `ajar` means that the protocol allows unknown `flexible` one-way methods and
    events. Any unknown two-way methods and `strict` one-way methods or events
    still cause an error and result in the bindings closing the channel.
    *   One-way methods and events may be declared as either `strict` or
        `flexible`.
    *   Two-way methods must be declared as `strict`.
*   `open` means that the protocol allows any unknown `flexible` interactions.
    Any unknown `strict` interactions still cause an error and result in the
    bindings closing the channel. Open is the default value if no openness is
    specified.
    *   All methods and events may be declared as either `strict` or `flexible`.

Here is a summary of which strictness modifiers are allowed for different kinds
of methods in each protocol type. The default value of openness, `open`, marked
in _italics_. The default value of strictness, `flexible`, is marked in
**bold**.

|          | strict M(); | **flexible M();**    | strict -> M(); | **flexible -> M();** | strict M() -> (); | **flexible M() -> ();** |
|----------|-------------|----------------------|----------------|----------------------|-------------------|-------------------------|
| _open P_ | _compiles_  | **_compiles_**       | _compiles_     | **_compiles_**       | _compiles_        | **_compiles_**          |
| ajar P   | compiles    | **compiles**         | compiles       | **compiles**         | compiles          | **fails to compile**    |
| closed P | compiles    | **fails to compile** | compiles       | **fails to compile** | compiles          | **fails to compile**    |

Example usage of the modifiers on a protocol.

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="unknown-interactions" %}
```

Keep in mind that unknown interaction handling applies only when the receiving
end doesn't recognize the ordinal and doesn't know what the interaction is.
This means that the receiving end does not know whether the interaction is
supposed to be strict or flexible. To allow the receiver to know how to handle
an unknown interaction, the sender includes a bit in the message header which
tells the receiver whether to treat the interaction as strict or flexible.
Therefore, the strictness used in an interaction is based on what strictness the
sender has for the method it tried to call, but the protocol's openness for that
interaction depends on what openness the receiver has.

Here's how a method or event with each strictness is handled by a protocol with
each openness when the receiving side doesn't recognize the method.

|          | strict M(); | flexible M(); | strict -> M(); | flexible -> M(); | strict M() -> (); | flexible M() -> (); |
|----------|-------------|---------------|----------------|------------------|-------------------|---------------------|
| open P   | auto-closed | handleable    | auto-closed    | handleable       | auto-closed       | handleable          |
| ajar P   | auto-closed | handleable    | auto-closed    | handleable       | auto-closed       | auto-closed         |
| closed P | auto-closed | auto-closed   | auto-closed    | auto-closed      | auto-closed       | auto-closed         |

#### Interaction with composition

`flexible` methods and events cannot be declared in `closed` protocols and
`flexible` two-way methods cannot be declared in `ajar` protocols. To ensure
these rules are enforced across protocol composition, a protocol may only
compose other protocols that are at least as closed as it is:

*   `open`: Can compose any protocol.
*   `ajar`: Can compose `ajar` and `closed` protocols.
*   `closed`: Can only compose other `closed` protocols.

### Aliasing {#aliasing}

Type aliasing is supported. For example:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="aliasing" %}
```

In the above, the identifier `StoryID` is an alias for the declaration of a
`string` with a maximum size of `MAX_SIZE`. The identifier `Chapters` is an
alias for a vector declaration of five `StoryId` elements.

The identifiers `StoryID` and `Chapters` can be used wherever their aliased
definitions can be used.
Consider:

```fidl
{% includecode gerrit_repo="fuchsia/fuchsia" gerrit_path="examples/fidl/fuchsia.examples.docs/language_reference.test.fidl" region_tag="aliasing-usage" %}
```

Here, the `Message` struct contains a string of `MAX_SIZE` bytes called `baseline`,
and a vector of up to `5` strings of `MAX_SIZE` called `chapters`.

<<../../../development/languages/fidl/widgets/_alias.md>>

### Builtins

FIDL provides the following builtins:

* Primitive types: `bool`, `int8`, `int16`, `int32`, `int64`, `uint8`, `uint16`,
  `uint32`, `uint64`, `float32`, `float64`.
* Other types: `string`, `client_end`, `server_end`.
* Type templates: `array`, `vector`, `box`.
* Aliases: `byte`.
* Constraints: `optional`, `MAX`.

All builtins below to the `fidl` library. This library is always available and
does not need to be imported with `using`. For example, if you declare a struct
named `string`, you can refer to the original string type as `fidl.string`.

#### Library `zx` {#zx-library}

Library `zx` is not built in, but it is treated specially by the compiler. It is
defined in [//zircon/vdso/zx](/zircon/vdso/zx). Libraries import it with
`using zx` and most commonly use the `zx.Handle` type.

### Inline layouts {#inline-layouts}

Layouts can also be specified inline, rather than in a `type` introduction
declaration. This is useful when a specific layout is only used once. For
example, the following FIDL:

```fidl
type Options = table {
    1: reticulate_splines bool;
};

protocol Launcher {
    GenerateTerrain(struct {
        options Options;
    });
};
```

can be rewritten using an inline layout:

```fidl
protocol Launcher {
    GenerateTerrain(struct {
        options table {
            1: reticulate_splines bool;
        };
    });
};
```

When an inline layout is used, FIDL will reserve a name for it that is
guaranteed to be unique, based on the [naming context][naming-context] that the
layout is used in. This results in the following reserved names:

* For inline layouts used as the type of an outer layout member, the reserved
  name is simply the name of the corresponding member.
    * In the example above, the name `Options` is reserved for the inline
      `table`.
* For top level request/response types, FIDL concatenates the protocol name,
  the method name, and then either `"Request"` or `"Response"` depending on
  where the type is used.
    * In the example above, the name `LauncherGenerateTerrainRequest` is
      reserved for the struct used as the request of the `GenerateTerrain`
      method.
    * Note that the `"Request"` suffix denotes that the type is used to initiate
      communication; for this reason, event types will have the `"Request"`
      suffix reserved instead of the `"Response"` suffix.

The name that is actually used in the generated code depends on the binding, and
is described in the individual [bindings references][bindings-reference].

For inline layouts used as the type of a layout member, there are two ways to
obtain a different reserved name:

* Rename the layout member.
* Override the reserved name using the [`@generated_name`][generated-name-attr]
  attribute.

[mixin]: https://en.wikipedia.org/wiki/Mixin
[rfc-0023]: /docs/contribute/governance/rfcs/0023_compositional_model_protocols.md
[rfc-0031]: /docs/contribute/governance/rfcs/0031_typed_epitaphs.md
[rfc-0033]: /docs/contribute/governance/rfcs/0033_handling_unknown_fields_strictness.md
[rfc-0057]: /docs/contribute/governance/rfcs/0057_default_no_handles.md
[rfc-0085]: /docs/contribute/governance/rfcs/0085_reducing_zx_status_t_space.md
[fidl-overview]: /docs/concepts/fidl/overview.md
[fidl-grammar]: /docs/reference/fidl/language/grammar.md
[doc-attribute]: /docs/reference/fidl/language/attributes.md#Doc
[naming-style]: /docs/development/languages/fidl/guides/style.md#Names
[compat]: /docs/development/languages/fidl/guides/compatibility/README.md
[union-compat]: /docs/development/languages/fidl/guides/compatibility/README.md#union
[resource-compat]: /docs/development/languages/fidl/guides/compatibility/README.md#modifiers
[bindings-reference]: /docs/reference/fidl/bindings/overview.md
[lexicon-validate]: /docs/reference/fidl/language/lexicon.md#validate
[wire-format]: /docs/reference/fidl/language/wire-format/README.md
[naming-context]: /docs/contribute/governance/rfcs/0050_syntax_revamp.md#layout-naming-contexts
[generated-name-attr]: /docs/reference/fidl/language/attributes.md#generated-name
[Life of a handle]: /docs/concepts/fidl/life-of-a-handle.md
[library-overview]: /docs/development/languages/fidl/guides/style.md#library-overview
