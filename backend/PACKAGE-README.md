# Bluink CLI

Install and run Bluink with Node.js 25 or newer:

```sh
npm install --global @bluink/cli@alpha
bluink --help
bluink server --project ./my-writing
```

The package supports Windows x64, Linux x64 with glibc, and macOS arm64.
Its exact dependencies provide the compiled interface, fonts and PDF engine.
Installation requires network access; ordinary offline use needs no resource
downloads or compiler. Calendar versions use YY.M.BUILD with an optional
prerelease suffix.

Bluink-owned backend code is MPL-2.0; the compiled frontend is proprietary.
Component licences and third-party notices accompany their packages. Public
source and build verification are linked from the release's repository metadata.

For documentation, product information and feedback, visit
[bluink.app](https://bluink.app).
