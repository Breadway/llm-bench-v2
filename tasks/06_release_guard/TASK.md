# Clean-clone release and lockfile guard

Implement the release checks used before publishing ecosystem crates. Parse semantic versions
with three numeric components, optional prerelease identifiers and optional build metadata. A
leading `v` is accepted. Numeric core components cannot have leading zeroes. Compare prerelease
identifiers by SemVer rules: numeric identifiers compare numerically and precede text identifiers;
a stable version sorts after prereleases with the same core version.

Stable updates choose the greatest newer stable version. Dev updates choose the greatest newer
version whose first prerelease identifier is `dev`; stable and `rc` tags are excluded.

`inspect_lock` reads Cargo.lock `[[package]]` records and checks each expected package's exact
version and source prefix. A missing package, version drift and an absent or wrong source are
distinct issues. This catches local path patches that make a developer build pass while a clean
CI clone with `--locked` cannot reproduce it. Ignore dependency-array entries when parsing.
