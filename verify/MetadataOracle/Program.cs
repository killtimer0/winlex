// Independent, stock .NET reader: checks row counts and index keys, not product data.
using System.Reflection.Metadata;
using System.Reflection.Metadata.Ecma335;
using System.Reflection.PortableExecutable;
using System.Text.Json;
using System.Globalization;
using System.Collections.Immutable;

using var stream = File.OpenRead(args[0]);
using var pe = new PEReader(stream);
var r = pe.GetMetadataReader();
var names = new SortedDictionary<string, List<uint>>(StringComparer.Ordinal);
var values = new SortedDictionary<string, List<uint>>(StringComparer.Ordinal);
var guids = new SortedDictionary<string, List<uint>>(StringComparer.Ordinal);
void Add(SortedDictionary<string,List<uint>> map, string key, EntityHandle h) {
    if (!map.TryGetValue(key, out var ids)) map[key] = ids = new();
    ids.Add((uint)MetadataTokens.GetToken(h));
}
string AttributeName(CustomAttribute a) {
    if (a.Constructor.Kind != HandleKind.MemberReference) return "";
    var parent = r.GetMemberReference((MemberReferenceHandle)a.Constructor).Parent;
    return parent.Kind switch {
        HandleKind.TypeReference => r.GetString(r.GetTypeReference((TypeReferenceHandle)parent).Name),
        HandleKind.TypeDefinition => r.GetString(r.GetTypeDefinition((TypeDefinitionHandle)parent).Name),
        _ => ""
    };
}
void GuidKeys(EntityHandle h, CustomAttributeHandleCollection attrs) {
    foreach (var handle in attrs) {
        var a=r.GetCustomAttribute(handle);
        if (AttributeName(a)!="GuidAttribute") continue;
        var blob=r.GetBlobReader(a.Value);
        if (blob.ReadUInt16()!=1) throw new Exception("attribute prolog");
        var d1=blob.ReadUInt32();var d2=blob.ReadUInt16();var d3=blob.ReadUInt16();
        var tail=Enumerable.Range(0,8).Select(_=>blob.ReadByte()).ToArray();
        Add(guids,$"{d1:x8}-{d2:x4}-{d3:x4}-{tail[0]:x2}{tail[1]:x2}-{string.Concat(tail.Skip(2).Select(v=>v.ToString("x2")))}",h);
    }
}
int constants=0;
foreach (var h in r.TypeDefinitions) {
    var t=r.GetTypeDefinition(h);
    Add(names,r.GetString(t.Name).ToLowerInvariant(),h);GuidKeys(h,t.GetCustomAttributes());
}
foreach (var h in r.FieldDefinitions) {
    var f=r.GetFieldDefinition(h);
    Add(names,r.GetString(f.Name).ToLowerInvariant(),h);GuidKeys(h,f.GetCustomAttributes());
    var ch=f.GetDefaultValue();if(ch.IsNil)continue;
    constants++;
    var c=r.GetConstant(ch);var b=r.GetBlobReader(c.Value);
    object v=c.TypeCode switch {
        ConstantTypeCode.SByte=>b.ReadSByte(),ConstantTypeCode.Byte=>b.ReadByte(),
        ConstantTypeCode.Int16=>b.ReadInt16(),ConstantTypeCode.UInt16=>b.ReadUInt16(),
        ConstantTypeCode.Int32=>b.ReadInt32(),ConstantTypeCode.UInt32=>b.ReadUInt32(),
        ConstantTypeCode.Int64=>b.ReadInt64(),ConstantTypeCode.UInt64=>b.ReadUInt64(),
        _=>null
    };
    if(v!=null)Add(values,Convert.ToString(v,CultureInfo.InvariantCulture)!,h);
}
foreach(var h in r.MethodDefinitions)Add(names,r.GetString(r.GetMethodDefinition(h).Name).ToLowerInvariant(),h);
foreach(var map in new[]{names,values,guids})foreach(var ids in map.Values)ids.Sort();
var sample=r.TypeDefinitions.Where(h=>r.GetString(r.GetTypeDefinition(h).Name)=="OPENFILENAMEW").Select(h=>{
 var t=r.GetTypeDefinition(h);var l=t.GetLayout();
 return new{token=MetadataTokens.GetToken(h),pack=l.PackingSize,size=l.Size,fields=t.GetFields().Select(f=>new{token=MetadataTokens.GetToken(f),name=r.GetString(r.GetFieldDefinition(f).Name),offset=r.GetFieldDefinition(f).GetOffset()}).ToArray()};
}).ToArray();
var layouts=r.FieldDefinitions.Where(h=>r.GetFieldDefinition(h).GetOffset()>=0).Select(h=>new{field=MetadataTokens.GetToken(h),offset=r.GetFieldDefinition(h).GetOffset()}).ToArray();
var scopes=r.TypeReferences.Select(h=>{var t=r.GetTypeReference(h);return new{token=MetadataTokens.GetToken(h),scope=t.ResolutionScope.IsNil?0:MetadataTokens.GetToken(t.ResolutionScope),name=r.GetString(t.Name),@namespace=r.GetString(t.Namespace)};}).ToArray();
var signatures=new Dictionary<string,int[]>();
var provider=new Refs();
foreach(var h in r.FieldDefinitions)signatures[MetadataTokens.GetToken(h).ToString()]=r.GetFieldDefinition(h).DecodeSignature(provider,0).ToArray();
foreach(var h in r.MethodDefinitions){var s=r.GetMethodDefinition(h).DecodeSignature(provider,0);signatures[MetadataTokens.GetToken(h).ToString()]=s.ReturnType.Concat(s.ParameterTypes.SelectMany(p=>p)).ToArray();}
File.WriteAllText(args[1],JsonSerializer.Serialize(new{type_rows=r.TypeDefinitions.Count,field_rows=r.FieldDefinitions.Count,method_rows=r.MethodDefinitions.Count,constants,field_layout_rows=r.GetTableRowCount(TableIndex.FieldLayout),names,values,guids,sample,layouts,scopes,signature_type_tokens=signatures}));
Console.WriteLine(JsonSerializer.Serialize(new{types=r.TypeDefinitions.Count,fields=r.FieldDefinitions.Count,methods=r.MethodDefinitions.Count,constants,field_layout_rows=r.GetTableRowCount(TableIndex.FieldLayout)}));

sealed class Refs : ISignatureTypeProvider<ImmutableArray<int>,int> {
 public ImmutableArray<int> GetTypeFromDefinition(MetadataReader r,TypeDefinitionHandle h,byte k)=>[MetadataTokens.GetToken(h)];
 public ImmutableArray<int> GetTypeFromReference(MetadataReader r,TypeReferenceHandle h,byte k)=>[MetadataTokens.GetToken(h)];
 public ImmutableArray<int> GetTypeFromSpecification(MetadataReader r,int c,TypeSpecificationHandle h,byte k)=>[MetadataTokens.GetToken(h)];
 public ImmutableArray<int> GetPrimitiveType(PrimitiveTypeCode c)=>[];
 public ImmutableArray<int> GetArrayType(ImmutableArray<int> t,ArrayShape s)=>t;
 public ImmutableArray<int> GetSZArrayType(ImmutableArray<int> t)=>t;
 public ImmutableArray<int> GetPointerType(ImmutableArray<int> t)=>t;
 public ImmutableArray<int> GetByReferenceType(ImmutableArray<int> t)=>t;
 public ImmutableArray<int> GetPinnedType(ImmutableArray<int> t)=>t;
 public ImmutableArray<int> GetModifiedType(ImmutableArray<int> m,ImmutableArray<int> t,bool required)=>t;
 public ImmutableArray<int> GetGenericInstantiation(ImmutableArray<int> t,ImmutableArray<ImmutableArray<int>> a)=>t.AddRange(a.SelectMany(v=>v));
 public ImmutableArray<int> GetGenericMethodParameter(int c,int i)=>[];
 public ImmutableArray<int> GetGenericTypeParameter(int c,int i)=>[];
 public ImmutableArray<int> GetFunctionPointerType(MethodSignature<ImmutableArray<int>> s)=>s.ReturnType.AddRange(s.ParameterTypes.SelectMany(v=>v));
}
