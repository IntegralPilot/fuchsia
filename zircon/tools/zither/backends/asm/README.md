# The Zither Assembly Backend

The Zither assembly backend gives defines C preprocessor macros meant to be
used in assembly code

## Output layout

Given a FIDL library by the name of `id1.id2.....idn`, one header
`<id1/id2/.../idn/asm/${filename}.h>` is generated per original FIDL source
file, containing the bindings for the declarations defined therein.

## GN integration

`${fidl_target}_zither.asm` gives a C library target with the generated headers
as public dependencies.

## Bindings

Any declaration type not mentioned below is ignored.

### Constants

```fidl
library example.lib;

const INT_CONST uint32 = 10;  // Or any integral type.
const STR_CONST string = "string constant";
```

yields

```c

#define EXAMPLE_LIB_INT_CONST (10)
#define EXAMPLE_LIB_STR_CONST ("string constant")
```

### Enums

```fidl
library example.lib;

type MyEnum = enum : int8 {  // Or any valid integral type
    ZERO = 0;
    ONE = 1;
};
```

yields

```c

#define EXAMPLE_LIB_MY_ENUM_ZERO (0)
#define EXAMPLE_LIB_MY_ENUM_ONE (1)
```

### Bits

```fidl
library example.lib;

type MyBits = bits : uint8 {  // Or any valid integral type
    ONE = 1;
    TWO = 2;
    FOUR = 4;
};
```

yields

```c
#define EXAMPLE_LIB_MY_BITS_ONE (1)
#define EXAMPLE_LIB_MY_BITS_ONE_SHIFT (0)

#define EXAMPLE_LIB_MY_BITS_TWO (2)
#define EXAMPLE_LIB_MY_BITS_TWO_SHIFT (1)

#define EXAMPLE_LIB_MY_BITS_FOUR (4)
#define EXAMPLE_LIB_MY_BITS_FOUR_SHIFT (2)
```

Where the `*_SHIFT` macros give the base-2 logarithm of the associated bits
values.

### Structs

```fidl
library example.lib;

type MyStruct = struct {
    member_a uint64;
    member_b bool;
};
```

yields

```c
#define EXAMPLE_LIB_MY_STRUCT_SIZEOF (16)
#define EXAMPLE_LIB_MY_STRUCT_MEMBER_A (0)
#define EXAMPLE_LIB_MY_STRUCT_MEMBER_B (8)
```

where the `*_SIZEOF` macro gives the size of the struct on the wire (that is,
including padding), and where the macros associated with members give the
field's offset into the wire layout.

### Overlays

```fidl
type MyOverlay = strict overlay {
    1: a MyOverlayStructVariant;
    2: b uint32;
};

type MyOverlayStructVariant = struct {
  value uint64;
};
```

yields

```c
#define EXAMPLE_MY_OVERLAY_SIZEOF (24)
#define EXAMPLE_MY_OVERLAY_DISCRIMINANT (0)
#define EXAMPLE_MY_OVERLAY_VALUE (8)

#define EXAMPLE_MY_OVERLAY_A (1)
#define EXAMPLE_MY_OVERLAY_B (2)
```
