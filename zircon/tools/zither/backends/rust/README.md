# The Zither Rust Backend

The Zither Rust backend is gives Rust data layout bindings.

## Output layout

Given a FIDL library by the name of `id1.id2.....idn`, one crate with name
`id1-id2-...-idn` is generated: it contains a `${filename}.rs` for each original
FIDL source file and a crate root `lib.rs`, where each `${filename}.rs` has its
hyphens replaced by underscores (as hyphens there are illegal).

## GN integration

`${fidl_target}_zither.rust` gives a `rustc_library()` defining the generated
crate.

## Bindings

Any declaration type not mentioned below is ignored.

### Built-in types

| FIDL type     | Rust type   |
| ------------- | ----------- |
| `int8`        | `i8`        |
| `int16`       | `i16`       |
| `int32`       | `i32`       |
| `int64`       | `i64`       |
| `uint8`       | `u8`        |
| `uint16`      | `u16`       |
| `uint32`      | `u32`       |
| `uint64`      | `u64`       |
| `bool`        | `bool`      |
| `string`      | `&str`      |
| `uchar`       | `u8`        |
| `usize64`     | `usize`     |
| `uintptr64`   | `usize`     |
| `array<T, N>` | `[T'; N]`   |

Note that FIDL `string`s are only permitted as constants.

### Constants

```fidl
const INT_CONST uint32 = 10;  // Or any integral type.
const STR_CONST string = "string constant";
```

yields

```rust
pub const INT_CONST: uint32  = 10;
pub const STR_CONST: &str  = "string constant";
```

### Enums

```fidl
type MyEnum = enum : int8 {  // Or any valid integral type
    ZERO = 0;
    ONE = 1;
};
```

yields

```rust
#[repr(i8)]
pub enum MyEnum {
    Zero = 0,
    One = 1,
}
```

### Bits

```fidl
type MyBits = bits : uint8 {  // Or any valid integral type
    ONE = 1;
    TWO = 2;
    FOUR = 4;
};
```

yields

```
bitflags! {
    pub struct MyBits : u8 {
    const ONE = 1 << 0;
    const TWO = 1 << 1;
    const FOUR = 1 << 2;
  }
}
```

### Structs

```fidl
type MyStruct = struct {
    member_a uint64;
    member_b bool;
};
```

yields

```rust
#[repr(C)]
pub struct MyStruct {
    pub member_a: uint64,
    pub member_b: bool,
}
```

### Aliases

```fidl
alias MyAlias = MyType;
```

yields

```rust
pub type MyAlias = MyType;
```

### Overlays

```fidl
type MyOverlay = strict overlay {
    1: a MyStruct;
    2: b uint32;
    3: c bool;
};
```

yields

```rust
#[repr(u64)]
pub enum MyOverlay {
    A(MyStruct) = 1,
    B(u32) = 2,
    C(bool) = 3,
}
```
