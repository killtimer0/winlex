//! Demangling for the Visual C++ and Itanium C++ symbol ABIs.
use anyhow::{Context, ensure};
use serde::Serialize;

/// ABI selection for a demangle request. Results always carry a resolved ABI.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Abi {
    #[default]
    Auto,
    Msvc,
    Itanium,
}

impl Abi {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "msvc" | "visual-c++" | "visual_cpp" => Some(Self::Msvc),
            "itanium" => Some(Self::Itanium),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Msvc => "msvc",
            Self::Itanium => "itanium",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Data,
    Special,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Convention {
    Cdecl,
    Stdcall,
    Fastcall,
    Thiscall,
    Pascal,
    Regcall,
}

impl Convention {
    pub fn name(self) -> &'static str {
        match self {
            Self::Cdecl => "cdecl",
            Self::Stdcall => "stdcall",
            Self::Fastcall => "fastcall",
            Self::Thiscall => "thiscall",
            Self::Pascal => "pascal",
            Self::Regcall => "regcall",
        }
    }
}

/// A symbol-name span. Offsets count UTF-16 code units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Demangled {
    /// The input symbol, trimmed.
    pub symbol: String,
    pub text: String,
    pub abi: Abi,
    pub kind: SymbolKind,
    pub convention: Option<Convention>,
    pub spans: Vec<Span>,
}

/// Detect the ABI from the symbol prefix. Plain `_name` is intentionally not
/// detected as a C decoration because it is also a valid undecorated name.
pub fn detect(symbol: &str) -> Option<Abi> {
    if symbol.starts_with('?') {
        Some(Abi::Msvc)
    } else if symbol.starts_with("_Z") {
        Some(Abi::Itanium)
    } else if c_decoration(symbol, true).is_some() {
        Some(Abi::Msvc)
    } else {
        None
    }
}

pub fn demangle(symbol: &str, abi: Abi) -> anyhow::Result<Demangled> {
    let symbol = symbol.trim();
    ensure!(!symbol.is_empty(), "empty symbol");
    let abi = match abi {
        Abi::Auto => detect(symbol).ok_or_else(|| {
            anyhow::anyhow!(
                "cannot determine the symbol ABI; choose Visual C++ or Itanium explicitly"
            )
        })?,
        abi => abi,
    };
    match abi {
        Abi::Msvc => msvc(symbol),
        Abi::Itanium => itanium(symbol),
        Abi::Auto => unreachable!("resolved ABI"),
    }
}

fn msvc(symbol: &str) -> anyhow::Result<Demangled> {
    ensure!(
        !symbol.starts_with("_Z"),
        "Itanium symbol; select the Itanium ABI"
    );
    ensure!(
        !symbol.starts_with("_R") || c_decoration(symbol, true).is_some(),
        "Rust v0 symbols are not supported"
    );
    if let Some((name, convention)) = c_decoration(symbol, false) {
        let text = name.to_owned();
        return Ok(Demangled {
            symbol: symbol.to_owned(),
            spans: name_span(&text, &text),
            text,
            abi: Abi::Msvc,
            kind: SymbolKind::Function,
            convention: Some(convention),
        });
    }
    let parsed = msvc_demangler::parse(symbol)
        .with_context(|| format!("invalid Visual C++ symbol {symbol:?}"))?;
    let flags = msvc_demangler::DemangleFlags::llvm();
    let text = msvc_demangler::serialize(&parsed, flags);
    ensure!(!text.is_empty(), "Visual C++ symbol demangled to nothing");
    let name = msvc_demangler::serialize(&parsed, flags | msvc_demangler::DemangleFlags::NAME_ONLY);
    Ok(Demangled {
        symbol: symbol.to_owned(),
        spans: name_span(&text, name.trim()),
        text,
        abi: Abi::Msvc,
        kind: msvc_kind(&parsed.symbol_type),
        convention: msvc_convention(&parsed.symbol_type),
    })
}

fn msvc_kind(ty: &msvc_demangler::Type<'_>) -> SymbolKind {
    use msvc_demangler::Type;
    match ty {
        Type::NonMemberFunction(..)
        | Type::MemberFunction(..)
        | Type::MemberFunctionPointer(..)
        | Type::VCallThunk(..) => SymbolKind::Function,
        Type::Var(..) => SymbolKind::Data,
        Type::CXXVBTable(..)
        | Type::CXXVFTable(..)
        | Type::RTTIType
        | Type::ThreadSafeStaticGuard(..) => SymbolKind::Special,
        _ => SymbolKind::Unknown,
    }
}

fn msvc_convention(ty: &msvc_demangler::Type<'_>) -> Option<Convention> {
    use msvc_demangler::{CallingConv, Type};
    let convention = match ty {
        Type::NonMemberFunction(convention, ..)
        | Type::MemberFunction(_, convention, ..)
        | Type::MemberFunctionPointer(_, _, convention, ..)
        | Type::VCallThunk(_, convention) => *convention,
        _ => return None,
    };
    Some(match convention {
        CallingConv::Cdecl => Convention::Cdecl,
        CallingConv::Stdcall => Convention::Stdcall,
        CallingConv::Fastcall => Convention::Fastcall,
        CallingConv::Thiscall => Convention::Thiscall,
        CallingConv::Pascal => Convention::Pascal,
        CallingConv::_Regcall => Convention::Regcall,
    })
}

/// `_name`, `_name@N` and `@name@N` decorations. The suffix is required for
/// automatic detection; explicit Visual C++ selection also accepts `_name`.
fn c_decoration(symbol: &str, require_suffix: bool) -> Option<(&str, Convention)> {
    let (body, prefix) = if let Some(body) = symbol.strip_prefix('_') {
        (body, Convention::Cdecl)
    } else {
        (symbol.strip_prefix('@')?, Convention::Fastcall)
    };
    if let Some((name, bytes)) = body.rsplit_once('@')
        && !bytes.is_empty()
        && bytes.bytes().all(|byte| byte.is_ascii_digit())
        && is_identifier(name)
    {
        let convention = if symbol.starts_with('@') {
            Convention::Fastcall
        } else {
            Convention::Stdcall
        };
        return Some((name, convention));
    }
    (!require_suffix && is_identifier(body)).then_some((body, prefix))
}

fn is_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    (first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn itanium(symbol: &str) -> anyhow::Result<Demangled> {
    let parsed = cpp_demangle::Symbol::new(symbol.as_bytes())
        .map_err(|error| anyhow::anyhow!("invalid Itanium symbol {symbol:?}: {error}"))?;
    let mut writer = NameWriter::default();
    parsed
        .structured_demangle(&mut writer, &cpp_demangle::DemangleOptions::default())
        .map_err(|error| anyhow::anyhow!("cannot demangle {symbol:?}: {error}"))?;
    let spans = writer.name_span().into_iter().collect();
    let text = writer.text;
    ensure!(!text.is_empty(), "Itanium symbol demangled to nothing");
    Ok(Demangled {
        symbol: symbol.to_owned(),
        kind: if text.contains('(') {
            SymbolKind::Function
        } else {
            SymbolKind::Data
        },
        convention: None,
        spans,
        text,
        abi: Abi::Itanium,
    })
}

/// Collects the demangled text and the span of the symbol's own qualified name.
#[derive(Default)]
struct NameWriter {
    text: String,
    utf16: usize,
    stack: Vec<(cpp_demangle::DemangleNodeType, usize)>,
    nested_start: Option<usize>,
    nested_end: Option<usize>,
    nested_depth: usize,
    simple: Option<(usize, usize)>,
}

impl cpp_demangle::DemangleWrite for NameWriter {
    fn push_demangle_node(&mut self, node: cpp_demangle::DemangleNodeType) {
        if node == cpp_demangle::DemangleNodeType::NestedName {
            self.nested_depth += 1;
            self.nested_start.get_or_insert(self.utf16);
        }
        self.stack.push((node, self.utf16));
    }
    fn write_string(&mut self, value: &str) -> std::fmt::Result {
        self.text.push_str(value);
        self.utf16 += value.encode_utf16().count();
        Ok(())
    }
    fn pop_demangle_node(&mut self) {
        let Some((node, start)) = self.stack.pop() else {
            return;
        };
        match node {
            cpp_demangle::DemangleNodeType::NestedName => {
                self.nested_depth = self.nested_depth.saturating_sub(1);
                if self.nested_depth == 0 && self.nested_end.is_none() {
                    // Only the symbol's own qualified name; later top-level
                    // nested names belong to parameter or return types.
                    self.nested_end = Some(self.utf16);
                }
            }
            cpp_demangle::DemangleNodeType::UnqualifiedName => {
                self.simple.get_or_insert((start, self.utf16));
            }
            _ => {}
        }
    }
}

impl NameWriter {
    fn name_span(&self) -> Option<Span> {
        // A nested name starting at offset zero is the symbol's own qualified
        // name. Later nested names belong to parameter types of a global
        // function, whose own name is the first unqualified name.
        let (start, end) = match (self.nested_start, self.nested_end) {
            (Some(0), Some(end)) if end > 0 => (0, end),
            _ => self.simple?,
        };
        Some(Span { start, end })
    }
}

fn name_span(text: &str, name: &str) -> Vec<Span> {
    if name.is_empty() {
        return Vec::new();
    }
    let mut fallback = None;
    let mut start = 0;
    while let Some(offset) = text[start..].find(name) {
        let byte = start + offset;
        let end = byte + name.len();
        // The function name is the occurrence that precedes an argument list.
        // For data symbols fall back to the last occurrence, because the type
        // may repeat the name.
        if text[end..].starts_with('(') {
            return vec![span(text, byte, end)];
        }
        fallback = Some((byte, end));
        start = end;
    }
    match fallback {
        Some((byte, end)) => vec![span(text, byte, end)],
        None => Vec::new(),
    }
}

fn span(text: &str, byte: usize, end: usize) -> Span {
    Span {
        start: text[..byte].encode_utf16().count(),
        end: text[..end].encode_utf16().count(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span_text(result: &Demangled) -> Vec<String> {
        let units: Vec<u16> = result.text.encode_utf16().collect();
        result
            .spans
            .iter()
            .map(|span| String::from_utf16(&units[span.start..span.end]).expect("utf16 span"))
            .collect()
    }

    #[test]
    fn msvc_members_and_specials_are_classified_and_qualified_name_is_spanned() {
        let constructor = demangle("??0Foo@@QEAA@XZ", Abi::Msvc).expect("constructor");
        assert_eq!(constructor.kind, SymbolKind::Function);
        assert_eq!(span_text(&constructor), ["Foo::Foo"]);
        let vtable = demangle("??_7Foo@@6B@", Abi::Msvc).expect("vftable");
        assert_eq!(vtable.kind, SymbolKind::Special);
        assert!(!vtable.spans.is_empty());
    }

    #[test]
    fn itanium_span_stops_before_parameter_types() {
        let result = demangle("_ZN3Foo3barEN5space3BazE", Abi::Auto).expect("itanium symbol");
        assert!(result.text.starts_with("Foo::bar("), "{}", result.text);
        assert_eq!(span_text(&result), ["Foo::bar"]);
        let result = demangle(
            "_ZN3Foo3barENSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEE",
            Abi::Auto,
        )
        .expect("itanium symbol with template parameter");
        assert_eq!(span_text(&result), ["Foo::bar"]);
    }

    #[test]
    fn itanium_qualified_names_prefer_the_outer_nested_span() {
        let result = demangle("_ZN5space3fooEibc", Abi::Auto).expect("itanium symbol");
        assert_eq!(result.abi, Abi::Itanium);
        assert_eq!(result.kind, SymbolKind::Function);
        assert_eq!(result.convention, None);
        assert_eq!(result.text, "space::foo(int, bool, char)");
        assert_eq!(span_text(&result), ["space::foo"]);
    }

    #[test]
    fn c_decorations_are_undecorated_with_their_convention() {
        for (symbol, name, convention) in [
            ("_foo@8", "foo", Convention::Stdcall),
            ("@foo@8", "foo", Convention::Fastcall),
            ("_foo", "foo", Convention::Cdecl),
        ] {
            let result = demangle(symbol, Abi::Msvc).expect("c decoration");
            assert_eq!(result.text, name, "{symbol}");
            assert_eq!(result.convention, Some(convention), "{symbol}");
            assert_eq!(span_text(&result), [name], "{symbol}");
        }
        assert_eq!(detect("_foo@8"), Some(Abi::Msvc));
        assert_eq!(detect("_foo"), None);
    }

    #[test]
    fn c_decorations_take_precedence_over_the_rust_prefix() {
        for symbol in ["_RegOpenKeyExW@20", "_ReadFile@20"] {
            assert_eq!(detect(symbol), Some(Abi::Msvc), "{symbol}");
        }
        let result = demangle("_RegOpenKeyExW@20", Abi::Auto).expect("stdcall decoration");
        assert_eq!(result.text, "RegOpenKeyExW");
        assert_eq!(result.convention, Some(Convention::Stdcall));
        assert_eq!(span_text(&result), ["RegOpenKeyExW"]);
        assert!(demangle("_Rhello", Abi::Msvc).is_err());
    }

    #[test]
    fn itanium_global_function_span_ignores_parameter_names() {
        let result = demangle("_Z3fooN5space3BazE", Abi::Auto).expect("global symbol");
        assert_eq!(result.text, "foo(space::Baz)");
        assert_eq!(span_text(&result), ["foo"]);
    }

    #[test]
    fn msvc_span_prefers_the_name_before_the_argument_list() {
        let result = demangle("?foo@@YAPEAUfoo@@XZ", Abi::Msvc).expect("msvc symbol");
        assert!(result.text.contains("foo(void)"), "{}", result.text);
        assert_eq!(span_text(&result), ["foo"]);
    }

    #[test]
    fn real_export_names_keep_their_function_span() {
        let result = demangle(
            "?CreateFileW@@YAPEAXPEBGKKPEAU_SECURITY_ATTRIBUTES@@W4_CREATIONDISPOSITION@@W4_FILE_FLAGS_AND_ATTRIBUTES@@PEAX@Z",
            Abi::Auto,
        )
        .expect("CreateFileW");
        assert_eq!(result.abi, Abi::Msvc);
        assert_eq!(result.kind, SymbolKind::Function);
        assert_eq!(result.convention, Some(Convention::Cdecl));
        assert!(result.text.contains("CreateFileW("), "{}", result.text);
        assert_eq!(span_text(&result), ["CreateFileW"]);
        let result = demangle("??YFoo@@QEAAAEAV0@H@Z", Abi::Msvc).expect("operator");
        assert_eq!(span_text(&result), ["Foo::operator+="]);
    }

    #[test]
    fn unknown_or_rust_prefixes_are_not_detected_as_supported_abis() {
        for symbol in ["foo", "", "_Rhello", "@", "_@1"] {
            assert_eq!(detect(symbol), None, "{symbol:?}");
        }
    }

    #[test]
    fn malformed_symbols_error_without_panicking() {
        for symbol in [
            "?",
            "??",
            "_Z",
            "_ZZ",
            "_Z99999999999999999999foo",
            "?foo@@YAHH@Z trailing",
            "??0",
            "@@",
            "\u{1f600}",
            "_ZN5space3fooE\u{0}",
        ] {
            let _ = demangle(symbol, Abi::Auto);
        }
    }

    #[test]
    fn explicit_abi_rejects_symbols_from_the_other_abi() {
        assert!(demangle("_ZN3Foo3barEi", Abi::Msvc).is_err());
        assert!(demangle("?foo@@YAHH@Z", Abi::Itanium).is_err());
    }
}
