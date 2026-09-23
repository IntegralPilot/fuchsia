# Scenic CPP Testing

This directory contains cpp fakes for Scenic protocols. These libraries
are part of the Fuchsia API surface for testing.

These are lightweight fake implementations that have no side effects
besides mutating their own internal state according to the rules of
interacting with the APIs. It makes that internal state available for
inspection by a test. Most commonly a unittest interacting with Scenic
is a good fit.

## Usage

These fakes provide data structures that expose a structured form of the
internal state, such as FakeFlatland::FakeGraph. Use unit testing libraries
such as gtest to add expectations against the results.

Using piecewise member matchers such as testing::FieldsAre() can cause
breakages as the fakes evolve. Instead, use member matcher based on pointers
such as testing::Field().

```
AllOf(Field("root_transform",
            &FakeGraph::root_transform,
            Pointee(AllOf(Field("translation",
                                &FakeTransform::translation,
                                FakeTransform::kDefaultTranslation),
                          Field("scale", &FakeTransform::scale, scale),
                          Field("opacity",
                                &FakeTransform::opacity,
                                FakeTransform::kDefaultOpacity),
                          Field("children",
                                &FakeTransform::children,
                                ElementsAreArray(child_transform_matchers))))),
      Field("view",
            &FakeGraph::view,
            Optional(AllOf(
                Field("view_token", &FakeView::view_token, view_token_koid),
                Field("parent_viewport_watcher",
                      &FakeView::parent_viewport_watcher,
                      watcher_koid)))))
```

## Evolution

This sections explains some tips for devs making a change in these fakes. They
should be careful when making changes to the internal data structures exposed
from the fakes to not break out-of-tree dependencies.

* Evolve the fakes as the protocols evolve. Align changes with the
corresponding FIDL additions and deprecations. Use the C++ availability macros
in `<zircon/availability.h>` to mark the availability. For example, to reference
a FIDL type that was added in 15 and removed at 17, you'd have to use
`#if FUCHSIA_API_LEVEL_AT_LEAST(15) && FUCHSIA_API_LEVEL_LESS_THAN(17)`.
* Adding a new field should be seamless for the clients using non-piecewise
matchers, such as testing::Field().
* Removing a field can cause breakages. Make sure there are no expectations
looking for these fields before deprecating. If you are removing a field that
is also deprecated from FIDL, use the C++ availability macros.
* Avoid renaming fields when possible.