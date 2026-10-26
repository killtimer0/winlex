use super::*;

impl Assembly<'_> {
    pub fn name(&self) -> &str { self.str(4) }
    pub fn culture(&self) -> &str { self.str(5) }
    pub fn version(&self) -> [u16; 4] { self.file().version(self.pos(), Self::TABLE, 1) }
    pub fn flags(&self) -> u32 { self.usize(2) as u32 }
    pub fn public_key(&self) -> &[u8] { self.file().blob(self.pos(), Self::TABLE, 3) }
}

impl AssemblyRef<'_> {
    pub fn name(&self) -> &str { self.str(3) }
    pub fn culture(&self) -> &str { self.str(4) }
    pub fn version(&self) -> [u16; 4] { self.file().version(self.pos(), Self::TABLE, 0) }
    pub fn flags(&self) -> u32 { self.usize(1) as u32 }
    pub fn public_key_or_token(&self) -> &[u8] { self.file().blob(self.pos(), Self::TABLE, 2) }
}

impl std::fmt::Debug for AssemblyRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_tuple("AssemblyRef").field(&self.0).finish()
    }
}
