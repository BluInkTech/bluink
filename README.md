# Bluink backend

This repository provides backend source and build tooling for reproducible Bluink
releases. The frontend is distributed separately as a compiled, proprietary npm
package. Public builds consume its exact version; they do not rebuild its source.

For product information, documentation and support, visit [bluink.app](https://bluink.app).

## Build

Use Node.js 25 or newer and the Rust toolchain pinned in `rust-toolchain.toml`.
Build on a supported host: Linux x64, Windows x64 or macOS arm64.

```sh
npm ci
npm run build
npm test
```

Resource preparation downloads checksum-pinned upstream inputs. The included
synthetic tests do not represent the complete product verification suite.
Windows source builds use 7-Zip installed at `%ProgramFiles%\7-Zip\7z.exe`
to decompress upstream XZ archives. GitHub's Windows runner includes it;
installed Bluink packages do not require it.

## Feedback and contributions

We welcome high-quality bug reports, feature suggestions and constructive
discussion about improving Bluink.

This repository does not currently accept pull requests. Complete product
verification relies on private end-to-end tests and datasets that are not
distributed here, so we cannot provide contributors with the complete validation
environment. Public build and synthetic-test failures are welcome bug reports.

AI-assisted tools have made generating implementation code more accessible.
Much of the continuing work remains in product design, integration, testing and
maintenance. Clear problem reports and thoughtful feedback are especially valuable.

## Licensing

Backend source and Bluink-owned public build tooling use MPL-2.0. Generated
browser contracts use MIT. The separately distributed frontend is proprietary.
Third-party components retain their own licences and notices. These contribution
guidelines do not restrict rights granted by the applicable software licences.
