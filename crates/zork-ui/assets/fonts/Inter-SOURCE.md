Inter by The Inter Project Authors, https://github.com/rsms/inter.
License: SIL Open Font License 1.1; see Inter-OFL.txt.

Source variable fonts already vendored in this repository:
- `InterVariable.ttf`: SHA-256 `1d81bda8fcebb6593b05a670381198e64fe8861f6fc0d8fbe973e4fd56f1d831`
- `InterVariable-Italic.ttf`: SHA-256 `87abc103451574f8bd8657f4a9a13c1b958b2deb25dc8ac51397b871204561bf`

The `static/Inter-*.ttf` assets are generated from those sources with fonttools
4.62.1 by `uv run scripts/design-pc/prepare_fonts.py`: optical size 14, weights
400/500/600/700, regular and italic. The static outlines preserve the existing
font design and avoid repeated CoreText variable-font setup on multiline text.
The native client and design app register the same faces. Keep these generated assets
in source control so an ordinary Cargo build needs no Python font toolchain.
