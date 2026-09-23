# The Zither C Backend

The Zither C backend is gives C data layout bindings.

Consider a FIDL library by the name of `${id1}.${id2}.....${idn}`.

## Output layout

Given a FIDL library by the name of `id1.id2.....idn`, one header
`<id1/id2/.../idn/c/${filename}.h>` is generated per original FIDL source file,
containing the bindings for the declarations defined therein.

A README.md comprised of library-level FIDL documentation is generated as well.


## GN integration

`${fidl_target}_zither.c` gives a C library target with the generated headers
as public dependencies.

## Bindings

Any declaration type not mentioned below is ignored.

### Built-in types

| FIDL type     | C type        |
| ------------- | ------------- |
| `int8`        | `int8_t`      |
| `int16`       | `int16_t`     |
| `int32`       | `int32_t`     |
| `int64`       | `int64_t`     |
| `uint8`       | `uint8_t`     |
| `uint16`      | `uint16_t`    |
| `uint32`      | `uint32_t`    |
| `uint64`      | `uint64_t`    |
| `bool`        | `bool`        |
| `string`      | `const char*` |
| `uchar`       | `char`        |
| `usize64`     | `size_t`      |
| `uintptr64`   | `uintptr_t`   |
| `array<T, N>` | `T'[N]`       |

Note that FIDL `string`s are only permitted as constants.


### Constants

```fidl
library example.lib;

const INT_CONST uint32 = 10;  // Or any integral type.
const STR_CONST string = "string constant";
```

yields

```c
#define EXAMPLE_LIB_INT_CONST ((uint32_t)(10u))
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
typedef int8_t example_lib_my_enum_t;

#define EXAMPLE_LIB_MY_ENUM_ZERO ((example_lib_my_enum_t)(0))
#define EXAMPLE_LIB_MY_ENUM_ONE ((example_lib_my_enum_t)(1))
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
typedef uint8_t example_lib_my_bits_t;

#define EXAMPLE_LIB_MY_BITS_ONE ((example_lib_my_bits_t)(1u << 0))
#define EXAMPLE_LIB_MY_BITS_TWO ((example_lib_my_bits_t)(1u << 1))
#define EXAMPLE_LIB_MY_BITS_FOUR ((example_lib_my_bits_t)(1u << 2))
```

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
typedef struct {
   uint64_t member_a;
   bool member_b;
} example_lib_my_struct_t;
```

### Aliases

```fidl
library example.lib;

alias MyAlias = MyType;
```

yields

```c
typedef example_lib_my_type_t example_lib_my_aliase_t;
```

### Overlays

```fidl
type MyOverlay = strict overlay {
    1: a MyOverlayStructVariant;
    2: b uint32;
};
```

yields

```c
#define EXAMPLE_MY_OVERLAY_A ((uint64_t)(1u))
#define EXAMPLE_MY_OVERLAY_B ((uint64_t)(2u))

typedef struct {
  uint64_t discriminant;
  union {
    example_my_overlay_struct_variant_t a;
    uint32_t b;
  };
} example_my_overlay_t;
```
