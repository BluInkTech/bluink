# @bluink/fonts

Offline fonts for Bluink's interface and publication generators. Each family
includes web, PDF and static Word faces from checksum-pinned upstream sources.

The package is installed by `@bluink/cli`. It contains prepared assets and
requires no installation scripts or first-use downloads. Bluink-owned packaging
code is MPL-2.0; the fonts retain their upstream OFL-1.1 licences, included
with the assets.

For product information, visit [bluink.app](https://bluink.app).

Build with `npm run build --workspace @bluink/fonts`. The pinned `harfbuzzjs`
development dependency instances variable fonts through WebAssembly in Node.
No Python installation or native build toolchain is needed for the fonts build.
Installed export reads prepared assets in Rust and does not run this build tool.

`fontDirectory('docx')` provides complete regular, bold, italic and bold-italic
TrueType faces plus their checksummed manifest and upstream licences. All
variation axes are pinned; weight uses 400/700 and other axes use their upstream
defaults. Every glyph is retained. Derived faces use stable Bluink family aliases
to respect upstream reserved names and avoid confusion with installed variable
fonts. Theme and UI font IDs continue to identify the original catalogue family.
