use std::collections::HashMap;
use std::path::{Path, PathBuf};

use windows_rdl::Error;

/// Creates a remapper for flat per-header RDL and WinMD output.
pub fn remap_by_header() -> HeaderRemap {
    HeaderRemap::new()
}

/// Structurally remaps flat metadata into defining-header namespaces.
#[derive(Default)]
pub struct HeaderRemap {
    rdl_dir: PathBuf,
    input: PathBuf,
    output: PathBuf,
    scratch_dir: PathBuf,
    source: String,
    imports: Vec<String>,
    references: Vec<PathBuf>,
    reference_default: bool,
}

impl HeaderRemap {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn rdl_dir(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.rdl_dir = path.as_ref().to_path_buf();
        self
    }

    pub fn input(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.input = path.as_ref().to_path_buf();
        self
    }

    pub fn output(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.output = path.as_ref().to_path_buf();
        self
    }

    pub fn scratch_dir(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.scratch_dir = path.as_ref().to_path_buf();
        self
    }

    pub fn source(&mut self, namespace: &str) -> &mut Self {
        self.source = namespace.to_string();
        self
    }

    /// Adds an RDL namespace import used to restore external TypeRef scopes.
    pub fn import(&mut self, namespace: &str) -> &mut Self {
        self.imports.push(namespace.to_string());
        self
    }

    pub fn reference(&mut self, path: impl AsRef<Path>) -> &mut Self {
        self.references.push(path.as_ref().to_path_buf());
        self
    }

    pub fn reference_default(&mut self) -> &mut Self {
        self.reference_default = true;
        self
    }

    pub fn write(&self) -> Result<(), Error> {
        self.validate()?;
        std::fs::create_dir_all(&self.scratch_dir).map_err(|error| {
            io_error(
                "failed to create scratch directory",
                &self.scratch_dir,
                error,
            )
        })?;

        let routes = self.routes()?;
        let remapped_winmd = self.scratch_dir.join("remapped.winmd");
        windows_metadata::remap()
            .source(&self.source)
            .fallback(&self.source)
            .routes(routes)
            .input(&self.input)
            .output(&remapped_winmd)
            .remap()
            .map_err(|error| {
                Error::new(
                    &format!("failed to remap metadata: {error}"),
                    &self.input.to_string_lossy(),
                    0,
                    0,
                )
            })?;

        let remapped_rdl = self.scratch_dir.join("remapped.rdl");
        windows_rdl::writer()
            .input(&remapped_winmd)
            .output(&remapped_rdl)
            .write()?;
        let normalized = std::fs::read_to_string(&remapped_rdl)
            .map_err(|error| io_error("failed to read remapped RDL", &remapped_rdl, error))?;

        let mut source = String::new();
        for namespace in &self.imports {
            source.push_str("use ");
            source.push_str(namespace);
            source.push_str("::*;\n");
        }
        if !self.imports.is_empty() {
            source.push('\n');
        }
        source.push_str(&normalized);
        std::fs::write(&remapped_rdl, source)
            .map_err(|error| io_error("failed to write remapped RDL", &remapped_rdl, error))?;

        let mut reader = windows_rdl::reader();
        reader.input(&remapped_rdl);
        for reference in &self.references {
            reader.reference(reference);
        }
        if self.reference_default {
            reader.reference_default();
        }
        reader.output(&self.output).write()?;

        let final_rdl = self.scratch_dir.join("final.rdl");
        windows_rdl::writer()
            .input(&self.output)
            .output(&final_rdl)
            .write()?;
        let final_normalized = std::fs::read_to_string(&final_rdl)
            .map_err(|error| io_error("failed to read final RDL", &final_rdl, error))?;
        if normalized != final_normalized {
            let difference = first_text_difference(&normalized, &final_normalized);
            return Err(Error::new(
                &format!("reference-scope roundtrip changed normalized RDL: {difference}"),
                &self.output.to_string_lossy(),
                0,
                0,
            ));
        }

        Ok(())
    }

    fn validate(&self) -> Result<(), Error> {
        for (name, path) in [
            ("RDL directory", &self.rdl_dir),
            ("input", &self.input),
            ("output", &self.output),
            ("scratch directory", &self.scratch_dir),
        ] {
            if path.as_os_str().is_empty() {
                return Err(Error::new(&format!("{name} is required"), "", 0, 0));
            }
        }
        if self.source.is_empty() {
            return Err(Error::new("source namespace is required", "", 0, 0));
        }
        Ok(())
    }

    fn routes(&self) -> Result<HashMap<String, String>, Error> {
        let mut rdl_files: Vec<_> = std::fs::read_dir(&self.rdl_dir)
            .map_err(|error| io_error("failed to read RDL directory", &self.rdl_dir, error))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "rdl"))
            .collect();
        rdl_files.sort();

        let mut routes = HashMap::new();
        for path in rdl_files {
            let stem = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or_else(|| Error::new("RDL file has no UTF-8 stem", "", 0, 0))?;
            let target = format!("{}.{}", self.source, module_stem(stem));
            for name in windows_rdl::item_names(&path, &self.source)? {
                // Header overlap is common in C APIs. Stable file ordering makes the final
                // defining-header owner deterministic, matching the existing remap behavior.
                routes.insert(name, target.clone());
            }
        }
        Ok(routes)
    }
}

fn module_stem(header_stem: &str) -> String {
    let mut stem: String = header_stem
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if stem
        .as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_digit())
    {
        stem.insert(0, '_');
    }
    if is_rust_keyword(&stem) {
        stem.push('_');
    }
    stem
}

fn io_error(message: &str, path: &Path, error: std::io::Error) -> Error {
    Error::new(
        &format!("{message}: {error}"),
        &path.to_string_lossy(),
        0,
        0,
    )
}

fn is_rust_keyword(value: &str) -> bool {
    matches!(
        value,
        "abstract"
            | "as"
            | "async"
            | "await"
            | "become"
            | "box"
            | "break"
            | "const"
            | "continue"
            | "crate"
            | "do"
            | "dyn"
            | "else"
            | "enum"
            | "extern"
            | "false"
            | "final"
            | "fn"
            | "for"
            | "gen"
            | "if"
            | "impl"
            | "in"
            | "let"
            | "loop"
            | "macro"
            | "match"
            | "mod"
            | "move"
            | "override"
            | "priv"
            | "pub"
            | "ref"
            | "return"
            | "self"
            | "static"
            | "struct"
            | "super"
            | "trait"
            | "true"
            | "try"
            | "type"
            | "typeof"
            | "unsafe"
            | "unsized"
            | "use"
            | "virtual"
            | "where"
            | "while"
            | "yield"
    )
}

fn first_text_difference(expected: &str, actual: &str) -> String {
    let mut expected_lines = expected.lines();
    let mut actual_lines = actual.lines();
    let mut line = 1;
    loop {
        match (expected_lines.next(), actual_lines.next()) {
            (Some(expected), Some(actual)) if expected == actual => line += 1,
            (Some(expected), Some(actual)) => {
                return format!("line {line}: expected `{expected}`, actual `{actual}`");
            }
            (Some(expected), None) => {
                return format!("line {line}: expected `{expected}`, actual end of file");
            }
            (None, Some(actual)) => {
                return format!("line {line}: expected end of file, actual `{actual}`");
            }
            (None, None) => return "different line endings".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_underscore_to_keyword_module_names() {
        assert_eq!(module_stem("in"), "in_");
        assert_eq!(module_stem("struct_tm"), "struct_tm");
    }
}
