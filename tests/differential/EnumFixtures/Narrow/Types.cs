namespace Fixtures.Shared { public enum Choice : short { Negative = -12345 } }
namespace Windows.Win32.Foundation.Metadata
{
    public sealed class NativeArrayInfoAttribute : System.Attribute
    {
        public NativeArrayInfoAttribute(Fixtures.Shared.Choice value) { }
    }
}
