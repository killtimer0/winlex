//! Error-message queries independent of metadata and presentation.
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "path", rename_all = "snake_case")]
pub enum Scope {
    Auto,
    Win32,
    HResult,
    NtStatus,
    Errno,
    Module(#[serde(serialize_with = "serialize_path")] PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Request {
    pub code: u32,
    pub scope: Scope,
    /// Zero uses source defaults; nonzero requests one language.
    pub language: u16,
}

impl Request {
    pub fn validate(&self) -> crate::Result<()> {
        if let Scope::Module(path) = &self.scope {
            anyhow::ensure!(!path.as_os_str().is_empty(), "module path is empty");
            #[cfg(windows)]
            {
                use std::os::windows::ffi::OsStrExt;
                anyhow::ensure!(
                    !path.as_os_str().encode_wide().any(|c| c == 0),
                    "module path contains NUL"
                );
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Scheme {
    Win32,
    HResult,
    NtStatus,
    Errno,
    Module,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "path", rename_all = "snake_case")]
pub enum Source {
    System,
    NtDll,
    Msvcrt,
    Module(#[serde(serialize_with = "serialize_path")] PathBuf),
}

fn serialize_path<S: serde::Serializer>(path: &Path, serializer: S) -> Result<S::Ok, S::Error> {
    if let Some(text) = path.to_str() {
        return serializer.serialize_str(text);
    }
    #[cfg(windows)]
    {
        use serde::ser::SerializeMap;
        use std::os::windows::ffi::OsStrExt;
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry("utf16", &path.as_os_str().encode_wide().collect::<Vec<_>>())?;
        map.end()
    }
    #[cfg(not(windows))]
    Err(serde::ser::Error::custom("path is not Unicode"))
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Encoding {
    pub severity: u8,
    pub customer: bool,
    pub facility: u16,
    pub code: u16,
    /// HRESULT's NT mapping bit (not part of the facility).
    pub nt_mapping: bool,
}

pub fn encoding(code: u32, scheme: Scheme) -> Option<Encoding> {
    match scheme {
        Scheme::HResult => Some(Encoding {
            severity: (code >> 31) as u8,
            customer: code & 0x20000000 != 0,
            facility: ((code >> 16) & 0x7ff) as u16,
            code: code as u16,
            nt_mapping: code & 0x10000000 != 0,
        }),
        Scheme::NtStatus => Some(Encoding {
            severity: (code >> 30) as u8,
            customer: code & 0x20000000 != 0,
            facility: ((code >> 16) & 0xfff) as u16,
            code: code as u16,
            nt_mapping: false,
        }),
        _ => None,
    }
}

/// Recognize only the canonical HRESULT_FROM_WIN32 wrapping pattern.
pub fn related_win32(code: u32) -> Option<u32> {
    (code & 0xffff0000 == 0x80070000 && code & 0xffff != 0).then_some(code & 0xffff)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageStatus {
    Found,
    NotFound,
    LanguageNotFound,
    NoMessageTable,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Failure {
    pub operation: String,
    pub os_code: Option<u32>,
    pub detail: String,
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.operation, self.detail)?;
        if let Some(code) = self.os_code {
            write!(f, " (Windows error {code})")?;
        }
        Ok(())
    }
}

impl std::error::Error for Failure {}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Message {
    pub code: u32,
    pub source: Source,
    pub requested_language: u16,
    /// None when the lookup API cannot reliably report its selected language.
    pub actual_language: Option<u16>,
    pub status: MessageStatus,
    pub text: Option<String>,
    pub failure: Option<Failure>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RelatedCode {
    pub relationship: &'static str,
    pub message: Message,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub scheme: Scheme,
    pub encoding: Option<Encoding>,
    pub message: Message,
    pub related: Vec<RelatedCode>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct QueryResult {
    pub request: Request,
    pub candidates: Vec<Candidate>,
    /// In automatic mode, unsuccessful attempts remain observable without
    /// presenting their bit decompositions as identified domains.
    pub attempts: Vec<Message>,
}

impl QueryResult {
    pub fn has_failures(&self) -> bool {
        self.attempts.iter().any(|m| m.failure.is_some())
            || self.candidates.iter().any(|c| {
                c.message.failure.is_some() || c.related.iter().any(|r| r.message.failure.is_some())
            })
    }
}

pub fn query(request: &Request) -> crate::Result<QueryResult> {
    request.validate()?;
    let schemes: &[Scheme] = match request.scope {
        Scope::Auto => &[Scheme::Win32, Scheme::HResult, Scheme::NtStatus],
        Scope::Win32 => &[Scheme::Win32],
        Scope::HResult => &[Scheme::HResult],
        Scope::NtStatus => &[Scheme::NtStatus],
        Scope::Errno => &[Scheme::Errno],
        Scope::Module(_) => &[Scheme::Module],
    };
    let mut result = QueryResult {
        request: request.clone(),
        candidates: vec![],
        attempts: vec![],
    };
    for &scheme in schemes {
        let source = match scheme {
            Scheme::NtStatus => Source::NtDll,
            Scheme::Errno => Source::Msvcrt,
            Scheme::Module => match &request.scope {
                Scope::Module(p) => Source::Module(p.clone()),
                _ => unreachable!(),
            },
            _ => Source::System,
        };
        let primary = message(request.code, request.language, source);
        let related = if scheme == Scheme::HResult {
            related_win32(request.code)
                .map(|code| RelatedCode {
                    relationship: "hresult_from_win32",
                    message: message(code, request.language, Source::System),
                })
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            vec![]
        };
        let identified = primary.status == MessageStatus::Found || !related.is_empty();
        if request.scope != Scope::Auto || identified {
            result.candidates.push(Candidate {
                scheme,
                encoding: encoding(request.code, scheme),
                message: primary,
                related,
            });
        } else {
            result.attempts.push(primary);
        }
    }
    Ok(result)
}

fn message(code: u32, language: u16, source: Source) -> Message {
    let mut message = Message {
        code,
        source,
        requested_language: language,
        actual_language: None,
        status: MessageStatus::NotFound,
        text: None,
        failure: None,
    };
    match platform::lookup(code, language, &message.source) {
        Ok((text, actual_language)) => {
            message.status = MessageStatus::Found;
            message.actual_language = actual_language;
            message.text = Some(text);
        }
        Err(failure) => {
            message.status = match failure.os_code {
                Some(317 | 1814) => MessageStatus::NotFound,
                Some(1812 | 1813) => MessageStatus::NoMessageTable,
                Some(1815) => MessageStatus::LanguageNotFound,
                _ => MessageStatus::Failed,
            };
            if message.status == MessageStatus::Failed {
                message.failure = Some(failure);
            }
        }
    }
    message
}

/// Loads resources for this call only; no handle is retained after return.
pub fn module_languages(path: &Path) -> Result<Vec<u16>, Failure> {
    platform::module_languages(path)
}

/// Unsorted, deduplicated language IDs; presentation supplies interface-language priority.
pub fn system_languages() -> Vec<u16> {
    platform::system_languages()
}

#[cfg(windows)]
mod platform;

#[cfg(not(windows))]
mod platform {
    use super::*;
    fn unsupported() -> Failure {
        Failure {
            operation: "platform".into(),
            os_code: None,
            detail: "Windows message resources require Windows".into(),
        }
    }
    pub fn lookup(_: u32, _: u16, _: &Source) -> Result<(String, Option<u16>), Failure> {
        Err(unsupported())
    }
    pub fn module_languages(_: &Path) -> Result<Vec<u16>, Failure> {
        Err(unsupported())
    }
    pub fn system_languages() -> Vec<u16> {
        vec![0, 0x409]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encoding_relations_are_not_low_word_guesses() {
        assert_eq!(related_win32(0x80070005), Some(5));
        for code in [5, 0x00070005, 0xA0070005, 0xC0000005, 0x80070000] {
            assert_eq!(related_win32(code), None);
        }
        let nt = encoding(0xC0000005, Scheme::NtStatus).unwrap();
        assert_eq!((nt.severity, nt.facility, nt.code), (3, 0, 5));
        let hr = encoding(0x80070005, Scheme::HResult).unwrap();
        assert_eq!((hr.severity, hr.facility, hr.code), (1, 7, 5));
    }

    #[cfg(windows)]
    #[test]
    fn windows_queries_preserve_identity_and_language_contract() {
        let lookup = |code, scope| {
            query(&Request {
                code,
                scope,
                language: 0,
            })
            .unwrap()
        };
        let request = Request {
            code: 0x80070005,
            scope: Scope::HResult,
            language: 0x409,
        };
        let result = query(&request).unwrap();
        assert!(!result.has_failures(), "{result:?}");
        let candidate = &result.candidates[0];
        assert_eq!(candidate.scheme, Scheme::HResult);
        assert_eq!(candidate.message.code, request.code);
        assert_eq!(candidate.related[0].message.code, 5);
        assert_eq!(candidate.related[0].message.status, MessageStatus::Found);
        assert_eq!(candidate.related[0].message.actual_language, Some(0x409));
        let automatic = lookup(5, Scope::Win32);
        assert_eq!(automatic.candidates[0].message.actual_language, None);
        let unknown = lookup(0x12345678, Scope::Auto);
        assert!(unknown.candidates.is_empty(), "{unknown:?}");
        assert!(!unknown.has_failures(), "{unknown:?}");
        let nt = lookup(0xC0000005, Scope::NtStatus);
        assert_eq!(nt.candidates[0].message.status, MessageStatus::Found);
        let text = nt.candidates[0].message.text.as_deref().unwrap();
        assert!(text.matches('%').count() >= 3, "{text:?}");
        assert!(text.contains("%s"), "{text:?}");
        assert!(nt.candidates[0].message.actual_language.is_some());
    }
}
