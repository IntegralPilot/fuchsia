# mos-tool

A command-line tool for interacting with the MOS (Managed OS) service.

## Building the tool

1. Add the tool to your build configuration:
   ```
   fx set <product.board> --with-host //build/assembly/tools/mos_tool:host
   ```
2. Build the tool:
   ```
   fx build
   ```

## Running the tool

Once built, you can run the tool using `fx`:
```
fx mos-tool <command> <args>
```
