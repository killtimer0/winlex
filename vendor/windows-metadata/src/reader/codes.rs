use super::*;

pub trait Decode<'a> {
    fn decode(index: &'a Index, file: usize, code: usize) -> Self;
}

impl TypeDefOrRef<'_> {
    pub fn identity(&self) -> (usize, u32) {
        match self {
            Self::TypeDef(r) => (r.to_row().file, 0x02000000 | (r.pos() + 1) as u32),
            Self::TypeRef(r) => (r.to_row().file, 0x01000000 | (r.pos() + 1) as u32),
            Self::TypeSpec(r) => (r.to_row().file, 0x1b000000 | (r.pos() + 1) as u32),
        }
    }
    pub fn type_name(&self) -> TypeName {
        let mut n = TypeName::named(self.namespace(), self.name());
        n.identity = Some(self.identity());
        n
    }
}

macro_rules! code {
    ($name:ident($size:literal) $(($table:ident, $code:literal))+) => {
        #[derive(Clone, Debug, Hash, PartialEq, Eq, Ord, PartialOrd)]
        pub enum $name<'a> {
            $($table($table<'a>),)*
        }
        impl<'a> Decode<'a> for $name<'a> {
            fn decode(index: &'a Index, file: usize, code: usize) -> Self {
                let (kind, row) = (code & ((1 << $size) - 1), (code >> $size) - 1);
                match kind {
                    $($code => Self::$table($table(Row::new(index, file, row))),)*
                    rest => panic!("{rest:?}"),
                }
            }
        }
        impl $name<'_> {
            pub fn encode(&self) -> usize {
                match self {
                    $(Self::$table(row) => (row.pos() + 1) << $size | $code,)*
                }
            }
        }
        $(
            impl<'a> From<$table<'a>> for $name<'a> {
                fn from(from: $table<'a>) -> Self {
                    Self::$table(from)
                }
            }
        )*
    };
}

code! { AttributeType(3)
    (MethodDef, 2)
    (MemberRef, 3)
}

impl<'a> AttributeType<'a> {
    pub fn parent(&self) -> MemberRefParent<'a> {
        match self {
            Self::MethodDef(row) => row.parent(),
            Self::MemberRef(row) => row.parent(),
        }
    }

    pub fn signature(&self, generics: &[Type]) -> Signature {
        match self {
            Self::MethodDef(row) => row.signature(generics),
            Self::MemberRef(row) => row.signature(generics),
        }
    }
}

code! { HasAttribute(5)
    (MethodDef, 0)
    (Field, 1)
    (TypeRef, 2)
    (TypeDef, 3)
    (MethodParam, 4)
    (InterfaceImpl, 5)
    (MemberRef, 6)
    (Property, 9)
    (Event, 10)
    (TypeSpec, 13)
    (GenericParam, 19)
}

code! { HasConstant(2)
    (Field, 0)
}

code! { MemberForwarded(1)
    (MethodDef, 1)
}

code! { MemberRefParent(3)
    (TypeDef, 0)
    (TypeRef, 1)
}

impl<'a> MemberRefParent<'a> {
    pub fn namespace(&self) -> &'a str {
        match self {
            Self::TypeDef(row) => row.namespace(),
            Self::TypeRef(row) => row.namespace(),
        }
    }

    pub fn name(&self) -> &'a str {
        match self {
            Self::TypeDef(row) => row.name(),
            Self::TypeRef(row) => row.name(),
        }
    }
}

code! { TypeDefOrRef(2)
    (TypeDef, 0)
    (TypeRef, 1)
    (TypeSpec, 2)
}

code! { TypeOrMethodDef(1)
    (TypeDef, 0)
    (MethodDef, 1)
}

impl<'a> TypeDefOrRef<'a> {
    pub fn namespace(&self) -> &'a str {
        match self {
            Self::TypeDef(row) => row.namespace(),
            Self::TypeRef(row) => row.namespace(),
            rest => panic!("{rest:?}"),
        }
    }

    pub fn name(&self) -> &'a str {
        match self {
            Self::TypeDef(row) => row.name(),
            Self::TypeRef(row) => row.name(),
            rest => panic!("{rest:?}"),
        }
    }

    pub fn ty(&self, generics: &[Type]) -> Type {
        if let Self::TypeSpec(def) = self {
            return def.ty(generics);
        }

        if let Self::TypeDef(def) = self {
            let tn = self.type_name();
            return match def.category() {
                TypeCategory::Struct | TypeCategory::Enum => Type::ValueName(tn),
                _ => Type::ClassName(tn),
            };
        }

        Type::ClassName(self.type_name())
    }
}

impl PartialEq<(&str, &str)> for &TypeDefOrRef<'_> {
    fn eq(&self, other: &(&str, &str)) -> bool {
        *self == other
    }
}

impl PartialEq<(&str, &str)> for TypeDefOrRef<'_> {
    fn eq(&self, other: &(&str, &str)) -> bool {
        self.namespace() == other.0 && self.name() == other.1
    }
}

code! { ResolutionScope(2)
    (Module, 0)
    (ModuleRef, 1)
    (AssemblyRef, 2)
    (TypeRef, 3)
}
