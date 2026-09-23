# Font provider service

Fuchsia offers a discoverable protocol called
[`fuchsia.fonts.Provider`](https://fuchsia.dev/reference/fidl/fuchsia.fonts#Provider)
for requesting font assets.

A client may obtain a digital font file for a font family with
[`Provider.GetTypeface()`](https://fuchsia.dev/reference/fidl/fuchsia.fonts#Provider.GetTypeface),
which specifies query parameters, as well as fallback
and match logic. If satisfiable, the method returns an appropriate font file
buffer and some metadata.

The client may also query the styles available within a font family, with
[`Provider.GetFontFamilyInfo()`](https://fuchsia.dev/reference/fidl/fuchsia.fonts#Provider.GetFontFamilyInfo).
