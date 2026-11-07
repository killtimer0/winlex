using System.Collections.Immutable;
using System.Globalization;
using System.Reflection.Metadata;
using System.Reflection.Metadata.Ecma335;
using System.Reflection.PortableExecutable;
using System.Text.Json;
using System.Text.Json.Nodes;

using var stream = File.OpenRead(args[0]);
using var pe = new PEReader(stream);
var r = pe.GetMetadataReader(MetadataReaderOptions.None);
var provider = new Types(r);
var attributeProvider = new AttributeTypes(r);
int Token(EntityHandle h) => h.IsNil ? 0 : MetadataTokens.GetToken(h);
object Identity(StringHandle name, Version version, StringHandle culture, BlobHandle key,
    System.Reflection.AssemblyFlags flags)
{
    var bytes = r.GetBlobBytes(key);
    if ((flags & System.Reflection.AssemblyFlags.PublicKey) != 0 && bytes.Length > 0)
        bytes = System.Security.Cryptography.SHA1.HashData(bytes).TakeLast(8).Reverse().ToArray();
    return new { name = r.GetString(name), version = new[] { version.Major, version.Minor, version.Build, version.Revision },
        culture = r.GetString(culture), public_key_token = Convert.ToHexString(bytes).ToLowerInvariant(), content_type = (int)flags & 0xe00 };
}
Dictionary<string, object> Rows<T>(IEnumerable<T> handles, Func<T, EntityHandle> entity, Func<T, object> read)
    => handles.ToDictionary(h => $"0x{Token(entity(h)):x8}", read);
IEnumerable<EntityHandle> Table(TableIndex table) => Enumerable.Range(1, r.GetTableRowCount(table))
    .Select(row => MetadataTokens.EntityHandle(((int)table << 24) | row));
object[] Accessors(IEnumerable<(int flags, MethodDefinitionHandle method)> items) => items
    .Where(a => !a.method.IsNil).OrderBy(a => Token(a.method)).ThenBy(a => a.flags)
    .Select(a => (object)new { a.flags, method = Token(a.method) }).ToArray();
var ad = r.GetAssemblyDefinition();
var propertyParents = r.TypeDefinitions.SelectMany(t => r.GetTypeDefinition(t).GetProperties().Select(p => (p, t))).ToDictionary(p => p.p, p => p.t);
var eventParents = r.TypeDefinitions.SelectMany(t => r.GetTypeDefinition(t).GetEvents().Select(e => (e, t))).ToDictionary(p => p.e, p => p.t);
var interfaceParents = r.TypeDefinitions.SelectMany(t => r.GetTypeDefinition(t).GetInterfaceImplementations().Select(i => (i, t))).ToDictionary(p => p.i, p => p.t);
var result = new Dictionary<string, object>
{
    ["assembly"] = Identity(ad.Name, ad.Version, ad.Culture, ad.PublicKey, ad.Flags),
    ["references"] = r.AssemblyReferences.Select(h => { var a = r.GetAssemblyReference(h);
        return Identity(a.Name, a.Version, a.Culture, a.PublicKeyOrToken, a.Flags); }).ToArray(),
    ["types"] = Rows(r.TypeDefinitions, h => h, h => {
        var t = r.GetTypeDefinition(h); var l = t.GetLayout();
        return new { name = r.GetString(t.Name), @namespace = r.GetString(t.Namespace), flags = (int)t.Attributes,
            extends = t.BaseType.IsNil ? (int?)null : Token(t.BaseType),
            fields = t.GetFields().Select(f => Token(f)).ToArray(), methods = t.GetMethods().Select(m => Token(m)).ToArray(),
            layout = l.IsDefault ? null : new { packing = l.PackingSize, size = l.Size } };
    }),
    ["scopes"] = Rows(r.TypeReferences, h => h, h => { var t = r.GetTypeReference(h);
        return new { name = r.GetString(t.Name), @namespace = r.GetString(t.Namespace), scope = Token(t.ResolutionScope) }; }),
    ["fields"] = Rows(r.FieldDefinitions, h => h, h => { var f = r.GetFieldDefinition(h); var offset = f.GetOffset();
        return new { name = r.GetString(f.Name), flags = (int)f.Attributes, type = f.DecodeSignature(provider, 0), offset = offset < 0 ? (int?)null : offset }; }),
    ["methods"] = Rows(r.MethodDefinitions, h => h, h => { var m = r.GetMethodDefinition(h); var i = m.GetImport();
        return new { name = r.GetString(m.Name), flags = (int)m.Attributes, impl_flags = (int)m.ImplAttributes,
            signature = Types.Signature(m.DecodeSignature(provider, 0)), parameters = m.GetParameters().Select(p => Token(p)).ToArray(),
            import = i.Module.IsNil ? null : new { flags = (int)i.Attributes, name = r.GetString(i.Name), module = r.GetString(r.GetModuleReference(i.Module).Name) } }; }),
    ["parameters"] = Rows(Table(TableIndex.Param), h => h, h => { var p = r.GetParameter((ParameterHandle)h);
        return new { name = r.GetString(p.Name), sequence = p.SequenceNumber, flags = (int)p.Attributes }; }),
    ["generics"] = Rows(Table(TableIndex.GenericParam), h => h, h => { var p = r.GetGenericParameter((GenericParameterHandle)h);
        return new { name = r.GetString(p.Name), index = p.Index, flags = (int)p.Attributes, owner = Token(p.Parent) }; }),
    ["constants"] = Rows(Table(TableIndex.Constant), h => h, h => { var c = r.GetConstant((ConstantHandle)h);
        var row = MetadataTokens.GetRowNumber(c.Parent); var tag = c.Parent.Kind switch {
            HandleKind.FieldDefinition => 0, HandleKind.Parameter => 1, HandleKind.PropertyDefinition => 2, _ => throw new BadImageFormatException() };
        var b = r.GetBlobReader(c.Value);
        object value = c.TypeCode switch {
            ConstantTypeCode.Boolean => b.ReadBoolean(),
            ConstantTypeCode.Char => ((ushort)b.ReadChar()).ToString(CultureInfo.InvariantCulture),
            ConstantTypeCode.SByte => b.ReadSByte().ToString(CultureInfo.InvariantCulture),
            ConstantTypeCode.Byte => b.ReadByte().ToString(CultureInfo.InvariantCulture),
            ConstantTypeCode.Int16 => b.ReadInt16().ToString(CultureInfo.InvariantCulture),
            ConstantTypeCode.UInt16 => b.ReadUInt16().ToString(CultureInfo.InvariantCulture),
            ConstantTypeCode.Int32 => b.ReadInt32().ToString(CultureInfo.InvariantCulture),
            ConstantTypeCode.UInt32 => b.ReadUInt32().ToString(CultureInfo.InvariantCulture),
            ConstantTypeCode.Int64 => b.ReadInt64().ToString(CultureInfo.InvariantCulture),
            ConstantTypeCode.UInt64 => b.ReadUInt64().ToString(CultureInfo.InvariantCulture),
            ConstantTypeCode.Single => BitConverter.SingleToUInt32Bits(b.ReadSingle()).ToString("x8"),
            ConstantTypeCode.Double => BitConverter.DoubleToUInt64Bits(b.ReadDouble()).ToString("x16"),
            ConstantTypeCode.String => b.ReadUTF16(b.Length),
            ConstantTypeCode.NullReference => null,
            _ => throw new NotSupportedException($"constant {c.TypeCode}") };
        return new { parent_code = (row << 2) | tag, type = provider.GetPrimitiveType((PrimitiveTypeCode)c.TypeCode), value }; }),
    ["properties"] = Rows(Table(TableIndex.Property), h => h, h => { var p = r.GetPropertyDefinition((PropertyDefinitionHandle)h); var a = p.GetAccessors();
        var parent = propertyParents[(PropertyDefinitionHandle)h];
        return new { name = r.GetString(p.Name), flags = (int)p.Attributes, parent = Token(parent),
            signature = Types.Signature(p.DecodeSignature(provider, 0)), accessors = Accessors(new[] { (1, a.Setter), (2, a.Getter) }.Concat(a.Others.Select(m => (4, m)))) }; }),
    ["events"] = Rows(Table(TableIndex.Event), h => h, h => { var e = r.GetEventDefinition((EventDefinitionHandle)h); var a = e.GetAccessors();
        var parent = eventParents[(EventDefinitionHandle)h];
        return new { name = r.GetString(e.Name), flags = (int)e.Attributes, parent = Token(parent), type = provider.Entity(r, e.Type, 0),
            accessors = Accessors(new[] { (8, a.Adder), (16, a.Remover), (32, a.Raiser) }.Concat(a.Others.Select(m => (4, m)))) }; }),
    ["attributes_raw"] = Rows(r.CustomAttributes, h => h, h => { var a = r.GetCustomAttribute(h);
        int[] tables = [6, 4, 1, 2, 8, 9, 10, 0, 14, 23, 20, 17, 26, 27, 32, 35, 38, 39, 40, 42, 44, 43];
        var tag = Array.IndexOf(tables, Token(a.Parent) >> 24);
        if (tag < 0) throw new BadImageFormatException("attribute parent");
        return new { parent_code = (MetadataTokens.GetRowNumber(a.Parent) << 5) | tag,
            constructor_code = (MetadataTokens.GetRowNumber(a.Constructor) << 3) | (a.Constructor.Kind == HandleKind.MethodDefinition ? 2 : 3),
            blob = Convert.ToHexString(r.GetBlobBytes(a.Value)).ToLowerInvariant() }; }),
    ["attributes_decoded"] = Rows(r.CustomAttributes.Where(h => attributeProvider.Selected(r.GetCustomAttribute(h))), h => h, h => {
        var a = r.GetCustomAttribute(h); var name = attributeProvider.Name(a); var decoded = a.DecodeValue(attributeProvider);
        var arguments = decoded.FixedArguments.Select(v => new { name = "", value = AttributeTypes.Value(v.Type, v.Value) })
            .Concat(decoded.NamedArguments.Select(v => new { name = v.Name, value = AttributeTypes.Value(v.Type, v.Value) }))
            .OrderBy(v => v.name, StringComparer.Ordinal).ToArray();
        var split = name.LastIndexOf('.');
        return new { @namespace = name[..split], name = name[(split + 1)..], arguments };
    }),
    ["layouts"] = Rows(r.FieldDefinitions.Where(f => r.GetFieldDefinition(f).GetOffset() >= 0), h => h,
        h => new { offset = r.GetFieldDefinition(h).GetOffset() }),
    ["interfaces"] = Rows(Table(TableIndex.InterfaceImpl), h => h, h => { var i = r.GetInterfaceImplementation((InterfaceImplementationHandle)h);
        var owner = interfaceParents[(InterfaceImplementationHandle)h];
        var tag = i.Interface.Kind switch { HandleKind.TypeDefinition => 0, HandleKind.TypeReference => 1, HandleKind.TypeSpecification => 2, _ => throw new BadImageFormatException() };
        return new { owner = Token(owner), interface_code = (MetadataTokens.GetRowNumber(i.Interface) << 2) | tag }; }),
    ["nested"] = Rows(r.TypeDefinitions.Where(h => !r.GetTypeDefinition(h).GetDeclaringType().IsNil), h => h,
        h => new { parent = Token(r.GetTypeDefinition(h).GetDeclaringType()) }),
};
File.WriteAllText(args[1], JsonSerializer.Serialize(result));

sealed class Types(MetadataReader reader) : ISignatureTypeProvider<JsonNode, int>
{
    static JsonNode Node(object value) => JsonSerializer.SerializeToNode(value);
    public static object Signature(MethodSignature<JsonNode> s) => new {
        flags = s.Header.RawValue, generic_arity = s.GenericParameterCount, required_parameters = s.RequiredParameterCount,
        @return = s.ReturnType, parameters = s.ParameterTypes };
    public JsonNode GetPrimitiveType(PrimitiveTypeCode c) => Node(new { kind = c switch {
        PrimitiveTypeCode.Boolean => "bool", PrimitiveTypeCode.SByte => "i8", PrimitiveTypeCode.Byte => "u8",
        PrimitiveTypeCode.Int16 => "i16", PrimitiveTypeCode.UInt16 => "u16", PrimitiveTypeCode.Int32 => "i32",
        PrimitiveTypeCode.UInt32 => "u32", PrimitiveTypeCode.Int64 => "i64", PrimitiveTypeCode.UInt64 => "u64",
        PrimitiveTypeCode.Single => "f32", PrimitiveTypeCode.Double => "f64", PrimitiveTypeCode.IntPtr => "isize",
        PrimitiveTypeCode.UIntPtr => "usize", _ => c.ToString().ToLowerInvariant() } });
    public JsonNode GetTypeFromDefinition(MetadataReader r, TypeDefinitionHandle h, byte k) => Named(h, k);
    public JsonNode GetTypeFromReference(MetadataReader r, TypeReferenceHandle h, byte k) => Named(h, k);
    static JsonNode Named(EntityHandle h, byte k) => Node(new { kind = k == 0x11 ? "value" : "class", token = MetadataTokens.GetToken(h), arguments = Array.Empty<object>() });
    public JsonNode Entity(MetadataReader r, EntityHandle h, int c) => h.Kind == HandleKind.TypeSpecification
        ? GetTypeFromSpecification(r, c, (TypeSpecificationHandle)h, 0) : Named(h, 0x12);
    public JsonNode GetTypeFromSpecification(MetadataReader r, int c, TypeSpecificationHandle h, byte k) => r.GetTypeSpecification(h).DecodeSignature(this, c);
    public JsonNode GetGenericInstantiation(JsonNode t, ImmutableArray<JsonNode> args) {
        var copy = t.DeepClone(); copy["arguments"] = Node(args); return copy;
    }
    public JsonNode GetGenericTypeParameter(int c, int i) => Node(new { kind = "var", index = i });
    public JsonNode GetGenericMethodParameter(int c, int i) => Node(new { kind = "mvar", index = i });
    public JsonNode GetSZArrayType(JsonNode t) => Node(new { kind = "array", element = t });
    public JsonNode GetArrayType(JsonNode t, ArrayShape s) => Node(new { kind = "shaped_array", element = t,
        rank = s.Rank, sizes = s.Sizes, lower_bounds = s.LowerBounds.Concat(Enumerable.Repeat(0, s.Rank - s.LowerBounds.Length)) });
    public JsonNode GetPointerType(JsonNode t) => Node(new { kind = "pointer", element = t });
    public JsonNode GetByReferenceType(JsonNode t) => Node(new { kind = "byref", element = t });
    public JsonNode GetPinnedType(JsonNode t) => Node(new { kind = "pinned", element = t });
    public JsonNode GetModifiedType(JsonNode m, JsonNode t, bool required) {
        if (m["token"] is JsonNode token) {
            var h = MetadataTokens.EntityHandle(token.GetValue<int>());
            var name = h.Kind switch {
                HandleKind.TypeReference => reader.GetString(reader.GetTypeReference((TypeReferenceHandle)h).Namespace) + "." + reader.GetString(reader.GetTypeReference((TypeReferenceHandle)h).Name),
                HandleKind.TypeDefinition => reader.GetString(reader.GetTypeDefinition((TypeDefinitionHandle)h).Namespace) + "." + reader.GetString(reader.GetTypeDefinition((TypeDefinitionHandle)h).Name), _ => "" };
            if (name == "System.Runtime.CompilerServices.IsConst")
                return t["kind"]?.GetValue<string>() == "byref"
                    ? Node(new { kind = "const_byref", element = t["element"] })
                    : Node(new { kind = "const", element = t });
        }
        return Node(new { kind = "modified", modifier = m, element = t, required });
    }
    public JsonNode GetFunctionPointerType(MethodSignature<JsonNode> s) => Node(new { kind = "function_pointer", signature = Signature(s) });
}

sealed class AttributeTypes(MetadataReader reader) : ICustomAttributeTypeProvider<string>
{
    public string GetPrimitiveType(PrimitiveTypeCode c) => c.ToString();
    public string GetSystemType() => "System.Type";
    public bool IsSystemType(string t) => t == "System.Type";
    public string GetSZArrayType(string t) => t + "[]";
    public string GetTypeFromSerializedName(string name) => name;
    public string GetTypeFromDefinition(MetadataReader r, TypeDefinitionHandle h, byte k) {
        var t = r.GetTypeDefinition(h); return r.GetString(t.Namespace) + "." + r.GetString(t.Name);
    }
    public string GetTypeFromReference(MetadataReader r, TypeReferenceHandle h, byte k) {
        var t = r.GetTypeReference(h); return r.GetString(t.Namespace) + "." + r.GetString(t.Name);
    }
    public PrimitiveTypeCode GetUnderlyingEnumType(string name) {
        var local = reader.TypeDefinitions.Where(h => GetTypeFromDefinition(reader, h, 0) == name).ToArray();
        if (local.Length != 1) throw new NotSupportedException($"Cannot independently resolve enum underlying type: {name}");
        var field = reader.GetTypeDefinition(local[0]).GetFields().Single(h => reader.GetString(reader.GetFieldDefinition(h).Name) == "value__");
        var signature = reader.GetBlobReader(reader.GetFieldDefinition(field).Signature);
        signature.ReadSignatureHeader();
        return (PrimitiveTypeCode)signature.ReadSignatureTypeCode();
    }
    public string Name(CustomAttribute a) {
        EntityHandle parent = a.Constructor.Kind == HandleKind.MemberReference
            ? reader.GetMemberReference((MemberReferenceHandle)a.Constructor).Parent
            : reader.GetMethodDefinition((MethodDefinitionHandle)a.Constructor).GetDeclaringType();
        return parent.Kind switch {
            HandleKind.TypeDefinition => GetTypeFromDefinition(reader, (TypeDefinitionHandle)parent, 0),
            HandleKind.TypeReference => GetTypeFromReference(reader, (TypeReferenceHandle)parent, 0),
            _ => "" };
    }
    public bool Selected(CustomAttribute a) => Name(a) is
        "Winlex.Metadata.NativeAlignmentAttribute" or
        "Windows.Win32.Foundation.Metadata.GuidAttribute" or
        "Windows.Win32.Foundation.Metadata.SupportedArchitectureAttribute" or
        "Windows.Win32.Foundation.Metadata.NativeArrayInfoAttribute" or
        "Windows.Win32.Foundation.Metadata.MemorySizeAttribute" or
        "Windows.Win32.Foundation.Metadata.DocumentationAttribute" or
        "Windows.Win32.Foundation.Metadata.SupportedOSPlatformAttribute" or
        "Windows.Win32.Foundation.Metadata.FlexibleArrayAttribute" or
        "Windows.Foundation.Metadata.GuidAttribute" or
        "Windows.Foundation.Metadata.OverloadAttribute" or
        "Windows.Foundation.Metadata.DefaultAttribute";
    public static object Value(string type, object value) => value switch {
        null => null,
        bool b => b,
        float f => BitConverter.SingleToUInt32Bits(f).ToString("x8"),
        double d => BitConverter.DoubleToUInt64Bits(d).ToString("x16"),
        string s => s,
        IFormattable n => n.ToString(null, CultureInfo.InvariantCulture),
        _ => throw new NotSupportedException($"attribute argument {type}") };
}
