//! Tool work and results independent of native document controls.
use std::{
    fs::File,
    io::Read,
    os::windows::{io::FromRawHandle, process::CommandExt},
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
use winlex_core::tools::{
    com::{Guid, activation, typelib},
    error_code::{self, Failure, QueryResult, Request},
};

const ACTIVATE_TIMEOUT: Duration = Duration::from_secs(10);
/// A child may leave its pipe open in an inherited handle; never block on it.
const OUTPUT_GRACE: Duration = Duration::from_secs(2);

pub mod demangle;

pub enum Job {
    Demangle {
        revision: u64,
        input: String,
        abi: winlex_core::tools::demangle::Abi,
    },
    Query {
        revision: u64,
        request: Request,
    },
    Languages {
        path: PathBuf,
    },
    TypeLibraries {
        libraries: Vec<(Guid, u16, u16)>,
    },
    Activate {
        guid: Guid,
    },
}

pub enum Completion {
    Demangle {
        revision: u64,
        result: anyhow::Result<demangle::Output>,
    },
    Query {
        revision: u64,
        result: anyhow::Result<QueryResult>,
    },
    Languages {
        path: PathBuf,
        result: Result<Vec<u16>, Failure>,
    },
    TypeLibraries {
        results: Vec<TypeLibraryResult>,
    },
    Activate {
        result: Result<activation::Instance, ActivationError>,
    },
}

pub struct TypeLibraryResult {
    pub guid: Guid,
    pub result: Result<typelib::TypeLibrary, typelib::Error>,
}

pub enum ActivationError {
    TimedOut(u64),
    Failed(String),
}

pub fn run(job: Job, ticket: &crate::worker::Ticket) -> Completion {
    match job {
        Job::Demangle {
            revision,
            input,
            abi,
        } => Completion::Demangle {
            revision,
            result: demangle::run(&input, abi, ticket),
        },
        Job::Query { revision, request } => Completion::Query {
            revision,
            result: error_code::query(&request),
        },
        Job::Languages { path } => Completion::Languages {
            result: error_code::module_languages(&path),
            path,
        },
        Job::TypeLibraries { libraries } => Completion::TypeLibraries {
            results: libraries
                .into_iter()
                .map(|(guid, major, minor)| TypeLibraryResult {
                    guid,
                    result: typelib::load(guid, major, minor),
                })
                .collect(),
        },
        Job::Activate { guid } => Completion::Activate {
            result: inspect_isolated(guid),
        },
    }
}

#[derive(serde::Deserialize)]
struct Response {
    ok: bool,
    #[serde(default)]
    instance: Option<activation::Instance>,
    #[serde(default)]
    error: Option<String>,
}

fn activation_pipe() -> std::io::Result<(File, File)> {
    let (mut read, mut write) = (std::ptr::null_mut(), std::ptr::null_mut());
    // std's overlapped child pipe uses unnamed NT pipe creation unsupported on XP.
    // Our dedicated reader thread only needs a synchronous CreatePipe handle.
    if unsafe {
        windows_sys::Win32::System::Pipes::CreatePipe(&mut read, &mut write, std::ptr::null(), 0)
    } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    // Neither original handle is inheritable; Command duplicates only its writer.
    Ok(unsafe { (File::from_raw_handle(read), File::from_raw_handle(write)) })
}

/// Starts this executable in its hidden activation mode and waits for one JSON
/// response. The child is terminated when it exceeds the timeout.
fn inspect_isolated(guid: Guid) -> Result<activation::Instance, ActivationError> {
    let exe = std::env::current_exe().map_err(|error| {
        ActivationError::Failed(format!("Cannot locate the executable: {error}"))
    })?;
    let (mut stdout, write) = activation_pipe().map_err(|error| {
        ActivationError::Failed(format!("Cannot create the activation output pipe: {error}"))
    })?;
    let mut child = Command::new(exe)
        .arg("--inspect-com")
        .arg(guid.to_string())
        .stdin(Stdio::null())
        .stdout(write)
        .stderr(Stdio::null())
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
        .spawn()
        .map_err(|error| {
            ActivationError::Failed(format!("Cannot start the isolated process: {error}"))
        })?;
    // Drain the pipe while the child runs; the result can exceed its buffer.
    // The channel keeps the wait bounded if another process inherits the handle.
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stdout.read_to_end(&mut buffer);
        let _ = sender.send(buffer);
    });
    let deadline = Instant::now() + ACTIVATE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => break None,
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ActivationError::Failed(format!(
                    "Cannot wait for the isolated process: {error}"
                )));
            }
        }
    };
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(ActivationError::TimedOut(ACTIVATE_TIMEOUT.as_secs()));
    };
    let output = receiver.recv_timeout(OUTPUT_GRACE).unwrap_or_default();
    let response: Response = serde_json::from_slice(&output).map_err(|error| {
        ActivationError::Failed(format!(
            "The isolated process exited with code {} without a response: {error}",
            status.code().unwrap_or(-1)
        ))
    })?;
    if !response.ok {
        return Err(ActivationError::Failed(response.error.unwrap_or_else(
            || {
                format!(
                    "The isolated process exited with code {}",
                    status.code().unwrap_or(-1)
                )
            },
        )));
    }
    response.instance.ok_or_else(|| {
        ActivationError::Failed("The activation response contains no instance.".into())
    })
}
