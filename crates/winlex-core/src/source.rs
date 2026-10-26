use serde::Serialize;
use sha1::{Digest, Sha1};
use windows_metadata::reader::{Assembly, AssemblyRef};

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
    pub assembly: Option<AssemblyIdentity>,
    pub references: Vec<AssemblyIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    pub file: usize,
    pub token: u32,
}
