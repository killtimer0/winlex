# Release preparation

Before public distribution:

- Retain the generator's `LICENSE`, third-party license texts and local patch
  notes in the source package.
- Review the applicable Microsoft terms before redistributing generated metadata.
  Do not include SDK or MSVC headers in the source package.
- Run `build.ps1 -Check` from an extracted source archive with no existing
  `out/` or `target/`, on an independent machine or CI runner.
- Describe public coverage and metadata conventions and their limitations as
  documented in `README.md`.
- Run optional Winlex integration tests only when a compatible CLI is available;
  report not-run checks separately from passing checks.

Create the source archive with `package-source.ps1`; usage and archive contents
are documented in [README.md](README.md#source-packaging).
