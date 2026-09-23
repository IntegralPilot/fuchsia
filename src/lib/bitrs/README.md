# bitrs

`bitrs` ("bitters") is a no-std crate for ergonomically specifying layouts of
bitfields over integral types. While the aim is to be general-purpose, the
imagined user is a systems programmer uncomfortably hunched over an
architectural manual or hardware spec, looking to transcribe register layouts
into Rust with minimal fuss.

The heavy lifting is done by the `layout!` procedural macro. Care is taken to
generate readable and efficient code. The [`zerocopy`][zerocopy] crate is
further leveraged for safe and efficient transmutation between integral values
and custom bitfield representations.

## Features

* Generation of a simple, extensible wrapper type around the given integral base
  type;
* Simple, const-friendly builder pattern for constructing layout instances;
* Automatic implementations of the basic, convenient traits one would expect out
  of a thin integral wrapper type:
    * `Copy`, `Clone`
    * `Eq`, `PartialEq`
    * `Default`
    * `From` over the base type
    * `Deref` and `DerefMut` with a target of the underlying base type
    * `Debug`, `Binary`, `LowerHex`, `UpperHex`, `Octal`
* Specification of default and "reserved-as" values, with `new()` respecting
  reserved-as values and `default()` respecting both;
* Custom bitfield representation types without any boilerplate;
* Iteration over individual bitfield metadata and values;
* Associated constants around masks and shifts for use in inline assembly.

## Example

```rust
use bitrs::{bitfield_repr, layout};

#[bitfield_repr(u8)]
pub enum CustomFieldRepr {
    Option1 = 0xa,
    Option2 = 0xf,
}

layout!({
    pub struct Example(u32);
    {
        let foo @ 21..14;
        let custom @ 13..10: CustomFieldRepr;
        let bar @ 9..8 = 0b11;
        let baz @ 7;
        let frob @ 6..4;
        let __ @ 3..2 = 1;
        let __ @ 1..0;
    }
});

fn main() {
    let example = *Example::default()
        .set_custom(CustomFieldRepr::Option2)
        .set_frob(0x7);
    assert_eq!(example.foo(), 0);
    assert_eq!(example.custom(), CustomFieldRepr::Option2);
    assert_eq!(example.bar(), 0b11);
    assert_eq!(example.baz(), false);
    assert_eq!(example.frob(), 0x7);
    assert_eq!(*example & 0b1100, 0b0100);

    // Will print: `Example { custom: Option2, foo: 0x0, bar: 0x3, baz: false, frob: 0xa }`
    println!("{example}");

    // Or iterate over all fields and and print them individually.
    for (metadata, value) in example {
      println!("{}: {:x}", metadata.name, value);
    }
}
```

## Syntax

To keep the DSL intuitive and formattable, we co-opt a few familiar Rust
syntax elements:

<blockquote>
    <em>Layout</em>:
    <br>
    &nbsp;&nbsp;
        <code>{</code>
            <em>LayoutType</em>
            <code>{</code>
                <em>Bitfield</em>
                <sup>*</sup>
            <code>}</code>
        <code>}</code>
    <br>
    <br>
    <em>LayoutType</em>:
    <br>
    &nbsp;&nbsp;
        <em>
            <a href="https://doc.rust-lang.org/reference/attributes.html">OuterAttribute </a>
        </em>
        <sup>*</sup>
    <br>
    &nbsp;&nbsp;
        <em>
            <a href="https://doc.rust-lang.org/reference/visibility-and-privacy.html">Visibility </a>
        </em>
        <sup>?</sup>
        <br>
    &nbsp;&nbsp;
        <code>struct</code>
        <a href="https://doc.rust-lang.org/reference/identifiers.html">IDENTIFIER </a>
        <code>(</code>
            <em>UnsignedBaseType</em>
        <code>)</code>
        <code>;</code>
    <br>
    <br>
    <em>Bitfield</em>:
    <br>
    &nbsp;&nbsp;&nbsp;&nbsp;
        <em>NamedBitfield</em>
        &nbsp;|&nbsp;
        <em>ReservedBitfield</em>
    <br>
    <br>
    <em>NamedBitfield</em>:
    <br>
    &nbsp;&nbsp;
        <code>#[unshifted]</code>
        <sup>?</sup>
    <br>
    &nbsp;&nbsp;
        <code>let</code>
        <a href="https://doc.rust-lang.org/reference/identifiers.html">IDENTIFIER </a>
        <code>@</code>
        <em>BitRange</em>
        (
            <code>:</code>
            <em>
                <a href="https://doc.rust-lang.org/reference/types.html">Type </a>
            </em>
        )
        <sup>?</sup>
        (
            <code>=</code>
            <em>
                <a href="https://doc.rust-lang.org/reference/expressions.html">Expression </a>
            </em>
        )
        <sup>?</sup>
        <code>;</code>
    <br>
    <br>
    <em>ReservedBitfield</em>:
    <br>
    &nbsp;&nbsp;
        <code>let __ @</code>
        <em>BitRange</em>
        (
            <code>=</code>
            <em>
                <a href="https://doc.rust-lang.org/reference/expressions.html">Expression </a>
            </em>
        )
        <sup>?</sup>
        <code>;</code>
    <br>
    <br>
    <em>BitRange</em>:
    <br>
    &nbsp;&nbsp;&nbsp;&nbsp;
        <a href="https://doc.rust-lang.org/reference/tokens.html#integer-literals">INTEGER_LITERAL </a>
    <br>
    &nbsp;&nbsp;|&nbsp;
        <a href="https://doc.rust-lang.org/reference/tokens.html#integer-literals">INTEGER_LITERAL </a>
        <code>..</code>
        <a href="https://doc.rust-lang.org/reference/tokens.html#integer-literals">INTEGER_LITERAL </a>
    <br>
    <br>
    <em>UnsignedBaseType</em>:<br>
    &nbsp;&nbsp;
            <code>u8</code> |
            <code>u16</code> |
            <code>u32</code> |
            <code>u64</code> |
            <code>u128</code>
    <br>
    <br>
</blockquote>

Despite the exclusive `..` token, both endpoints of a *BitRange*
are treated as inclusive bit indices; see
[Named and reserved fields](#named-and-reserved-fields).

<blockquote>
    <em>Multilayout</em>:
    <br>
    &nbsp;&nbsp;
        <code>{</code>
            <em>TaggedLayoutType</em>
            <sup>+</sup>
            <em>BitfieldSet</em>
            <sup>+</sup>
        <code>}</code>
    <br>
    <br>
    <em>TaggedLayoutType</em>:
        &nbsp;
        (
            <code>#[bitrs(</code>
            <em>TagList</em>
            <sup>?</sup>
            <code>)]</code>
        )
        <sup>?</sup>
        &nbsp;
        <em>LayoutType</em>
    <br>
    <br>
    <em>BitfieldSet</em>:
    <br>
    &nbsp;&nbsp;
        (
            <code>#[</code>
            <em>Predicate</em>
            <code>]</code>
        )
        <sup>?</sup>
        <code>{</code>
            <em>Bitfield</em>
            <sup>*</sup>
        <code>}</code>
    <br>
    <br>
    <em>Predicate</em>:
    <br>
    &nbsp;&nbsp;
        <em>Tag</em>
        <br>
    &nbsp;|&nbsp;
        <code>all(</code> <em>PredicateList</em><sup>?</sup> <code>)</code>
        <br>
    &nbsp;|&nbsp;
        <code>any(</code> <em>PredicateList</em><sup>?</sup> <code>)</code>
        <br>
    &nbsp;|&nbsp;
        <code>not(</code> <em>Predicate</em> <code>)</code>
    <br>
    <br>
    <em>PredicateList</em>:
        &nbsp;<em>Predicate</em> ( <code>,</code> <em>Predicate</em> )<sup>*</sup>
    <br>
    <br>
    <em>TagList</em>:
        &nbsp;<em>Tag</em> ( <code>,</code> <em>Tag</em> )<sup>*</sup>
    <br>
    <br>
    <em>Tag</em>:
        &nbsp;<a href="https://doc.rust-lang.org/reference/identifiers.html">IDENTIFIER </a>
        (other than <code>all</code>, <code>any</code>, or <code>not</code>)
    <br>
    <br>
</blockquote>

Every tag referenced in a predicate must be declared on at least one
struct, and every bitfield set must match at least one struct. The
bitfield-set sequence must be non-empty.

## Generated Code

Using the `Example` struct from the [Example](#example) section, here is how the macro translates the definition:

### Layout type

The layout type is always a tuple struct wrapping an unsigned integral type,
giving a layout of bitfields over such an integer. The underlying integral
type is referred to as the "base type". In particular, `Example` defines a
32-bit layout.

### Trait implementations

The basic, convenient traits one might expect of a thinly-wrapped integral
type are implemented for the layout type:
* `Copy`, `Clone`
* `Eq`, `PartialEq`
* `Default` (see [below](#new-and-default-default-and-reserved-as-values))
* `From` over the base type
* `Deref` and `DerefMut`, with a target of the base type
* `Debug`, `Binary`, `LowerHex`, `UpperHex`, `Octal`

`IntoIterator` is also implemented to [iterate](#iteration) over
individual field values and metadata.

### Layout type attributes

Though none are given on `Example`, all attributes annotating the layout
type in the macro are forwarded verbatim to the definition. However, any
derivations that conflict with the above implemented traits will result in a
compilation error.

### Visibility

To keep things simple and practical, only the visibility of the layout type
may be specified; it too is forwarded verbatim from the definition in the
macro. All methods are generated as public. The associated iterator type is
also given the same visibility as the layout type.

### Const-ness

All methods are const where possible. This is limited only by the current
unavailability of const traits, the exceptions being trait methods and the
getters and setters of [fields with custom representations](#fields-with-custom-representations).

### Named and reserved fields

Bitfields are defined in the block following the layout type definition, and
each is defined with a let statement of one of the following forms:

* `let $name @ $bit (= $default)?;`
* `let $name @ $high..$low (: $repr)? (= $default)?;`
* `let __ @ $bit (= $value)?;`
* `let __ @ $high..$low (= $value)?;`

Reserved fields are denoted by the identifier `__`, and yield no accessors.
A bare bit index `$bit` or range `$high..$low` (inclusive at both ends,
despite the exclusive-range token) indicates the bits covered by the field.
Fields that span a single bit are referred to as _width-1_ fields.

A width-1 field named `foo` will yield a getter and setter of that bit's
content of the forms
```rust
const pub fn foo(&self) -> bool;

const pub fn set_foo(&mut self, value: bool) -> &mut Self;
```

Otherwise, a named field `foo` that does not specify a
[custom representation](#fields-with-custom-representations) (given by `$repr`
above) yields a getter and setter over its range

```rust
const pub fn foo(&self) -> MinWidth<$high, $low>;

const pub fn set_foo(&mut self, value: MinWidth<$high, $low>) -> &mut Self;
```

where `MinWidth<$high, $low>` is the smallest unsigned integral type of
bit size at least `$high - $low + 1`.

Each getter and setter is annotated with an auto-generated doc string
referencing the corresponding bit range (i.e., `TypeName[hi:lo]` or
`TypeName[bit]`). Doc comments on field declarations are also forwarded to
the getter, and appear before the auto-generated line.

A named field may be annotated with `#[unshifted]`, which results in the
getter and setter operating on values in their original bit position within
the base type rather than shifting them down. For example, the setter and
getter for a field at \[19:16\] would expect and return values like
`0x50000`  instead of `0x5`. `#[unshifted]` is incompatible with reserved
fields and fields with custom representations.

If an expression is given on the right-hand side of a field declaration,
this indicates a _default_ value in the case of a named field or a
_reserved-as_ value in the case of a reserved value. More on that
[below](#new-and-default-default-and-reserved-as-values).

Note that a reserved field with no reserved-as value has no semantic meaning
and is purely for documentation's sake.

### Fields with custom representations

Fields (of width > 1) can be given more structure with a _custom
representation_, which is a type specified as `$repr` above. The
corresponding getters and setter are of the forms

```rust
fn foo(&self) -> $repr
where
    $repr: zerocopy::TryFromBytes;

fn try_foo(&self) -> Result<$repr, bitrs::InvalidBits<MinWidth<$high, $low>>>
where
    $repr: zerocopy::TryFromBytes;

fn set_foo(&mut self, value: $repr)
where
    $repr: zerocopy::IntoBytes + zerocopy::Immutable;
```

Not all bit patterns are necessarily valid with a custom representation.
`foo` panics on an invalid pattern; `try_foo` returns an `InvalidBits`
error wrapping the invalid pattern.

A custom representation must - definitionally - satisfy the three zerocopy
traits above, which are leveraged for safe and efficient transmutation
between `MinWidth<$high, $low>` and `$repr`. They are all derivable.

Another requirement of a custom representation is that it implements
`Debug`, which is used in the formatting of the layout type.

For brevity and readability, the `bitfield_repr` attribute may be used to
define a custom representation.

### `new()` and `default()`; default and reserved-as values

Reserved-as field values reflect values that fields _must_ have, modeling
hardware requirements in the case of registers. Given that, `new()` will
yield an otherwise-zeroed base value with the reserved-as values set.

Default field values reflect desired defaults, possibly modeling reset
values in the case of registers. `default()` will yield an otherwise-zeroed
base value with _both_ the default and reserved-as values set.

### Associated constants

In C, one would accomplish bitfield manipulation through manual masking and
shifting, usually with an equivalent set of `FOO_MASK` and `FOO_SHIFT`
preprocessor variables. Even though we have more structure at our disposal
in this context, it can sometimes be convenient to have these raw masks and
shifts on hand. One example is when building up a register value in inline
assembly. Accordingly, the layout type will have the associated constants of
`FOO_MASK: $base` and `FOO_SHIFT: usize` for each named field `foo` of
width > 1; for a width-1 field named `foo` only a `FOO_BIT: usize` constant
will be defined, representing the shift. Further, `RSVD1_MASK: $base` and
`RSVD0_MASK: $base` are defined, giving the mask of reserved-as bits that
should be set or unset, as well as `DEFAULT: $base` giving the default
layout value.

### Iteration

The layout type admits iterators over field values and metadata. An iterator
can be accessed via `iter()`, and `IntoIterator` is implemented by the
layout type and references to it. Its item type is
`(&'static bitrs::FieldMetadata<$base>, $base)`. See `FieldMetadata` for
more info.

Iterators and iteration are both cheap, with the associated metadata being
defined as a static constant.

## Custom Field Representations

`bitfield_repr` is an attribute that is syntactic sugar for deriving
`repr(X)` and the handful of traits expected of a custom field representation.

## Multiple Related Layouts

`multilayout!` defines a family of closely related bitfield layouts in one
place, expanding to one `layout!`-equivalent type per declared struct.

`multilayout!` is a strict layering over `layout!`: it parses one or
more struct heads followed by a sequence of *bitfield sets*,
fans the fields out per struct, and emits the same code that hand-written
`layout!` invocations would. There is no shared trait, no const-generic
parameterization, and no runtime relationship between the emitted types.

Each struct head may carry a `#[bitrs(tag, ...)]` attribute declaring the
set of tags that struct participates in. Tags are arbitrary identifiers
(except for the reserved names `all`, `any`, and `not`), and a struct may
declare zero or more.

Each bitfield set is either bare (`{ ... }` — applies to every
declared struct) or predicated (`#[<predicate>] { ... }`), where a
*predicate* is a boolean expression over the declared tags:

* `tag` — true iff the struct carries `tag`;
* `all(<predicate>, ...)` — true iff every inner predicate is true
  (the empty `all()` is vacuously true);
* `any(<predicate>, ...)` — true iff some inner predicate is true
  (the empty `any()` is vacuously false);
* `not(<predicate>)` — true iff the inner predicate is false.

Predicates nest freely. A bitfield set applies to a struct iff its
predicate evaluates true against that struct's tag set; an unannotated
set always applies. Set order is irrelevant; each set's fields union
into the matching per-struct field lists. Each field name may appear
at most once per struct, and field bit ranges may not overlap within
a struct; declaring the same name twice, or declaring two fields whose
ranges overlap, in any combination of bitfield sets that all apply
to a given struct is an error.

### Example

The RISC-V status-register family — `mstatus` and `sstatus` in both RV32
and RV64 forms. The M-mode structs are tagged with `m`; every struct is
tagged with its XLEN (`rv32` or `rv64`). Bitfield-set predicates
select against those tags.

```rust
use bitrs::multilayout;

multilayout!({
    #[bitrs(m, rv32)]
    pub struct Mstatus32(u32);
    #[bitrs(m, rv64)]
    pub struct Mstatus64(u64);
    #[bitrs(rv32)]
    pub struct Sstatus32(u32);
    #[bitrs(rv64)]
    pub struct Sstatus64(u64);

    // SD sits at XLEN-1 in every *status form.
    #[rv32]
    {
        let sd @ 31;
    }
    #[rv64]
    {
        let sd @ 63;
    }

    // RV64 high-half. MBE/SBE/SXL are M-mode only.
    #[all(m, rv64)]
    {
        let mbe @ 37;
        let sbe @ 36;
        let sxl @ 35..34;
    }
    // UXL is visible from both M and S modes on RV64.
    #[rv64]
    {
        let uxl @ 33..32;
    }

    // M-mode-only low-half fields (WPRI from supervisor's view).
    #[m]
    {
        let tsr @ 22;
        let tw @ 21;
        let tvm @ 20;
        let mprv @ 17;
        let mpp @ 12..11;
        let mpie @ 7;
        let mie @ 3;
    }

    // Shared low-half fields.
    {
        let mxr @ 19;
        let sum @ 18;
        let xs @ 16..15;
        let fs @ 14..13;
        let vs @ 10..9;
        let spp @ 8;
        let ube @ 6;
        let spie @ 5;
        let sie @ 1;
    }
});

// SD lives at XLEN-1 in every *status form.
let m32 = *Mstatus32::new().set_sd(true);
let m64 = *Mstatus64::new().set_sd(true);
let s32 = *Sstatus32::new().set_sd(true);
let s64 = *Sstatus64::new().set_sd(true);
assert_eq!(m32.bits() & Mstatus32::SD_MASK, 1u32 << 31);
assert_eq!(m64.bits() & Mstatus64::SD_MASK, 1u64 << 63);
assert_eq!(s32.bits() & Sstatus32::SD_MASK, 1u32 << 31);
assert_eq!(s64.bits() & Sstatus64::SD_MASK, 1u64 << 63);

// M-mode-only fields only exist in mstatus.
let m = *Mstatus64::new().set_tsr(true).set_mpp(0b11).set_mie(true);
assert!(m.tsr());
assert_eq!(m.mpp(), 0b11);
assert!(m.mie());

// UXL is visible from both M and S modes; SXL is M-mode only.
let m = *Mstatus64::new().set_uxl(0b10);
let s = *Sstatus64::new().set_uxl(0b10);
assert_eq!(m.uxl(), 0b10);
assert_eq!(s.uxl(), 0b10);
```

## Why another crate for bitfields?

There are already a handful out there, so why this one too? It is the author's
opinion that none of those at the time of writing this offer _all_ of the above
features (e.g., around reserved semantics) or the author's desired ergonomics
around register modeling.
For example, some constrain field specification by bit width instead of by an
explicit bit range, which is not how registers are commonly described in
official references (plus, the author surely can't trust himself to do mental
math like that).

[zerocopy]: https://docs.rs/zerocopy/latest/zerocopy/
