# Release preparation

This is an experimental query-metadata generator, not an upstream Microsoft
WinMD format extension or a ready-to-link binding generator.

Before public distribution:

- Retain the generator's GPL-3.0-only `LICENSE` and the separate licenses of
  third-party components in the source package.
- Review the applicable Microsoft terms before redistributing generated metadata.
  Do not include SDK or MSVC headers in the source package.
- Run `build.ps1 -Check` from an extracted source archive with no existing
  `out/`, `target/` or a Winlex checkout. Record external tool versions
  through the generated manifest. A clean extraction is not a clean OS image;
  use an independent machine or CI runner before claiming broad reproducibility.
- Retain all bundled third-party license texts and local patch notes.
- Describe public coverage and metadata conventions, including unsupported variables,
  macro/configuration limits and unverified native ARM64 layouts.
- Run optional Winlex integration tests only when a compatible CLI is available;
  report not-run checks separately from passing checks.

`package-source.ps1` packages an allowlist of source, configuration, tests and
notices. It excludes generated metadata, local baselines, logs, SDK headers and
build outputs. Its destination must not already exist. It does not publish or
upload anything. Package generation does not establish generated-metadata
redistribution rights.
