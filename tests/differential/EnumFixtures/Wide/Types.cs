namespace Fixtures.Shared { public enum Choice : ulong { High = 0xfedcba9876543210UL } }
namespace Windows.Win32.Foundation.Metadata
{
    public sealed class SupportedArchitectureAttribute : System.Attribute
    {
        public SupportedArchitectureAttribute(Fixtures.Shared.Choice value) { }
    }
}
