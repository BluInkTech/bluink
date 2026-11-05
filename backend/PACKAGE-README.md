# Bluink CLI

Install and run Bluink with Node.js 26 or newer:

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

Local Git history works offline without an installed Git executable. To exchange changes, connect a local bare repository or an SSH/HTTPS Git URL. Use Settings › Sync for credentials and SSH host trust. Close any server using the same project before a standalone sync or restore.

```sh
bluink history --project ./my-writing --limit 10
bluink sync --project ./my-writing --origin /absolute/path/to/writing.git --create
bluink sync --project ./my-writing
bluink sync --project ./my-writing --status
bluink sync --open-from /absolute/path/to/writing.git --destination ./second-copy
bluink restore --project ./my-writing --checkpoint COMMIT_ID --item PAGE_ID
bluink restore --project ./my-writing --checkpoint COMMIT_ID --item PAGE_ID --confirm --review-token REVIEWED_TOKEN
```

The first restore prints a preview only. Use its `reviewToken` for the confirmed restore. A changed project needs a fresh preview. Omitting `--item` previews or restores the entire project. `bluink sync --disconnect` removes only the destination.

Bluink-owned backend code is MPL-2.0; the compiled frontend is proprietary.
Component licences and third-party notices accompany their packages. Public
source and build verification are linked from the release's repository metadata.

For documentation, product information and feedback, visit
[bluink.app](https://bluink.app).
