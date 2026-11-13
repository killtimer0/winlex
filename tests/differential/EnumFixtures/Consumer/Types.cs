extern alias wide;
extern alias narrow;

[wide::Windows.Win32.Foundation.Metadata.SupportedArchitecture(wide::Fixtures.Shared.Choice.High)]
[narrow::Windows.Win32.Foundation.Metadata.NativeArrayInfo(narrow::Fixtures.Shared.Choice.Negative)]
public struct Attributed { }
