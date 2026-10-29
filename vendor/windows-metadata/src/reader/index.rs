use super::*;

/// Indexes metadata types.
pub struct Index {
    files: Vec<File>,
    types: HashMap<String, HashMap<String, Vec<(usize, usize)>>>,
    nested: HashMap<(usize, usize), Vec<usize>>,
    architecture: i32,
}

impl Index {
    /// Enumerate raw rows, without namespace or architecture filtering.
    pub fn rows<'a, R: AsRow<'a> + 'a>(
        &'a self,
        file: usize,
    ) -> impl ExactSizeIterator<Item = R> + 'a {
        RowIterator::new(self, file, 0..self.files(file).row_count::<R>())
    }
    /// Reads a single metadata file from disk and builds an `Index` for it.
    #[must_use]
    pub fn read<P: AsRef<std::path::Path>>(path: P) -> Option<Self> {
        Some(Self::new(vec![File::read(path)?]))
    }

    /// Builds an `Index` over the given metadata files.
    #[must_use]
    pub fn new(files: Vec<File>) -> Self {
        Self::new_for_architecture(files, 0)
    }

    /// Builds an `Index` filtered to one architecture bit (1=X86, 2=X64, 4=Arm64). A zero bit
    /// preserves all architecture-specific rows.
    #[must_use]
    pub fn new_for_architecture(files: Vec<File>, architecture: i32) -> Self {
        let mut types: HashMap<String, HashMap<String, Vec<(usize, usize)>>> = HashMap::new();
        let mut nested: HashMap<(usize, usize), Vec<usize>> = HashMap::new();

        for (file_pos, file) in files.iter().enumerate() {
            for def_pos in file.TypeDef() {
                let namespace = file.str(def_pos, TypeDef::TABLE, 2);

                if namespace.is_empty() {
                    continue;
                }

                let name = file.str(def_pos, TypeDef::TABLE, 1);

                types
                    .entry(namespace.to_string())
                    .or_default()
                    .entry(trim_tick(name).to_string())
                    .or_default()
                    .push((file_pos, def_pos));
            }

            for map in file.NestedClass() {
                let inner = file.usize(map, NestedClass::TABLE, 0) - 1;
                let outer = file.usize(map, NestedClass::TABLE, 1) - 1;
                nested.entry((file_pos, outer)).or_default().push(inner);
            }
        }

        Self {
            files,
            types,
            nested,
            architecture,
        }
    }

    /// Leaks `self` for consumers that need `'static` metadata rows.
    #[must_use]
    pub fn leak(self) -> &'static Self {
        Box::leak(Box::new(self))
    }

    /// Reads one metadata file and leaks the resulting `Index`.
    #[must_use]
    pub fn read_static<P: AsRef<std::path::Path>>(path: P) -> Option<&'static Self> {
        Some(Self::new(vec![File::read(path)?]).leak())
    }

    pub(crate) fn files(&self, pos: usize) -> &File {
        &self.files[pos]
    }

    /// Iterates `(namespace, name, TypeDef)` triples over every type in the index.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str, TypeDef<'_>)> + '_ {
        self.types
            .iter()
            .flat_map(|(namespace, types)| {
                types
                    .iter()
                    .map(move |(name, types)| (namespace.as_str(), name.as_str(), types))
            })
            .flat_map(|(namespace, name, types)| types.iter().map(move |ty| (namespace, name, ty)))
            .map(|(namespace, name, (file, pos))| {
                (namespace, name, TypeDef(Row::new(self, *file, *pos)))
            })
            .filter(|(_, _, def)| self.supports_architecture(*def))
    }

    /// Iterates every `TypeDef` in the index.
    pub fn types(&self) -> impl Iterator<Item = TypeDef<'_>> + '_ {
        self.types
            .values()
            .flat_map(|types| types.values())
            .flatten()
            .map(|(file, pos)| TypeDef(Row::new(self, *file, *pos)))
            .filter(|def| self.supports_architecture(*def))
    }

    /// Iterates the `TypeDef`s matching `(namespace, name)`.
    pub fn get(&self, namespace: &str, name: &str) -> impl Iterator<Item = TypeDef<'_>> + '_ {
        self.types
            .get(namespace)
            .and_then(|types| types.get(name))
            .into_iter()
            .flatten()
            .map(|(file, pos)| TypeDef(Row::new(self, *file, *pos)))
            .filter(|def| self.supports_architecture(*def))
    }

    /// Returns whether any type lives in `namespace`.
    pub fn contains_namespace(&self, namespace: &str) -> bool {
        self.types.get(namespace).is_some_and(|types| {
            types
                .values()
                .flatten()
                .any(|(file, pos)| self.supports_architecture(TypeDef(Row::new(self, *file, *pos))))
        })
    }

    /// Returns whether the `(namespace, name)` type exists.
    pub fn contains(&self, namespace: &str, name: &str) -> bool {
        self.get(namespace, name).next().is_some()
    }

    /// Returns the assembly name of the first file in which `(namespace, name)` was defined.
    pub fn assembly_name(&self, namespace: &str, name: &str) -> Option<&str> {
        self.types
            .get(namespace)
            .and_then(|types| types.get(name))
            .and_then(|types| {
                types.iter().find(|(file, pos)| {
                    self.supports_architecture(TypeDef(Row::new(self, *file, *pos)))
                })
            })
            .map(|(file, _)| self.files(*file))
            .and_then(|file| file.assembly_name())
    }

    /// Returns the single `TypeDef` matching `(namespace, name)`, panicking if there are zero
    /// or more than one.
    #[track_caller]
    pub fn expect(&self, namespace: &str, name: &str) -> TypeDef<'_> {
        let mut iter = self.get(namespace, name);

        if let Some(def) = iter.next() {
            if iter.next().is_none() {
                def
            } else {
                panic!("more than one type found: {namespace}.{name}");
            }
        } else {
            panic!("type not found: {namespace}.{name}")
        }
    }

    /// Iterates the types directly nested inside `ty`. Use [`Self::nested_recursive`] to recurse.
    pub fn nested(&self, ty: TypeDef) -> impl Iterator<Item = TypeDef<'_>> + '_ {
        self.nested
            .get(&(ty.0.file, ty.0.pos))
            .into_iter()
            .flatten()
            .copied()
            .map(move |pos| {
                TypeDef(Row {
                    index: self,
                    file: ty.0.file,
                    pos,
                })
            })
            .filter(|def| self.supports_architecture(*def))
    }

    /// Depth-first walk of every type nested directly or transitively inside `ty`.
    pub fn nested_recursive<'a>(&'a self, ty: TypeDef<'a>) -> Vec<TypeDef<'a>> {
        let mut out = Vec::new();
        self.collect_nested(ty, &mut out);
        out
    }

    fn collect_nested<'a>(&'a self, ty: TypeDef<'a>, out: &mut Vec<TypeDef<'a>>) {
        for inner in self.nested(ty) {
            out.push(inner);
            self.collect_nested(inner, out);
        }
    }

    fn supports_architecture<'a, R: HasAttributes<'a>>(&self, row: R) -> bool {
        let arches = row.arches();
        self.architecture == 0 || arches == 0 || arches & self.architecture != 0
    }
}
