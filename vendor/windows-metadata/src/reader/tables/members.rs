use super::*;

impl<'a> TypeDef<'a> {
    pub fn properties(&self) -> impl Iterator<Item = Property<'a>> {
        self.equal_range::<PropertyMap>(0, self.pos() + 1).flat_map(|r| r.list(1))
    }
    pub fn events(&self) -> impl Iterator<Item = Event<'a>> {
        self.equal_range::<EventMap>(0, self.pos() + 1).flat_map(|r| r.list(1))
    }
}

impl<'a> Property<'a> {
    pub fn name(&self) -> &'a str { self.str(1) }
    pub fn flags(&self) -> u16 { self.usize(0) as u16 }
    pub fn parent(&self) -> TypeDef<'a> {
        self.parent_row::<PropertyMap>(1).row(0)
    }
    pub fn signature(&self) -> Signature {
        self.blob(2).read_method_signature(&self.parent().generic_types())
    }
    pub fn accessors(&self) -> RowIterator<'a, MethodSemantics<'a>> {
        self.equal_range(2, (self.pos() + 1) << 1 | 1)
    }
}

impl<'a> Event<'a> {
    pub fn name(&self) -> &'a str { self.str(1) }
    pub fn flags(&self) -> u16 { self.usize(0) as u16 }
    pub fn parent(&self) -> TypeDef<'a> {
        self.parent_row::<EventMap>(1).row(0)
    }
    pub fn ty(&self) -> Type {
        match self.decode::<TypeDefOrRef>(2) {
            TypeDefOrRef::TypeSpec(t) => t.ty(&self.parent().generic_types()),
            t => Type::ClassName(t.type_name()),
        }
    }
    pub fn accessors(&self) -> RowIterator<'a, MethodSemantics<'a>> {
        self.equal_range(2, (self.pos() + 1) << 1)
    }
}

impl<'a> MethodSemantics<'a> {
    pub fn flags(&self) -> u16 { self.usize(0) as u16 }
    pub fn method(&self) -> MethodDef<'a> { self.row(1) }
}

impl std::fmt::Debug for Property<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.debug_tuple("Property").field(&self.name()).finish() }
}
impl std::fmt::Debug for Event<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.debug_tuple("Event").field(&self.name()).finish() }
}
