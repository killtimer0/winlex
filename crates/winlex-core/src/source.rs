use sha1::{Digest, Sha1};
use windows_metadata::reader::{Assembly, AssemblyRef};

#[cfg(test)]
mod snapshot_tests {
    use super::SourceData;
    use crate::test_support::{source as data, type_id as find};
    use std::sync::Arc;
    use windows_metadata::{FieldAttributes, Type, TypeAttributes, reader, writer};

    fn dependency(ty: Type) -> SourceData {
        let mut file = writer::File::new("Dependency");
        let base = file.TypeRef("System", "ValueType");
        file.TypeDef(
            "Fixture",
            "Widget",
            writer::TypeDefOrRef::TypeRef(base),
            TypeAttributes::Public
                | TypeAttributes::SequentialLayout
                | TypeAttributes::Sealed
                | TypeAttributes::WindowsRuntime,
        );
        file.Field("Value", &ty, FieldAttributes::Public);
        data("Dependency", file)
    }
    #[test]
    fn source_descriptions_keep_input_order_paths_and_assembly_names() {
        let first = data("甲", writer::File::new("First"));
        let mut second = data("乙", writer::File::new("Second"));
        second.path = format!("\\\\server\\share\\{}\\😀.winmd", "long".repeat(80)).into();
        let inputs = [first.clone(), second, first];
        let db = crate::Catalog::from_inputs(inputs.clone()).unwrap();
        for (id, (input, source)) in inputs.iter().zip(db.sources()).enumerate() {
            assert_eq!(source.id, id);
            assert_eq!(source.path, input.path);
            let assembly = source.assembly.as_ref().unwrap();
            assert_eq!(assembly.name, if id == 1 { "Second" } else { "First" });
        }
        let invalid = SourceData {
            path: "坏😀.winmd".into(),
            bytes: vec![0; 16].into(),
        };
        let error = crate::Catalog::from_inputs([inputs[0].clone(), invalid])
            .err()
            .unwrap();
        assert_eq!(error.to_string(), "invalid metadata: 坏😀.winmd");
        assert_eq!(
            crate::Catalog::from_inputs([]).err().unwrap().to_string(),
            "at least one WinMD is required"
        );
    }

    #[test]
    fn duplicate_inputs_keep_independent_catalog_local_ids() {
        let original = dependency(Type::I32);
        let db = crate::Catalog::from_inputs([original.clone(), original.clone()]).unwrap();
        assert_eq!(db.sources().len(), 2);
        let page = db
            .search(
                "Widget",
                crate::SearchOptions {
                    mode: crate::SearchMode::Name,
                    ..Default::default()
                },
            )
            .unwrap()
            .collect::<crate::Result<Vec<_>>>()
            .unwrap();
        assert_eq!(page.len(), 2);
        let [first, second] = [page[0].id, page[1].id];
        assert_eq!(first.source_id, 0);
        assert_eq!(second.source_id, 1);
        assert_eq!(first.token, second.token);
        for id in [first, second] {
            assert_eq!(db.entity(id).unwrap().name, "Widget");
        }
        assert!(
            db.declaration(
                crate::EntityId {
                    source_id: 2,
                    ..first
                },
                None
            )
            .is_err()
        );
        let current = crate::Catalog::from_inputs([dependency(Type::I64), original]).unwrap();
        assert_eq!(
            db.layout(first, crate::Architecture::X64)
                .unwrap()
                .size_bits,
            32
        );
        assert_eq!(
            current
                .layout(first, crate::Architecture::X64)
                .unwrap()
                .size_bits,
            64
        );
        assert_eq!(
            current
                .layout(second, crate::Architecture::X64)
                .unwrap()
                .size_bits,
            32
        );
    }
    #[test]
    fn dependency_replacement_does_not_repair_or_rebind_old_snapshots() {
        let original = dependency(Type::I32);
        let retained = Arc::downgrade(&original.bytes);
        let mut file = writer::File::new("Consumer");
        file.set_reference(reader::Index::new(vec![
            reader::File::new_shared(original.bytes.clone()).unwrap(),
        ]));
        let base = file.TypeRef("System", "ValueType");
        file.TypeDef(
            "Fixture",
            "Holder",
            writer::TypeDefOrRef::TypeRef(base),
            TypeAttributes::Public
                | TypeAttributes::SequentialLayout
                | TypeAttributes::Sealed
                | TypeAttributes::WindowsRuntime,
        );
        file.Field(
            "Member",
            &Type::value_named("Fixture", "Widget"),
            FieldAttributes::Public,
        );
        let consumer = data("Consumer", file);
        let duplicate =
            crate::Catalog::from_inputs([consumer.clone(), original.clone(), original.clone()])
                .unwrap();
        assert!(
            duplicate
                .layout(find(&duplicate, "Holder"), crate::Architecture::X64)
                .is_err()
        );
        drop(duplicate);
        let missing = Arc::new(crate::Catalog::from_inputs([consumer.clone()]).unwrap());
        assert!(
            missing
                .layout(find(&missing, "Holder"), crate::Architecture::X64)
                .is_err()
        );
        let old = Arc::new(crate::Catalog::from_inputs([consumer.clone(), original]).unwrap());
        assert_eq!(
            old.layout(find(&old, "Holder"), crate::Architecture::X64)
                .unwrap()
                .size_bits,
            32
        );
        let current =
            Arc::new(crate::Catalog::from_inputs([consumer, dependency(Type::I64)]).unwrap());
        assert_eq!(
            current
                .layout(find(&current, "Holder"), crate::Architecture::X64)
                .unwrap()
                .size_bits,
            64
        );
        assert!(
            missing
                .layout(find(&missing, "Holder"), crate::Architecture::X64)
                .is_err()
        );
        drop(current);
        assert_eq!(
            old.layout(find(&old, "Holder"), crate::Architecture::X86)
                .unwrap()
                .size_bits,
            32
        );
        assert!(retained.upgrade().is_some());
        drop(old);
        assert!(retained.upgrade().is_none());
    }
    #[test]
    fn snapshots_share_bytes_and_release_after_last_owner() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../inputs/Windows.Win32.winmd");
        let input = SourceData::read(path).unwrap();
        let weak = Arc::downgrade(&input.bytes);
        let first = crate::Catalog::from_inputs([input.clone()]).unwrap();
        let second = crate::Catalog::from_inputs(first.inputs().to_vec()).unwrap();
        assert!(Arc::ptr_eq(
            &first.inputs()[0].bytes,
            &second.inputs()[0].bytes
        ));
        drop(input);
        drop(first);
        assert!(weak.upgrade().is_some());
        assert!(
            second
                .search("CreateFileW", crate::SearchOptions::default())
                .is_ok()
        );
        drop(second);
        assert!(weak.upgrade().is_none());
    }
}

/// Immutable file version. Cloning shares bytes; reading again creates a new version.
#[derive(Clone)]
pub struct SourceData {
    pub(crate) path: std::path::PathBuf,
    pub(crate) bytes: std::sync::Arc<[u8]>,
}
impl SourceData {
    pub fn read(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let bytes: std::sync::Arc<[u8]> = std::fs::read(&path)?.into();
        Ok(Self { path, bytes })
    }
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
    #[allow(
        clippy::len_without_is_empty,
        reason = "Size is a file property; empty inputs are rejected as invalid metadata."
    )]
    pub fn len(&self) -> usize {
        self.bytes.len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct AssemblyIdentity {
    pub name: String,
    pub version: [u16; 4],
    pub culture: String,
    pub public_key_token: String,
    pub content_type: u32,
}

fn key_token(bytes: &[u8], full: bool) -> String {
    let bytes = if full && !bytes.is_empty() {
        Sha1::digest(bytes)[12..]
            .iter()
            .rev()
            .copied()
            .collect::<Vec<_>>()
    } else {
        bytes.to_vec()
    };
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl AssemblyIdentity {
    fn new(name: &str, version: [u16; 4], culture: &str, key: &[u8], flags: u32) -> Self {
        Self {
            name: name.into(),
            version,
            culture: culture.into(),
            public_key_token: key_token(key, flags & 1 != 0),
            content_type: flags & 0xe00,
        }
    }
    pub(crate) fn definition(a: Assembly<'_>) -> Self {
        Self::new(
            a.name(),
            a.version(),
            a.culture(),
            a.public_key(),
            a.flags(),
        )
    }
    pub(crate) fn reference(a: AssemblyRef<'_>) -> Self {
        Self::new(
            a.name(),
            a.version(),
            a.culture(),
            a.public_key_or_token(),
            a.flags(),
        )
    }
    pub(crate) fn matches(&self, other: &Self) -> bool {
        self.name.eq_ignore_ascii_case(&other.name)
            && self.version == other.version
            && self.culture.eq_ignore_ascii_case(&other.culture)
            && self.public_key_token == other.public_key_token
            && self.content_type == other.content_type
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Source {
    /// Index in the owning Catalog.
    pub id: usize,
    /// Original native path; display text must never be used as a file identity.
    #[serde(serialize_with = "display_path")]
    pub path: std::path::PathBuf,
    pub assembly: Option<AssemblyIdentity>,
    pub references: Vec<AssemblyIdentity>,
}

fn display_path<S: serde::Serializer>(
    path: &std::path::Path,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&path.to_string_lossy())
}

/// A metadata record in one particular Catalog. Not transferable between catalogs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub struct EntityId {
    pub source_id: usize,
    pub token: u32,
}
impl std::fmt::Display for EntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}::0x{:08x}", self.source_id, self.token)
    }
}
