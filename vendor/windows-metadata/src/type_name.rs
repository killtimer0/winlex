use super::*;

#[derive(Debug, PartialEq, Clone)]
pub struct TypeName {
    pub namespace: String,
    pub name: String,
    pub generics: Vec<Type>,
    /// Original file index and ECMA token, when read from a signature.
    pub identity: Option<(usize, u32)>,
}

impl TypeName {
    pub fn named(namespace: &str, name: &str) -> Self {
        Self {
            namespace: namespace.to_string(),
            name: name.to_string(),
            generics: vec![],
            identity: None,
        }
    }
}

impl PartialEq<(&str, &str)> for &TypeName {
    fn eq(&self, other: &(&str, &str)) -> bool {
        self.namespace == other.0 && self.name == other.1
    }
}
