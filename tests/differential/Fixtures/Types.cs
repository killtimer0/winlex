using System;
using System.Runtime.InteropServices;

namespace Differential
{
    [StructLayout(LayoutKind.Explicit, Size = 24, Pack = 2)]
    public struct Layout
    {
        [FieldOffset(0)] public int First;
        [FieldOffset(8)] public long Second;
    }

    public unsafe class Box<T>
    {
        public const long Minimum = long.MinValue;
        public const ulong Maximum = ulong.MaxValue;
        public const float NegativeZero = -0.0f;
        public const double Infinity = double.PositiveInfinity;
        public const string Text = "metadata 汉字";
        public T Item;
        public int* Pointer;
        public int** PointerPointer;
        public T[] Items;
        public T Value { get; set; }
        public event Action<T> Changed { add { } remove { } }
        public U Convert<U>(T input, ref U output) => output;
        public void Arrays(ref T[] values) { }
        public class Nested { }
    }

    public static class Imports
    {
        [DllImport("kernel32.dll", EntryPoint = "GetTickCount", ExactSpelling = true)]
        public static extern uint Tick();
    }

    [Windows.Win32.Foundation.Metadata.Guid(0x12345678, 0xabcd, 0xef01)]
    [Windows.Win32.Foundation.Metadata.NativeArrayInfo(CountConst = 17, CountParamIndex = 2)]
    public struct Attributed { public int Number; }
}

namespace Windows.Win32.Foundation.Metadata
{
    public sealed class GuidAttribute : Attribute
    {
        public GuidAttribute(uint a, ushort b, ushort c) { }
    }

    public sealed class NativeArrayInfoAttribute : Attribute
    {
        public int CountConst;
        public short CountParamIndex { get; set; }
    }
}
