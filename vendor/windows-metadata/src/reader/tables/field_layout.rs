use super::*;

impl<'a> FieldLayout<'a> {
    /// Explicit byte offset from the containing type, as encoded in ECMA-335.
    pub fn offset(&self) -> u32 { self.usize(0).try_into().unwrap() }
    pub fn field(&self) -> Field<'a> { self.row(1) }
}
