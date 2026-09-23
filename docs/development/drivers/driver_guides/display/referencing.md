# Referencing information sources in Fuchsia display drivers

This document recommends formats for:

* **references**: a per-project list binding short tags to external information
  sources
* **citations**: one-line comments binding code to a precise location inside a
  referenced document
* **aliases**: one-line records binding the names code uses to the names its
  references use

The formats are optimized for processing by AI agents and scripts, both directly
and through tooling. Human readability matters; human authoring convenience is
explicitly deprioritized.

## Formats for citations, references, and aliases

Summary:

* One record per line behind a distinctive sigil.
    * Citations use `@cite`, references use `@ref`, aliases use `@alias`.
    * One grammar for all records.
* The tag is in parentheses.
* All remaining data is order-insensitive `key=value` fields separated by ` `.

### Grammar

Records:

```abnf
record      = sigil "(" tag ")" ":" 1*( SP field )   ; exactly one SP before each field
sigil       = %s"@cite" / %s"@ref" / %s"@alias"
tag         = lower-alnum *( lower-alnum / "_" / "-" )
field       = key "=" value
key         = lower-alpha *( lower-alnum / "-" )     ; "x-" prefix reserved for experimental keys
value       = bare / quoted
bare        = 1*bare-char
quoted      = DQUOTE *( escaped / quoted-char ) DQUOTE
escaped     = backslash ( DQUOTE / backslash )       ; \" and \\ are the only escapes
bare-char   = %x21 / %x23-7E / %x80-10FFFF           ; excludes SP, HTAB, DQUOTE, all controls
quoted-char = %x20-21 / %x23-5B / %x5D-7E / %x80-10FFFF
                                                     ; excludes DQUOTE, backslash, all controls
backslash   = %x5C
lower-alpha = %x61-7A                                ; a-z
lower-alnum = lower-alpha / DIGIT
```

Source code embedding records:

```abnf
source-line = *WSP "//" *WSP record          ; normative in-source form (citations, aliases,
                                             ; and @ref blocks in foreign files)
readme-line = "* " backtick record backtick  ; README bullet form
backtick    = %x60
```

Grammar details:

* [ABNF (RFC 5234)][abnf] extended with
  [RFC 7405 (case-sensitive string literals)][abnf-case]
* Terminal values are Unicode scalar values.
* Source files are UTF-8.
* Core rules from RFC 5234 Appendix B.1:
    * SP (space, %x20)
    * HTAB (horizontal tab, %x09)
    * DQUOTE (", %x22)
    * DIGIT (0-9, %x30-39)
    * WSP (SP / HTAB)

### References

#### Examples

Owned project README section:

```markdown
## References

* `@ref(virtio): kind=doc title="Virtio 1.4" version=1.4 date=2024-06-27 url=https://docs.oasis-open.org/virtio/... local=local/virtio/virtio-1.4.md`
* `@ref(socdb): kind=db title="SoC register database" version=1.4 local=docs/refs/soc-regs.json`
```

Non-owned project file:

```cpp
// clang-format off
// @ref(freebsd): kind=code title="FreeBSD kernel" version=12.0.0 pin=release/12.0.0 url=https://cgit.freebsd.org/src/ local=local/freebsd
// clang-format on
```

#### Keys

`kind` selects the citation key vocabulary. Required. Valid values:

* `doc` - document
* `code` - reference code
* `db` - database
* `x-` prefix for experimental key vocabulary sets

`title` must match the information source's stated title, when available.
Required.

`version`, `revision`, `date` must match the information source's metadata, when
it exists.

`url` is a canonical web URL for the information source. Strongly recommended.

`pin` is a commit hash or release tag. Required for `kind=code`.

`local` is the recommended path for a local cache of the information source.
Relative to the Fuchsia repository root. AI agents look here first.

#### Placement

Owned projects:

* README section "References" is a bullet list.
* Each bullet is one `@ref` record wrapped in backticks.

Non-owned project files:

* Comment block at the top of the file.
* Each line is one `@ref` record, scoped to that file.

#### Versioning

Default to short unversioned tags. The `@ref` record has versioning information.

Migrating to a new information source version: edit
`version=`/`revision=`/`date=`/`pin=` in one place, then audit every `@cite`.

Use version-bearing tags (`usb32` alongside `usb4`) when newer versions don't
subsume older versions, and a project may need to reference multiple versions.

### Citations

#### Examples

```rust
// @cite(virtio): sec=2.7 title="Virtqueues" page=30-31
// @cite(e-edid): sec=2.2 title="EDID Extension Blocks" page=16 q="if the maximum value of N is ‘1’"
// @cite(freebsd): file=sys/dev/drm2/i915/intel_ddi.c lines=718-735 sym=intel_ddi_mode_set
// @cite(socdb): key=/root/mipi_dsi0/PHY_STATUS bits=4:1
```

The same bytes work after any comment leader:

```cpp
// @cite(e-edid): sec=2.2 title="EDID Extension Blocks" page=16
```

```fidl
// @cite(virtio): sec=5.7.3 title="Feature bits" page=197-198 note="EDID needs feature negotiation"
```

#### Key vocabulary by reference `kind`

##### `doc` (document)

Must contain `sec` (section) or `page`.

`page` is the printed page number; fallback: the PDF sequence number. In
converted Markdown, `page` is the `N` of the nearest preceding `<!-- page N -->`
marker. (Identical semantics for PDF and converted Markdown.)

`title` is the section title. Required if `sec` is present.

`q` (quote) is a short verbatim phrase (10 words or less) anchoring the exact
passage.

##### `code`

Must contain `file`, a path resolved inside the tree pinned by the reference.

Must contain `sym` (symbol / identifier name) or `lines` (`<line>` or
`<start>-<stop>`). Preferably both.

DeviceTree bindings are `kind=code` citations into the kernel tree.

##### `db` (database)

`key` is the primary key identifying the database entry. Required.

##### Universal

`bits=<high>:<low>` or `bits=<bit>` narrows any citation to a bit range.

`note` carries free-form text. (Intended for agent-to-agent handoffs.)

#### Placement

Citations live in plain `//` comments. (Supported languages have `//` comments.)

Declaration-level citations go on `//` lines between the doc comment and the
item and bind to the item that follows. Legal in Rust, and the doc comment still
attaches to the item.

Implementation citations go on their own line immediately above the code they
support and bind to the statement or block that follows.

Citations must not appear in doc comments (`///`, `//!`, `/**`). Linters reject
citations in doc comment blocks.

### Aliases

#### Examples

Identifier:

```rust
struct Timings {
  // @cite(e-edid): sec=2.2 title="EDID Extension Blocks" page=33
  // @alias(e-edid): theirs="vertical addressable line count"
  // @cite(socdb): key=/root/mipi_dsi0/VACTIVE
  height: u16,
}
```

Project-wide concept:

```md
## Aliases

* `@alias(virtio): theirs="used ring" ours="device-owned ring"`
```

#### Keys

`theirs` is the name used by the information source - verbatim. Required.
Unquoted where possible, so a reverse grep lands on the record.

`ours` is the name used in our project. Required by project-wide concepts.
Implicit for identifiers, bound to the identifier name immediately following the
record group.

`note` carries free-form text. (Intended for agent-to-agent handoffs.)

#### Placement

Identifiers, such as register names:

* Same as `@cite`, preceding the identifier declaration.
* Immediately follows a `@cite` with the same tag.

Project-wide concepts:

* Same as `@ref`.
* README: in `## Aliases` section.
* Non-owned project file: same comment block, follows `@ref` with the same tag.

Entries may be whole names or name fragments; tooling translates by
longest-match substitution, and an attached record wins over a fragment rule
where both apply.

### Canonical match patterns

Humans, casual:

```
grep -rn '@cite('         # every citation
grep -rn '@cite(virtio)'  # citations of one document
grep -rn '@ref('          # every reference record
grep -rn '@alias('        # every alias record
```

Tooling, anchored - source scanner:
`^[ \t]*(//[/!]?|\*)[ \t]*@(cite|ref|alias)\(([a-z0-9][a-z0-9_-]*)\): `

Tooling, anchored - README scanner:
``^\* `@(ref|alias)\(([a-z0-9][a-z0-9_-]*)\): ``

The source scanner deliberately also matches `///`, `//!`, and block-comment `*`
lines so a linter can detect misplaced records and reject them rather than
silently skip them.

### Interactions with formatters

Long record lines must survive code formatters.

#### Rust

We assume that [Fuchsia customizations][rustfmt-toml] will not override
the unstable nightly-only `wrap_comments` option, which defaults to `false`.

`rustfmt` offers no inline directive that protects an individual comment.
`#[rustfmt::skip]` does not reliably shield leading comments from wrapping.

#### C++

`clang-format` reflows long `//` comments under the default styles.

Owned project: Add `CommentPragmas: '^ @(cite|ref|alias)\('` to `.clang-format`.
Do not use the much broader `ReflowComments: Never`.

Not owned project: Wrap each block of references and citations in
`// clang-format off` / `// clang-format on` guards.

## Background: Goals and assumptions

### Conceptual model

A *reference* binds a short lowercase *tag* to one external document at one
pinned version. A *citation* names a tag plus a *locator* into that document.
The *kind* of the referenced document determines which locator vocabulary
applies.

An *alias* is a record binding a local name to the name one specific referenced
document uses.

### Information sources

The following sources are supported:

* Paged documents
     * PDFs (native page numbers) and equivalent formats
     * Markdown conversions of PDFs in which the converter inserts
       `<!-- page N -->` markers so page numbers survive conversion.
* Reference code - typically an external tree pinned to a commit or release tag
     * C and C++
     * Rust
     * DeviceTree bindings
* Hardware documentation databases
     * Example: register definition databases
     * Key assumption: primary-key scheme that uniquely identifies a citation's
       target, such as a register or a hardware module

Web pages and other formats are deferred for future consideration.

### Use cases

1. **Fact-checking** - Given Fuchsia source code plus its citations, a human or
   AI developer reads each cited location and verifies the code against the
   source.
2. **Subagent communication** - AI agents exchange claims grounded in citations.
   A citation must be self-contained enough to hand to another agent.
3. **Topic search** (secondary) - Human and AI researchers survey all available
   information sources and produce a set of citations relevant to a topic.

### Name aliases

Code regularly names things differently from its references:
* vendor terms get inclusive replacements
* acronyms get expanded for clarity
* the references disagree among themselves

Both fact-checking (translating code identifiers into a document's vocabulary
before matching) and reverse lookup (grepping the codebase for a vendor name)
depend on the mapping being explicit.

[abnf]: https://datatracker.ietf.org/doc/html/rfc5234
[abnf-case]: https://datatracker.ietf.org/doc/html/rfc7405
[rustfmt-toml]: /rustfmt.toml
