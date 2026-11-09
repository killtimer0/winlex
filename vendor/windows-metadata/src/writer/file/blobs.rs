use super::*;

pub struct Blobs {
    map: HashMap<Vec<u8>, BlobId>,
    stream: Vec<u8>,
}

impl Default for Blobs {
    fn default() -> Self {
        Self {
            map: Default::default(),
            stream: vec![0], // the empty blob
        }
    }
}

impl Blobs {
    pub fn insert(&mut self, value: &[u8]) -> BlobId {
        if value.is_empty() {
            return BlobId(0);
        }

        if let Some(pos) = self.map.get(value) {
            return *pos;
        }

        let pos = BlobId(self.stream.len().try_into().unwrap());
        self.map.insert(value.to_vec(), pos);
        self.stream.write_compressed(value.len());

        self.stream.extend_from_slice(value);
        pos
    }

    pub fn into_stream(self) -> Vec<u8> {
        self.stream.into_stream()
    }
}
