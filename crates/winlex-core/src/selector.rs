//! Exact catalog-local definition selectors, shared by CLI queries and type expressions.
use crate::{Architecture, Catalog, Declaration, EntityCategory, EntityId, Result, SearchOptions};
use anyhow::{bail, ensure};

impl Catalog {
    pub fn source_index(&self, selector: &str) -> Result<usize> {
        if let Ok(index) = selector.parse::<usize>() {
            ensure!(index < self.sources().len(), "source index out of range");
            return Ok(index);
        }
        let found: Vec<_> = self
            .sources()
            .iter()
            .filter(|source| source.assembly.as_ref().is_some_and(|a| a.name == selector))
            .collect();
        ensure!(
            found.len() == 1,
            "source selector {selector:?} matches {} files; use a source index",
            found.len()
        );
        Ok(found[0].id)
    }

    /// Select a case-sensitive name or `SOURCE::0xTOKEN`, optionally restricted to types.
    pub fn select_entity(
        &self,
        selector: &str,
        arch: Option<Architecture>,
        types_only: bool,
    ) -> Result<EntityId> {
        let (source, name) = if let Some((source, name)) = selector.split_once("::") {
            (Some(self.source_index(source)?), name)
        } else {
            (None, selector)
        };
        if let Some(token) = name.strip_prefix("0x") {
            ensure!(
                source.is_some() || self.sources().len() == 1,
                "token requires SOURCE::TOKEN with multiple sources"
            );
            let id = EntityId {
                source_id: source.unwrap_or(0),
                token: u32::from_str_radix(token, 16)?,
            };
            let row = self.declaration(id, arch)?;
            ensure!(
                !types_only || matches!(row, Declaration::Type(_)),
                "expected a type"
            );
            return Ok(id);
        }
        let mut matches = self.search_exact(
            name,
            SearchOptions {
                source,
                case_sensitive: true,
                architecture: arch,
                categories: Some(if types_only {
                    EntityCategory::Type as u32
                } else {
                    EntityCategory::ALL & !(EntityCategory::TypeReference as u32)
                }),
                ..Default::default()
            },
        )?;
        let first = matches.next().transpose()?;
        let second = matches.next().transpose()?;
        match (first, second) {
            (Some(item), None) => Ok(item.id),
            (_, second) => bail!(
                "expected one entity, found {}; specify --arch, a qualified name or token",
                if second.is_some() { "2 or more" } else { "0" }
            ),
        }
    }
}
