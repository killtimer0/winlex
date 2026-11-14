using System.Reflection;
using System.Reflection.Metadata;
using System.Reflection.PortableExecutable;
using System.Security.Cryptography;

// Readers stay alive until all custom attribute blobs have been decoded.
sealed class MetadataDependencies(MetadataReader root, string input, IEnumerable<string> explicitPaths) : IDisposable
{
    readonly Dictionary<string, (PEReader Pe, MetadataReader Reader)> opened = new(StringComparer.OrdinalIgnoreCase);
    readonly string[] paths = explicitPaths.Select(Path.GetFullPath).Concat(
        Directory.EnumerateFiles(Path.GetDirectoryName(Path.GetFullPath(input)))
            .Where(p => Path.GetExtension(p).Equals(".winmd", StringComparison.OrdinalIgnoreCase)
                || Path.GetExtension(p).Equals(".dll", StringComparison.OrdinalIgnoreCase)))
        .Append(Path.GetFullPath(input)).Distinct(StringComparer.OrdinalIgnoreCase).ToArray();

    static string Identity(MetadataReader r, StringHandle name, Version version, StringHandle culture,
        BlobHandle key, AssemblyFlags flags)
    {
        var bytes = r.GetBlobBytes(key);
        if ((flags & AssemblyFlags.PublicKey) != 0 && bytes.Length > 0)
            bytes = SHA1.HashData(bytes).TakeLast(8).Reverse().ToArray();
        return $"{r.GetString(name).ToLowerInvariant()}, Version={version}, Culture={r.GetString(culture).ToLowerInvariant()}, PublicKeyToken={Convert.ToHexString(bytes)}, ContentType={(int)flags & 0xe00}";
    }

    MetadataReader Assembly(string identity)
    {
        var matches = new List<MetadataReader>();
        foreach (var path in paths)
        {
            if (!opened.TryGetValue(path, out var item))
            {
                var pe = new PEReader(File.OpenRead(path));
                try { item = (pe, pe.GetMetadataReader(MetadataReaderOptions.None)); }
                catch { pe.Dispose(); throw; }
                opened.Add(path, item);
            }
            var r = item.Reader;
            if (!r.IsAssembly) continue;
            var a = r.GetAssemblyDefinition();
            if (Identity(r, a.Name, a.Version, a.Culture, a.PublicKey, a.Flags) == identity)
                matches.Add(r);
        }
        return matches.Count switch {
            1 => matches[0],
            0 => throw new InvalidOperationException($"Missing metadata assembly identity: {identity}"),
            _ => throw new InvalidOperationException($"Ambiguous metadata assembly identity: {identity}") };
    }

    static (MetadataReader, TypeDefinitionHandle) Find(MetadataReader r, string ns, string name,
        TypeDefinitionHandle parent = default)
    {
        var matches = r.TypeDefinitions.Where(h => {
            var t = r.GetTypeDefinition(h);
            return r.GetString(t.Name) == name && r.GetString(t.Namespace) == ns && t.GetDeclaringType() == parent;
        }).ToArray();
        if (matches.Length != 1)
            throw new InvalidOperationException($"Expected one metadata type definition: {ns}.{name}; found {matches.Length}");
        return (r, matches[0]);
    }

    public (MetadataReader Reader, TypeDefinitionHandle Handle) Resolve(AttributeType type)
    {
        if (type.Reader == null) type = SerializedType(type.Name);
        var r = type.Reader;
        if (type.Handle.Kind == HandleKind.TypeDefinition) return (r, (TypeDefinitionHandle)type.Handle);
        var t = r.GetTypeReference((TypeReferenceHandle)type.Handle);
        var ns = r.GetString(t.Namespace);
        var name = r.GetString(t.Name);
        switch (t.ResolutionScope.Kind)
        {
            case HandleKind.AssemblyReference:
                var a = r.GetAssemblyReference((AssemblyReferenceHandle)t.ResolutionScope);
                return Find(Assembly(Identity(r, a.Name, a.Version, a.Culture, a.PublicKeyOrToken, a.Flags)), ns, name);
            case HandleKind.ModuleDefinition:
                return Find(r, ns, name);
            case HandleKind.TypeReference:
                var (owner, parent) = Resolve(new AttributeType("", r, t.ResolutionScope));
                return Find(owner, ns, name, parent);
            default:
                throw new NotSupportedException($"Unsupported enum resolution scope: {t.ResolutionScope.Kind} ({type.Name})");
        }
    }

    AttributeType SerializedType(string name)
    {
        // Serialized enum names carry assembly identity, unlike primitive type names.
        var split = name.IndexOf(',');
        var fullName = split < 0 ? name : name[..split].Trim();
        var r = root;
        if (split >= 0)
        {
            var a = new AssemblyName(name[(split + 1)..].Trim());
            if (a.Version == null) throw new NotSupportedException($"Incomplete serialized assembly identity: {name}");
            r = Assembly($"{a.Name.ToLowerInvariant()}, Version={a.Version}, Culture={(a.CultureName ?? "").ToLowerInvariant()}, PublicKeyToken={Convert.ToHexString(a.GetPublicKeyToken() ?? [])}, ContentType={(int)a.ContentType << 9}");
        }
        var dot = fullName.LastIndexOf('.');
        var (_, h) = Find(r, dot < 0 ? "" : fullName[..dot], fullName[(dot + 1)..]);
        return new(fullName, r, h);
    }

    public void Dispose()
    {
        foreach (var item in opened.Values) item.Pe.Dispose();
    }
}
