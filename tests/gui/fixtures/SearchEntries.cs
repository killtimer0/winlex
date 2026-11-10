namespace SearchFixture;

public class EntryType {
    public const int EntryConstant = 17;
    public int EntryField;
    public int EntryProperty { get; set; }
    public event System.Action EntryEvent { add {} remove {} }
    public void EntryMethod() {}
}

public class Apis {
    public const int EntryConstant = 18;
    public static void EntryMethod() {}
    [System.Runtime.InteropServices.DllImport("kernel32.dll", EntryPoint = "GetCurrentProcessId")]
    public static extern uint EntryFunction();
}
