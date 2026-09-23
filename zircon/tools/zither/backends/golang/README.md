# The Zither Go Backend

The Zither Go backend is gives Go data layout bindings.

## Output layout

One file `${filename}.go` is generated per original FIDL source file,
containing the bindings for the declarations defined there. Given a FIDL
library by the name of `id1.id2.....idn`, these files comprise a package of
name `${id1}/.../${idn}`.

Additionally, a `pkg_name.txt` is generated with the package name for its
contents.

## GN integration

`${fidl_target}_zither.golang` gives a `go_library()` defining the generated
package.

## Bindings

Any declaration type not mentioned below is ignored.

### Built-in types

| FIDL type     | Go type   |
| ------------- | --------- |
| `int8`        | `int8`    |
| `int16`       | `int16`   |
| `int32`       | `int32`   |
| `int64`       | `int64`   |
| `uint8`       | `uint8`   |
| `uint16`      | `uint16`  |
| `uint32`      | `uint32`  |
| `uint64`      | `uint64`  |
| `bool`        | `bool`    |
| `string`      | `string`  |
| `uchar`       | `byte`    |
| `usize64`     | `uint`    |
| `uintptr64`   | `uintptr` |
| `array<T, N>` | `[N]T'`   |

Note that FIDL `string`s are only permitted as constants.

### Constants

```fidl
const INT_CONST uint32 = 10;  // Or any integral type.
const STR_CONST string = "string constant";
```

yields

```go
const IntConst uint32  = 10
const StringConst string = "string constant"
```

### Enums

```fidl
type MyEnum = enum : int8 {  // Or any valid integral type
    ZERO = 0;
    ONE = 1;
};
```

yields

```go
type MyEnum int8

const (
    MyEnumZero MyEnum = 0
    MyEnumOne MyEnum = 1
)
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

```go
type MyBits uint8

const (
    MyBitsOne  MyBits = 1 << 0
    MyBitsTwo  MyBits = 1 << 1
    MyBitsFour MyBits = 1 << 2
)
```

### Structs

```fidl
type MyStruct = struct {
    member_a uint64;
    member_b bool;
};
```

yields

```go
type MyStruct struct {
   MemberA uint64
   MemberB bool
}
```

### Aliases

```fidl
alias MyAlias = MyType;
```

yields

```go
type MyAlias = MyType
```

### Overlays

```fidl
type MyOverlay = strict overlay {
    1: a MyOverlayStructVariant;
    2: b uint32;
};
```

yields

```go
type MyOverlayDiscriminant uint64

const (
	MyOverlayDiscriminantA MyOverlayDiscriminant = 1
	MyOverlayDiscriminantB MyOverlayDiscriminant = 2
)

type MyOverlay struct {
	Discriminant MyOverlayDiscriminant
	variant      [8]byte
}

func (o MyOverlay) IsA() bool {
	return o.Discriminant == MyOverlayDiscriminantA
}

func (o *MyOverlay) AsA() *MyOverlayStructVariant {
	if !o.IsA() {
		return nil
	}
	return (*MyOverlayStructVariant)(unsafe.Pointer(&o.variant))
}

func (o MyOverlay) IsB() bool {
	return o.Discriminant == MyOverlayDiscriminantB
}

func (o *MyOverlay) AsB() *uint32 {
	if !o.IsB() {
		return nil
	}
	return (*uint32)(unsafe.Pointer(&o.variant))
}
```
