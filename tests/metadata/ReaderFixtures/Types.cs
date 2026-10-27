using System.Runtime.InteropServices;

namespace Fixtures;

[StructLayout(LayoutKind.Explicit)]
public struct Nonzero {
    [FieldOffset(0)] public int A;
    [FieldOffset(8)] public int B;
}

[StructLayout(LayoutKind.Explicit, Size=32)]
public struct SizedOverlay {
    [FieldOffset(0)] public int A;
    [FieldOffset(0)] public long B;
}

[StructLayout(LayoutKind.Explicit)]
public struct Overlay {
    [FieldOffset(0)] public int A;
    [FieldOffset(0)] public long B;
}

public class OuterA { public struct Child { public int Value; } }
public class OuterB { public struct Child { public long Value; } }
public struct Holder { public OuterA.Child A; public OuterB.Child B; }
public struct External { public System.DateTime Value; }

public interface IBox<T> {
    T Value { get; set; }
    event System.Action<T> Changed;
    U Convert<U>(T input, U fallback);
    System.Collections.Generic.IEnumerable<T[]> Arrays();
}
public struct Box<T> { public T Value; }
public struct Box<T, U> { public T First; public U Second; }
public struct BoxUses { public Box<int> One; public Box<int, long> Two; }
