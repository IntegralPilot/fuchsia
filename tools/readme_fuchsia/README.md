# Readme Fuchsia Tool

The `readme_fuchsia` library provides a robust parser, formatter, and strict schema validator for Fuchsia's custom plain-text `README.fuchsia` files. It is primarily used by tools like `check-licenses` and `shac` to ensure formatting consistency and schema compliance.

## How to use

The library can be used as a standalone binary or imported as a Go library.

### Standalone CLI

You can use the standalone tool to validate a `README.fuchsia` file locally:

```bash
fx readme_fuchsia validate [--project-root <dir>] [--allow-missing-license] <path/to/README.fuchsia>
```

### Go Library

```go
import "go.fuchsia.dev/fuchsia/tools/readme_fuchsia"

// Parsing
readmes, err := readme_fuchsia.ParseFile("path/to/README.fuchsia")

// Validating
errs := readme_fuchsia.Validate("path/to/project/root", readmes)

// Formatting
formattedText := readme_fuchsia.Format(readmes)
```

## Adding or Removing Fields

The `readme_fuchsia` parser and formatter are completely dynamic and powered by Go reflection. You do **not** need to edit the parser or formatter logic (`parser.go` or `formatter.go`) to add or remove fields.

To add a new field (e.g., `Maintained By`), simply add it to the `Readme` struct in `types.go`:

```go
type Readme struct {
	...
	SecurityCritical         string   `readme:"Security Critical"`
	MaintainedBy             string   `readme:"Maintained By"`
	...
}
```

### Struct Ordering

**The order of fields in `types.go` strictly dictates the order they are printed by the formatter.** If you want a new field to appear below `Security Critical`, simply place it below `SecurityCritical` in the `Readme` struct definition.

### Available Attributes

The struct tags on the `Readme` struct control exactly how each field is parsed and formatted:

- `` `readme:"<Directive>"` ``: The exact string directive expected in the text file (e.g., `readme:"Upstream Git"` maps to `Upstream Git:`).
- **Aliases:** You can provide a comma-separated list of fallback directives for backwards compatibility (e.g., `` `readme:"Local Modifications,Modifications"` ``). The **first** alias is always the one used as the canonical key when formatting the file back out.
- `` multiline:"true" ``: Allows the field's value to span multiple lines. The formatter will automatically indent the text appropriately, and the parser will automatically group indented lines together.
- `` separator:"," ``: Used exclusively for Go slice types (e.g., `[]string`). Tells the parser to split the field value into a list. Note: For slices, the formatter handles specific legacy formatting logic (e.g., printing comma-separated on a single line vs. repeating the key on multiple lines).
- `` `readme:"-"` ``: Specifically instructs the reflection engine to ignore this field.

### Unrecognized Fields

Any key-value pair that is not explicitly defined in `types.go` is safely captured in the `UnknownFields` slice. This ensures that the formatter does not accidentally delete data it doesn't recognize. Strict tools like SHAC can inspect `UnknownFields` and fail validation to enforce a strict schema.
