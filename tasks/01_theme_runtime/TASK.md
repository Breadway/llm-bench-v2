# Shared theme tokens and app stylesheet cascade

Implement the pure-Rust part of the shared GTK theme pipeline in `src/lib.rs`. The caller loads
the shared sheet and each app's local sheet separately. App declarations always win for the
same selector/property, independent of load order; later declarations within one origin win.
An invalid sheet must not partially change the cascade.

`Color::parse_hex` accepts only CSS `#RGB` and `#RRGGBB` ASCII hexadecimal forms. Compute
WCAG relative luminance and contrast ratio from sRGB values. `readable_ink` returns whichever
of pure black or white has the higher contrast against the receiver.

`CssCascade::load` accepts simple rules of the form `selector[, selector] { name: value; }`.
Whitespace around selectors, properties and values is ignored. Empty declarations are allowed;
empty selectors, missing braces, missing colons, or empty names/values are errors. Selector and
property matching is exact and case-sensitive. No external CSS engine is available.

Keep the public API as declared. The suite also checks that a bad app update leaves the previously
loaded styles intact. Avoid byte slicing that can panic on malformed UTF-8 input.
