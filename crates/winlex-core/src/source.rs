use serde::Serialize;
use sha1::{Digest, Sha1};
use windows_metadata::reader::{Assembly, AssemblyRef};

#[cfg(test)]
mod snapshot_tests {
    use super::SourceData;
    use sha2::Digest;
    use std::sync::Arc;
    use windows_metadata::{FieldAttributes, Type, TypeAttributes, reader, writer};

    fn data(name: &str, file: writer::File) -> SourceData {
        let bytes: Arc<[u8]> = file.into_stream().into();
        SourceData {
            path: format!("{name}.winmd").into(),
            id: format!("{:x}", sha2::Sha256::digest(&bytes)),
            bytes,
        }
    }
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
        let missing = Arc::new(crate::Catalog::from_inputs([consumer.clone()]).unwrap());
        assert!(missing.layout("Holder", crate::Architecture::X64).is_err());
        let old = Arc::new(crate::Catalog::from_inputs([consumer.clone(), original]).unwrap());
        assert_eq!(
            old.layout("Holder", crate::Architecture::X64).unwrap()["size_bits"],
            32
        );
        let current =
            Arc::new(crate::Catalog::from_inputs([consumer, dependency(Type::I64)]).unwrap());
        assert_eq!(
            current.layout("Holder", crate::Architecture::X64).unwrap()["size_bits"],
            64
        );
        assert!(missing.layout("Holder", crate::Architecture::X64).is_err());
        drop(current);
        assert_eq!(
            old.layout("Holder", crate::Architecture::X86).unwrap()["size_bits"],
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
                .describe("CreateFileW", Some(crate::Architecture::X64))
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
    pub(crate) id: String,
    pub(crate) bytes: std::sync::Arc<[u8]>,
}
impl SourceData {
    pub fn read(path: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let bytes: std::sync::Arc<[u8]> = std::fs::read(&path)?.into();
        let id = format!("{:x}", sha2::Sha256::digest(&bytes));
        Ok(Self { path, bytes, id })
    }
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
    pub fn len(&self) -> usize {
        self.bytes.len()
    }
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }
    /// Associate already verified identical bytes with another native path.
    pub fn with_path(&self, path: std::path::PathBuf) -> Self {
        Self {
            path,
            ..self.clone()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
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
    pub(crate) fn definition(a: Assembly<'_>) -> Self {
        Self {
            name: a.name().into(),
            version: a.version(),
            culture: a.culture().into(),
            public_key_token: key_token(a.public_key(), a.flags() & 1 != 0),
            content_type: a.flags() & 0xe00,
        }
    }
    pub(crate) fn reference(a: AssemblyRef<'_>) -> Self {
        Self {
            name: a.name().into(),
            version: a.version(),
            culture: a.culture().into(),
            public_key_token: key_token(a.public_key_or_token(), a.flags() & 1 != 0),
            content_type: a.flags() & 0xe00,
        }
    }
    pub(crate) fn matches(&self, other: &Self) -> bool {
        self.name.eq_ignore_ascii_case(&other.name)
            && self.version == other.version
            && self.culture.eq_ignore_ascii_case(&other.culture)
            && self.public_key_token == other.public_key_token
            && self.content_type == other.content_type
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Source {
    /// SHA256 of the original file; also the stable source selector.
    pub id: String,
    pub path: String,
    /// Original native path; display text must never be used as a file identity.
    #[serde(skip)]
    pub native_path: std::path::PathBuf,
    pub assembly: Option<AssemblyIdentity>,
    pub references: Vec<AssemblyIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    pub file: usize,
    pub token: u32,
}
