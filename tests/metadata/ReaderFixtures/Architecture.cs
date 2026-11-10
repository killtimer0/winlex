namespace Windows.Win32.Foundation.Metadata {
    [System.Flags]
    public enum Architecture { X86 = 1, X64 = 2, Arm64 = 4 }

    [System.AttributeUsage(System.AttributeTargets.Struct)]
    public sealed class SupportedArchitectureAttribute : System.Attribute {
        public SupportedArchitectureAttribute(Architecture architecture) { }
    }
}

namespace Fixtures {
    [Windows.Win32.Foundation.Metadata.SupportedArchitecture(
        Windows.Win32.Foundation.Metadata.Architecture.X64)]
    public struct X64Argument { public int Value; }
}
