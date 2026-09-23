#[allow(clippy::unreadable_literal)]
static PROPERTY_KEYS: phf::Set<&'static str> = ::phf::Set { map: ::phf::Map {
    key: 16263683158343804936,
    disps: &[
        (0, 0),
    ],
    entries: &[
        ("properties", ()),
        ("patternProperties", ()),
    ],
} };
#[allow(clippy::unreadable_literal)]
static NON_SCHEMA_KEYS: phf::Set<&'static str> = ::phf::Set { map: ::phf::Map {
    key: 16287231350648472473,
    disps: &[
        (0, 0),
        (0, 1),
        (0, 1),
        (0, 7),
    ],
    entries: &[
        ("dependencies", ()),
        ("anyOf", ()),
        ("const", ()),
        ("allOf", ()),
        ("properties", ()),
        ("dependentSchemas", ()),
        ("definitions", ()),
        ("dependentRequired", ()),
        ("patternProperties", ()),
        ("enum", ()),
        ("$defs", ()),
        ("oneOf", ()),
    ],
} };#[allow(clippy::unreadable_literal)]
static BOOLEAN_SCHEMA_ARRAY_KEYS: phf::Set<&'static str> = ::phf::Set { map: ::phf::Map {
    key: 4203492208743950414,
    disps: &[
        (0, 0),
        (0, 1),
    ],
    entries: &[
        ("items", ()),
        ("allOf", ()),
        ("anyOf", ()),
        ("oneOf", ()),
    ],
} };#[allow(clippy::unreadable_literal)]
static FINAL_KEYS: phf::Set<&'static str> = ::phf::Set { map: ::phf::Map {
    key: 16263683158343804936,
    disps: &[
        (0, 0),
        (0, 2),
    ],
    entries: &[
        ("enum", ()),
        ("required", ()),
        ("type", ()),
        ("default", ()),
    ],
} };#[allow(clippy::unreadable_literal)]
const ALLOW_NON_CONSUMED_KEYS: phf::Set<&'static str> = ::phf::Set { map: ::phf::Map {
    key: 16287231350648472473,
    disps: &[
        (0, 3),
        (1, 0),
        (0, 3),
        (2, 6),
    ],
    entries: &[
        ("$schema", ()),
        ("format", ()),
        ("description", ()),
        ("title", ()),
        ("examples", ()),
        ("$anchor", ()),
        ("$defs", ()),
        ("definitions", ()),
        ("default", ()),
        ("$id", ()),
    ],
} };