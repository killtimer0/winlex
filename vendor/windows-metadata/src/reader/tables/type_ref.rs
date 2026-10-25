use super::*;

impl std::fmt::Debug for TypeRef<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "TypeRef({}.{})", self.namespace(), self.name())
    }
}

impl<'a> TypeRef<'a> {
    /// Raw ResolutionScope as an ECMA token (zero denotes a null scope).
    pub fn scope_token(&self) -> u32 {
        let code = self.usize(0) as u32;
        if code == 0 { return 0; }
        let table = [0x00, 0x1a, 0x23, 0x01][(code & 3) as usize];
        (table << 24) | (code >> 2)
    }
    pub fn scope(&self) -> ResolutionScope<'a> {
        self.decode(0)
    }

    pub fn name(&self) -> &'a str {
        self.str(1)
    }

    pub fn namespace(&self) -> &'a str {
        self.str(2)
    }
}
