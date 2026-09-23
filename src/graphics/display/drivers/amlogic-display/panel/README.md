# Display Panel Configurations

This directory stores the MIPI-DSI panel configurations, including the
initialization / shutdown sequences for all the display panels supported by the
driver.

Each panel must have its own file for its configurations. The file must be named
`{panel manufacturer}-{panel model}-{DDIC manufacturer}-{DDIC model}[-{variant}].h`,
where all the fields must be in lower case alphanumeric characters.

## DSI Sequence Code

### Style guide

For better readability, all the DSI sequence code must follow the following
format guidelines:

* Indentation of 4 spaces in arrays.

* All hexadecimal numbers use the `0xaa` format.

* Only leave one space (ASCII 0x20) between values; do not align the numbers
  using tabs / spaces.

* Only use `//` for comments.

* Each separate DSI command must be on its own line.
  - All command parameters must be on the same line. Line wrapping is not
    allowed to make it easier for tools / scripts to parse the code.
  - The command type must be a `DsiOpcode` or a MIPI-DSI data type constant.
  - The payload size must be in decimal.
