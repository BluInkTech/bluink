# Bluink CLI

Install and run Bluink with Node.js 25 or newer:

```sh
npm install --global @bluink/cli@alpha
bluink --help
bluink server --project ./my-writing
```

The package supports Windows x64, Linux x64 with glibc, and macOS arm64.
The compiled interface is bundled in this package. Its exact npm dependencies
provide fonts and the PDF engine.
Installation requires network access; ordinary offline use needs no resource
downloads or separate compiler installation. HTML, websites, PDF and Word
(DOCX) share reusable publication themes. PDF uses the packaged Typst engine;
Word is generated natively with embedded offline fonts. No Python installation
is required. Calendar versions use YY.M.BUILD with an optional
prerelease suffix.

```sh
bluink export docx --project ./my-writing --note PAGE_ID --out ./new-note.docx
bluink export docx --project ./my-writing --note PAGE_ID --include-linked --out ./new-book.docx
bluink publication generate --project ./my-writing --id PUBLICATION_ID --format docx --output ./new-publication.docx
```

Output paths must be new; Bluink does not overwrite an existing export.
Use the app's Publication themes workspace to create a reusable theme, preview
all four outputs, and apply shared or format-specific appearance overrides.

Bluink-owned backend code is MPL-2.0; the compiled frontend is proprietary.
Component licences and third-party notices accompany their packages. Public
source and build verification are linked from the release's repository metadata.

For documentation, product information and feedback, visit
[bluink.app](https://bluink.app).
