<!--

// LINT.IfChange

-->

# gpioutil

List, read from, write to, and configure GPIOs.

## Usage

```none
gpioutil <command> [<name>] [<value>]
```

## Commands

### list {#list}

```none
gpioutil list
```

List the known GPIOs.

`list` returns values like this:

```none
[gpio-0] GPIO_HW_ID_3
```

The value inside the brackets (`gpio-0`) is a reference to the pin index.
The value after the brackets (`GPIO_HW_ID_3`) is the `<name>` value to
provide to other `gpioutil` commands. See [GPIO names](#names) for more details.

Aliases: `l`

### read

```none
gpioutil read <name>
```

Read the current value of a GPIO that's configured as OUT.

`<name>` should be one of the values returned from [`list`](#list).

Possible return values are `0` (LOW) or `1` (HIGH).

Aliases: `r`

### in

```none
gpioutil in <name>
```

Configure a GPIO as IN.

`<name>` should be one of the values returned from [`list`](#list).

This command doesn't return a value.

Aliases: `i`

### out

```none
gpioutil out <name> <value>
```

Configure a GPIO as OUT.

`<name>` should be one of the values returned from [`list`](#list).

`<value>` is the initial OUT value. Accepted values are
`0` (LOW) or `1` (HIGH).

This command doesn't return a value.

Aliases: `o`

### drive

```none
gpioutil drive <name> [<value>]
```

Get or set the drive strength of a GPIO in microamps.

`<name>` should be one of the values returned from [`list`](#list).

When `<value>` is omitted, `drive` returns the current drive strength
of the GPIO in microamps.

When `<value>` is provided, `drive` updates the drive strength of the
GPIO. `<value>` should be in microamps.

Aliases: `d`

### interrupt

```none
gpioutil interrupt <name> <value>
```

Get the GPIO interrupt and wait for it to be triggered once.

`<name>` should be one of the values returned from [`list`](#list).

`<value>` is the interrupt mode to use. Accepted values are `default`,
`edge-high`, `edge-low`, `edge-both`, `level-low`, or `level-high`.

Aliases: `q`

### function

```none
gpioutil function <name> <value>
```

Set the function for a pin.

`<name>` should be one of the values returned from [`list`](#list).

`<value>` is a function number or string name that is specific to the GPIO
controller being used.

Aliases: `f`

### pull

```none
gpioutil pull <name> <value>
```

Configure the pull-up or pull-down resistor for a pin.

`<name>` should be one of the values returned from [`list`](#list).

`<value>` is the resistor pull. Accepted values are `down`, `up`, or `none`.

Aliases: `p`

### help

```none
gpioutil help
```

Print help text.

Aliases: `h`

## Examples

### List all known GPIOs

```none {:.devsite-disable-click-to-copy}
$ gpioutil list
[gpio-0] GPIO_HW_ID_3
[gpio-1] GPIO_SOC_TH_BOOT_MODE_L
...
```

### Read the current value of a GPIO

```none {:.devsite-disable-click-to-copy}
$ gpioutil read GPIO_HW_ID_3
GPIO Value: 1
```

### Write a LOW value to a GPIO

```none {:.devsite-disable-click-to-copy}
$ gpioutil write GPIO_HW_ID_3 0
```

### Configure a GPIO as IN with a pull-down resistor

```none {:.devsite-disable-click-to-copy}
$ gpioutil in GPIO_HW_ID_3 down
```

### Configure a GPIO as OUT with an initial value of HIGH

```none {:.devsite-disable-click-to-copy}
$ gpioutil out GPIO_HW_ID_3 1
```

### Get the current drive strength of a GPIO in microamps

```none {:.devsite-disable-click-to-copy}
$ gpioutil drive GPIO_HW_ID_3
Drive Strength: 500 ua
```

### Set the drive strength of a GPIO to 500 microamps

```none {:.devsite-disable-click-to-copy}
$ gpioutil drive GPIO_HW_ID_3 500
Set drive strength to 500
```

### Wait for a falling edge on a GPIO

```none {:.devsite-disable-click-to-copy}
$ gpioutil interrupt GPIO_HW_ID_3 edge-low
Received interrupt at time 12345
```

### Set a pin to function six

```none {:.devsite-disable-click-to-copy}
$ gpioutil function GPIO_HW_ID_3 6
```

### Set a pin to function by name

```none {:.devsite-disable-click-to-copy}
$ gpioutil function GPIO_HW_ID_3 spi
```

## Notes

<<./_access.md>>

### GPIO names {#names}

GPIO names are defined in the driver source code and usually match the
datasheet's name for the GPIO. See the `DECL_GPIO_PIN` statements in
[`vim3-gpio.cc`] for an example.

[`vim3-gpio.cc`]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/board/drivers/vim3/vim3-gpio.cc;l=72

### Source code

Source code for `gpioutil`: [`//src/devices/gpio/bin/gpioutil/`][src]

[src]: https://cs.opensource.google/fuchsia/fuchsia/+/main:src/devices/gpio/bin/gpioutil/

<!--

// LINT.ThenChange(//src/devices/gpio/bin/gpioutil/main.cc)

-->
