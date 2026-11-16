# Source reload inputs

`before.winmd` defines `GuiFixture.ReloadSnapshot` with `OriginalField`.
`after.winmd` replaces that field with `UpdatedField` and adds `GuiFixture.ReloadVersionTwo`.
The GUI workflow overwrites one source path to verify new searches and retained document snapshots independently.

Regenerate both inputs from the repository root:

```powershell
cargo run -p winlex-core --example gui-fixtures --locked
```
