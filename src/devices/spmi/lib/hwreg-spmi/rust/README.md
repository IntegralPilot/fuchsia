# `spmi-hwreg` - Rust SPMI register access library

`spmi-hwreg` provides highly ergonomic, type-safe, and asynchronous access to SPMI registers on
Fuchsia, matching the MMIO `hwreg` paradigm.

## Targets

This library provides two build targets to support different FIDL backends:

- **`spmi-hwreg`**: Uses the legacy Fuchsia FIDL bindings (`fidl_fuchsia_hardware_spmi`).
- **`spmi-hwreg-next`**: Uses the next-generation, zero-copy FIDL bindings
  (`fidl_next_fuchsia_hardware_spmi`).

Both targets share the same core register definition macros and logic in `common.rs`, ensuring API
compatibility and minimizing code duplication.

## Features

### Individual register definitions (`spmi_register!`)
Define single registers with bitfield extraction, masking, and helper setter functions:

```rust
spmi_register! {
    test_reg, u16, 0x10, RW, LE, {
        pub field1, set_field1: 7;
        pub field2, set_field2: 15, 8;
        pub const SUCCESS: u8 = 0x82;
    }
}
```

### Endianness support
Support both little-endian (`LE`) and big-endian (`BE`) registers natively:

```rust
spmi_register! {
    test_be_reg, u16, 0xEF, RW, BE, {
        pub flag, set_flag: 4;
        pub field, set_field: 3, 0;
    }
}
```

> [!NOTE]
> For 1-byte width registers (`u8`), specifying the endianness is completely optional as byte-order
> is irrelevant. The macro exposes a specialized arm for `u8` to omit it entirely.

### Register block access (`spmi_register_block!`)
Consolidate multiple registers into a single type-safe block tied to the SPMI FIDL client:

```rust
spmi_register_block! {
    pub struct TestRegs {
        pub test => test_reg,
    }
}

let regs = TestRegs::new(proxy);
let mut val = regs.test().read().await?;
let is_set = val.field1();
val = val.set_field2(0x5);
regs.test().write(val).await?;
```

Alternatively, initialize a new value from scratch using `Default` (which defaults to `0`):

```rust
let val = test_reg::Value::default()
    .set_field1(true)
    .set_field2(0x5);
regs.test().write(val).await?;
```

### Custom Device Wrapping (Generic Register Blocks)
By default, `spmi_register_block!` binds the SPMI register block to the platform's raw device client
(`DeviceType`).

However, if your driver needs to intercept register accesses (for example, to automatically manage
locking, logging, or telemetry), you can define a custom wrapped device. To do this, implement
`SpmiDevice` for your wrapper, and pass your wrapper as the type parameter to the
register block:

```rust
// 1. Define your custom wrapped device
struct LockedSpmiDevice {
    raw_client: DeviceType,
    // lock state...
}

impl LockedSpmiDevice {
    pub fn new(raw_client: DeviceType) -> Self {
        Self { raw_client }
    }
}

impl SpmiDevice for LockedSpmiDevice {
    async fn read_reg(&self, address: u16, size: u32) -> Result<Vec<u8>, spmi_hwreg::Error> {
        self.raw_client.read_reg(address, size).await
    }
    async fn write_reg(&self, address: u16, data: &[u8]) -> Result<(), spmi_hwreg::Error> {
        // Automatically unlock, write, and re-lock.
        // Note: For production drivers, an RAII guard pattern (e.g., `let _guard = self.lock_guard().await;`)
        // is recommended to guarantee re-locking if an error occurs.
        self.unlock().await;
        let res = self.raw_client.write_reg(address, data).await;
        self.lock().await;
        res
    }
}


// 2. Instantiate the register block with your wrapper
spmi_register_block! {
    pub struct FgRegs {
        pub config2 => config2_reg,
    }
}

// Generic over LockedSpmiDevice (where `proxy` is an instance of `DeviceType`)
let regs = FgRegs::new(LockedSpmiDevice::new(proxy));
let val = regs.config2().read().await?;
```

### Multi-register contiguous access (`spmi_read_contiguous!`, `spmi_write_contiguous!`)
Perform atomic multi-register contiguous reads and writes type-safely in exactly one async FIDL
call:

```rust
// Read both 'general' and 'status' registers in one call:
let (mut general_val, status_val) = spmi_read_contiguous!(
    &regs,
    my_reg,
    status_be_reg
).await?;

general_val = general_val.set_field1(true);

spmi_write_contiguous!(
    &regs,
    my_reg => general_val,
    status_be_reg => status_val
).await?;
```

> [!IMPORTANT]
> These macros perform **compile-time contiguity validation** using `const` assertions. If you
> attempt to group non-contiguous registers, the code will fail to compile with a clear error
> message, preventing accidental wrong-address accesses at runtime.

#### Custom Address Units (Word-Addressing)
By default, the contiguity check assumes standard **byte-addressing** (`u8` address unit), where
adjacent registers are expected to differ by their byte size (e.g., a step of `2` for `u16`
registers).

If your target hardware is **word-addressed** (meaning sequential 16-bit registers increment the
hardware address by `1` per 16-bit word instead of `2` bytes), you can configure this at the block
level by declaring the `address_unit`:

```rust
spmi_register_block! {
    address_unit: u16, // Contiguous checks will automatically calculate steps relative to 16-bit word units.
    pub struct FgDebugRegs {
        pub r1 => test_word_reg_1,
        pub r2 => test_word_reg_2,
    }
}
```

If no `address_unit` is specified, it defaults to standard byte-addressing (`u8`).

### Typestate access modes (`ReadOnly`, `WriteOnly`, `ReadWrite`)
Use typestate marker traits to restrict access modes at compile-time:

- **ReadOnly**: Only exposes the `read()` method.
- **WriteOnly**: Only exposes the `write()` method.
- **ReadWrite**: Exposes both `read()` and `write()` methods.

### Enum field support
Define named states for bitfields using Rust enums either out-of-line or inline:

#### Out-of-line enum
```rust
#[repr(u16)]
pub enum PowerMode {
    Normal = 0,
    Hibernate = 1,
    Unknown = 0xFFFF,
}

impl PowerMode {
    pub const fn from_val(val: u16) -> Self {
        match val {
            0 => PowerMode::Normal,
            1 => PowerMode::Hibernate,
            _ => PowerMode::Unknown,
        }
    }
}

spmi_register! {
    mode_reg, u16, 0x0A, RW, LE, {
        pub enum PowerMode, mode, set_mode: 3, 2;
    }
}
```

#### Inline enum
```rust
spmi_register! {
    mode_reg, u16, 0x0A, RW, LE, {
        pub enum PowerMode {
            Normal = 0,
            Hibernate = 1,
        }, mode, set_mode: 3, 2;
    }
}

let mut val = regs.mode().read().await?;
val = val.set_mode(mode_reg::PowerMode::Hibernate);
```

## Testing

Run unit tests with:
```posix-terminal
fx test //src/devices/spmi/lib/hwreg-spmi/rust:tests
```
