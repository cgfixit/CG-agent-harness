//! Atomic Windows process-tree ownership. The kernel assigns the Job Object
//! during CreateProcess, before any child instruction can run. No breakaway or
//! inheritable job handle is enabled. This is not filesystem/network isolation.
use std::ffi::OsStr;
use std::fs::File;
use std::io;
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::os::windows::process::ExitStatusExt;
use std::process::{Command, ExitStatus};
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, TerminateJobObject,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_JOB_MEMORY,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW;
use windows_sys::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess, InitializeProcThreadAttributeList,
    UpdateProcThreadAttribute, WaitForSingleObject, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT,
    EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_JOB_LIST, STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

pub struct JobChild {
    process: OwnedHandle,
    job: OwnedHandle,
    pid: u32,
    pub stdin: Option<File>,
    pub stdout: Option<File>,
    pub stderr: Option<File>,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

pub fn windows_directory() -> io::Result<std::path::PathBuf> {
    let mut buffer = vec![0u16; 32768];
    // SAFETY: writable UTF-16 buffer with the supplied capacity.
    let size = unsafe { GetWindowsDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
    if size == 0 || size >= buffer.len() {
        return Err(io::Error::last_os_error());
    }
    Ok(std::ffi::OsString::from_wide(&buffer[..size]).into())
}

fn wide(value: &OsStr) -> io::Result<Vec<u16>> {
    let mut value: Vec<_> = value.encode_wide().collect();
    if value.contains(&0) {
        return Err(invalid("NUL in Windows process input"));
    }
    value.push(0);
    Ok(value)
}

// Always quote each native executable argument using the Microsoft CRT rules.
// Batch scripts are refused; cmd.exe's command language is not argv escaping.
fn command_line(command: &Command) -> io::Result<Vec<u16>> {
    let mut out = Vec::new();
    for arg in std::iter::once(command.get_program()).chain(command.get_args()) {
        if !out.is_empty() {
            out.push(b' ' as u16);
        }
        out.push(b'"' as u16);
        let mut slashes = 0;
        for ch in arg.encode_wide() {
            match ch {
                0 => return Err(invalid("NUL in Windows process argument")),
                92 => {
                    slashes += 1;
                    continue;
                }
                34 => {
                    out.extend(std::iter::repeat_n(92, slashes * 2 + 1));
                }
                _ => {
                    out.extend(std::iter::repeat_n(92, slashes));
                }
            }
            out.push(ch);
            slashes = 0;
        }
        out.extend(std::iter::repeat_n(92, slashes * 2));
        out.push(b'"' as u16);
    }
    out.push(0);
    if out.len() > 32767 {
        return Err(invalid("Windows command line exceeds limit"));
    }
    Ok(out)
}

fn environment(command: &Command) -> io::Result<Vec<u16>> {
    let mut entries = std::collections::BTreeMap::new();
    for (key, value) in command.get_envs() {
        let Some(value) = value else {
            continue;
        };
        let key = key.to_str().ok_or_else(|| invalid("invalid environment name"))?;
        if key.is_empty() || key.contains(['=', '\0']) {
            return Err(invalid("invalid environment name"));
        }
        if entries.insert(key.to_ascii_uppercase(), (key, value)).is_some() {
            return Err(invalid("duplicate case-insensitive environment name"));
        }
    }
    let mut out = Vec::new();
    for (_, (key, value)) in entries {
        out.extend(key.encode_utf16());
        out.push(b'=' as u16);
        out.extend(wide(value)?);
    }
    if out.is_empty() {
        out.push(0);
    }
    out.push(0);
    if out.len() > 65536 {
        return Err(invalid("Windows environment exceeds limit"));
    }
    Ok(out)
}

fn pipe(child_reads: bool) -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut read = null_mut();
    let mut write = null_mut();
    let security = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 1,
    };
    // SAFETY: initialized security descriptor and writable handle slots. On
    // success each returned handle is adopted exactly once by OwnedHandle.
    if unsafe { CreatePipe(&mut read, &mut write, &security, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let read = unsafe { OwnedHandle::from_raw_handle(read) };
    let write = unsafe { OwnedHandle::from_raw_handle(write) };
    let (parent, child) = if child_reads { (write, read) } else { (read, write) };
    // SAFETY: valid owned handle. Only the child ends may be inherited.
    if unsafe { SetHandleInformation(parent.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((parent, child))
}

struct Attributes(Vec<usize>);
impl Attributes {
    fn new() -> io::Result<Self> {
        let mut bytes = 0;
        // SAFETY: documented sizing call, followed by suitably aligned storage.
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 2, 0, &mut bytes);
        }
        if bytes == 0 || bytes > 65536 {
            return Err(io::Error::last_os_error());
        }
        let mut slots = vec![0usize; bytes.div_ceil(std::mem::size_of::<usize>())];
        if unsafe { InitializeProcThreadAttributeList(slots.as_mut_ptr().cast(), 2, 0, &mut bytes) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(slots))
    }
    fn handles(&mut self, attribute: u32, values: &[HANDLE]) -> io::Result<()> {
        // SAFETY: initialized list and a live handle slice retained until the
        // list is destroyed after CreateProcessW (Update retains the pointer).
        if unsafe {
            UpdateProcThreadAttribute(
                self.0.as_mut_ptr().cast(),
                0,
                attribute as usize,
                values.as_ptr().cast(),
                std::mem::size_of_val(values),
                null_mut(),
                null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: the list was initialized and has not been freed.
        unsafe { DeleteProcThreadAttributeList(self.0.as_mut_ptr().cast()) };
    }
}

impl JobChild {
    /// `memory_mb: None` sets no job memory limit (verification checks never had one).
    pub fn spawn(command: &Command, processes: u32, memory_mb: Option<u32>) -> io::Result<Self> {
        if !(8..=128).contains(&processes) || memory_mb.is_some_and(|m| !(64..=4096).contains(&m)) {
            return Err(invalid("invalid Job Object resource limits"));
        }
        let path = std::path::Path::new(command.get_program());
        if !path.is_absolute()
            || !path
                .extension()
                .and_then(OsStr::to_str)
                .is_some_and(|s| s.eq_ignore_ascii_case("exe"))
        {
            return Err(invalid("Job Object command requires an absolute native .exe path"));
        }
        let application = wide(command.get_program())?;
        let mut argv = command_line(command)?;
        let environment = environment(command)?;
        let cwd = wide(
            command
                .get_current_dir()
                .ok_or_else(|| invalid("explicit working directory required"))?
                .as_os_str(),
        )?;
        // SAFETY: null security attributes make the unnamed job handle non-inheritable.
        let raw = unsafe { CreateJobObjectW(null(), null()) };
        if raw.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.BasicLimitInformation.ActiveProcessLimit = processes;
        if let Some(memory_mb) = memory_mb {
            limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
            limits.JobMemoryLimit = (u64::from(memory_mb) * 1024 * 1024)
                .try_into()
                .map_err(|_| invalid("memory limit does not fit this platform"))?;
        }
        // SAFETY: owned job and correctly sized initialized limit structure.
        if unsafe {
            SetInformationJobObject(
                job.as_raw_handle(),
                JobObjectExtendedLimitInformation,
                (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&limits) as u32,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let (stdin, input) = pipe(true)?;
        let (stdout, output) = pipe(false)?;
        let (stderr, error) = pipe(false)?;
        let inherited = [input.as_raw_handle(), output.as_raw_handle(), error.as_raw_handle()];
        let jobs = [job.as_raw_handle()];
        let mut attributes = Attributes::new()?;
        attributes.handles(PROC_THREAD_ATTRIBUTE_HANDLE_LIST, &inherited)?;
        attributes.handles(PROC_THREAD_ATTRIBUTE_JOB_LIST, &jobs)?;
        let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
        startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = inherited[0];
        startup.StartupInfo.hStdOutput = inherited[1];
        startup.StartupInfo.hStdError = inherited[2];
        startup.lpAttributeList = attributes.0.as_mut_ptr().cast();
        let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
        // SAFETY: all UTF-16 inputs are bounded/NUL-terminated, argv is writable,
        // attribute backing arrays outlive this call. Only the three pipe ends
        // are inherited. JOB_LIST establishes ownership atomically at creation.
        if unsafe {
            CreateProcessW(
                application.as_ptr(),
                argv.as_mut_ptr(),
                null(),
                null(),
                1,
                CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW,
                environment.as_ptr().cast(),
                cwd.as_ptr(),
                &startup.StartupInfo,
                &mut info,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
        let thread = unsafe { OwnedHandle::from_raw_handle(info.hThread) };
        drop(thread);
        Ok(Self {
            process,
            job,
            pid: info.dwProcessId,
            stdin: Some(stdin.into()),
            stdout: Some(stdout.into()),
            stderr: Some(stderr.into()),
        })
    }

    pub fn id(&self) -> Option<u32> {
        Some(self.pid)
    }

    pub fn start_kill(&mut self) -> io::Result<()> {
        // SAFETY: this private job is owned by this invocation only.
        if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        // SAFETY: valid process handle, nonblocking wait.
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut exit = 0;
                if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &mut exit) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Some(ExitStatus::from_raw(exit)))
            }
            _ => Err(io::Error::last_os_error()),
        }
    }
}

impl Drop for JobChild {
    fn drop(&mut self) {
        let _ = self.start_kill();
    }
}
