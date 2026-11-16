# Source reload inputs

`before.winmd` defines `GuiFixture.ReloadSnapshot` with `OriginalField`.
`after.winmd` replaces that field with `UpdatedField` and adds `GuiFixture.ReloadVersionTwo`.
The GUI workflow overwrites one source path to verify new searches and retained document snapshots independently.
Both versions also define `ReloadHolder`, whose field references `ReloadSnapshot`,
for declaration navigation within a retained snapshot.

Regenerate both inputs from the repository root:

```powershell
cargo run -p winlex-core --example gui-fixtures --locked
```

## Search and dependency inputs

`search_generate.rs` defines two sources with a shared type name but different
fields, distinct query targets, and a consumer with an external value-type field.
The dependency declares two int32 fields; the consumer places it between uint32
fields, giving a 16-byte structure with field offsets 0, 4, and 12.

```powershell
cargo run -p winlex-core --example gui-search-fixtures --locked -- tests/gui/fixtures
```

## C# generic inputs

`ReaderFixtures.winmd` is the unchanged managed assembly built from
[`tests/metadata/ReaderFixtures`](../../metadata/ReaderFixtures). Its C# definitions
provide an independent input for generic substitution and architecture checks.
No CLR or compiler is required on GUI targets.

```powershell
dotnet build tests/metadata/ReaderFixtures/ReaderFixtures.csproj -c Debug -o tmp/verify/reader-fixtures --nologo
Copy-Item tmp/verify/reader-fixtures/ReaderFixtures.dll tests/gui/fixtures/ReaderFixtures.winmd
```
