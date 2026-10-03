//! OS credential store for managed provider keys, plus the one-time `.env` migration.
//!
//! Read order at startup is an inherited environment variable, then this store.
//! A legacy home `.env` is input to [`migrate_text`] only, unless
//! `security.allow_plaintext_key_file` is the literal boolean `true`.
//! Values are never included in errors, warnings, or `Debug` output from this module.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use subtle::ConstantTimeEq;

pub const SERVICE: &str = "CGagentHarness";
pub const PLAINTEXT_KEY_FILE: &str = "security.allow_plaintext_key_file";
pub const OS_STORE_LABEL: &str = "os-credential-store";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreError {
    Unavailable,
    WriteFailed,
    VerifyMismatch,
    ReadFailed,
}

/// Platform credential store. Unit tests supply a mock; production uses [`OsCredentialStore`].
pub trait CredentialStore: Send + Sync {
    fn get(&self, name: &str) -> Result<Option<String>, StoreError>;
    fn set(&self, name: &str, value: &str) -> Result<(), StoreError>;
    fn delete(&self, name: &str) -> Result<(), StoreError>;
}

pub struct MigrationReport {
    pub migrated: Vec<String>,
    pub failed: Vec<(String, StoreError)>,
    pub text: String,
}

/// Write each managed assignment, read it back, and drop only the lines that verified.
/// Unknown lines stay. A failed name is left in `text` and is not reported as migrated.
pub fn migrate_text(
    text: &str,
    assignments: &BTreeMap<String, String>,
    store: &dyn CredentialStore,
) -> MigrationReport {
    let mut remove = BTreeSet::new();
    let mut migrated = Vec::new();
    let mut failed = Vec::new();
    for (name, value) in assignments {
        match store_and_verify(store, name, value) {
            Ok(()) => {
                remove.insert(name.clone());
                migrated.push(name.clone());
            }
            Err(error) => failed.push((name.clone(), error)),
        }
    }
    MigrationReport {
        migrated,
        failed,
        text: strip_assignments(text, &remove),
    }
}

fn store_and_verify(store: &dyn CredentialStore, name: &str, value: &str) -> Result<(), StoreError> {
    store.set(name, value)?;
    match store.get(name) {
        Ok(Some(got)) if secret_eq(&got, value) => Ok(()),
        Ok(_) => {
            let _ = store.delete(name);
            Err(StoreError::VerifyMismatch)
        }
        Err(StoreError::Unavailable) => Err(StoreError::Unavailable),
        Err(_) => {
            let _ = store.delete(name);
            Err(StoreError::VerifyMismatch)
        }
    }
}

pub fn secret_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    left.len() == right.len() && bool::from(left.ct_eq(right))
}

pub fn migration_warning(name: &str, error: StoreError) -> String {
    let why = match error {
        StoreError::Unavailable => "the OS credential store is unavailable",
        StoreError::WriteFailed => "the OS credential store refused the write",
        StoreError::VerifyMismatch => "the OS credential store read-back did not match",
        StoreError::ReadFailed => "the OS credential store could not be read",
    };
    format!(
        "{name} remains in the private .env file because {why}. It was not loaded. Unlock the OS credential store (macOS Keychain, Linux Secret Service, or Windows Credential Manager), or set {PLAINTEXT_KEY_FILE}: true and restart."
    )
}

pub fn save_refusal(error: StoreError) -> &'static str {
    match error {
        StoreError::Unavailable => {
            "OS credential store is unavailable, so the key was not saved. On Linux, start a Secret Service provider in a session that sets DBUS_SESSION_BUS_ADDRESS. On macOS, unlock Keychain. On Windows, use Credential Manager. To keep the legacy 0600 .env file, set security.allow_plaintext_key_file: true and restart."
        }
        StoreError::VerifyMismatch => {
            "OS credential store read-back did not match, so the key was not saved."
        }
        StoreError::WriteFailed | StoreError::ReadFailed => {
            "OS credential store refused the update, so the key was not saved."
        }
    }
}

/// macOS Keychain, Windows Credential Manager, or Linux Secret Service.
/// Entries are scoped by the canonical home path so two homes do not share keys.
pub struct OsCredentialStore {
    home: String,
    unavailable: AtomicBool,
}

/// Service string that isolates one harness home.
///
/// keyring 3's `target` is not a namespace. On macOS it must be a keychain
/// domain (`User`, `System`, `Common`, or `Dynamic`); any other string makes
/// `Entry::new_with_target` fail before the login keychain is opened. On
/// Windows that same argument replaces the per-entry target name, so one
/// shared target would collapse every managed key into a single credential.
/// Linux uses it as a Secret Service collection label. `Entry::new` keeps
/// each platform's default (login keychain, `user.service`, default collection)
/// and the canonical home lives in the service string instead. Backslashes
/// become slashes so a Windows path is a legal Credential Manager target name.
pub(crate) fn service_name(home: &str) -> String {
    format!("{SERVICE} ({})", home.replace('\\', "/"))
}

impl OsCredentialStore {
    pub fn for_home(home: &Path) -> Self {
        let home = std::fs::canonicalize(home)
            .unwrap_or_else(|_| home.to_path_buf())
            .display()
            .to_string();
        Self {
            home,
            unavailable: AtomicBool::new(platform_store_missing()),
        }
    }

    fn entry(&self, name: &str) -> Result<keyring::Entry, StoreError> {
        if self.unavailable.load(Ordering::Acquire) {
            return Err(StoreError::Unavailable);
        }
        keyring::Entry::new(&service_name(&self.home), name).map_err(|error| self.classify(error))
    }

    fn classify(&self, error: keyring::Error) -> StoreError {
        match error {
            keyring::Error::NoStorageAccess(_) => {
                self.unavailable.store(true, Ordering::Release);
                StoreError::Unavailable
            }
            keyring::Error::NoEntry => StoreError::ReadFailed,
            _ => StoreError::WriteFailed,
        }
    }
}

impl CredentialStore for OsCredentialStore {
    fn get(&self, name: &str) -> Result<Option<String>, StoreError> {
        let entry = self.entry(name)?;
        // Provider tokens are UTF-8 strings. Read the secret bytes instead of
        // `get_password`, and drop a non-UTF-8 blob without formatting it.
        match entry.get_secret() {
            Ok(bytes) => String::from_utf8(bytes).map(Some).map_err(|_| StoreError::ReadFailed),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(self.classify(error)),
        }
    }

    fn set(&self, name: &str, value: &str) -> Result<(), StoreError> {
        let entry = self.entry(name)?;
        entry.set_password(value).map_err(|error| self.classify(error))
    }

    fn delete(&self, name: &str) -> Result<(), StoreError> {
        let entry = self.entry(name)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(self.classify(error)),
        }
    }
}

/// Headless Linux without a session bus cannot reach Secret Service. Fail closed
/// before the client blocks. macOS and Windows always attempt the native store.
fn platform_store_missing() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none_or(|value| value.is_empty())
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

const QUOTE_ESCAPE: &str = r"'\''";

pub(crate) fn split_assignment(line: &str) -> Option<(String, String)> {
    let mut stripped = line.trim();
    if stripped.is_empty() || stripped.starts_with('#') {
        return None;
    }
    if let Some(rest) = stripped.strip_prefix("export ") {
        stripped = rest.trim_start();
    }
    let (name, raw) = stripped.split_once('=')?;
    Some((name.trim().to_string(), raw.to_string()))
}

pub(crate) fn unquote(raw: &str) -> String {
    let token = raw.trim();
    if token.len() >= 2 {
        let first = token.chars().next().unwrap();
        let last = token.chars().last().unwrap();
        if first == last && (first == '\'' || first == '"') {
            let inner = &token[1..token.len() - 1];
            return if first == '\'' {
                inner.replace(QUOTE_ESCAPE, "'")
            } else {
                inner.to_string()
            };
        }
    }
    token.to_string()
}

pub(crate) fn parse_managed(text: &str, is_managed: impl Fn(&str) -> bool) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    for line in text.lines() {
        if let Some((name, raw)) = split_assignment(line) {
            if is_managed(&name) {
                found.insert(name, unquote(&raw));
            }
        }
    }
    found
}

pub(crate) fn strip_assignments(text: &str, remove: &BTreeSet<String>) -> String {
    if remove.is_empty() {
        return text.to_string();
    }
    let mut out = String::new();
    for line in text.split_inclusive('\n') {
        let logical = line.trim_end_matches(['\n', '\r']);
        if let Some((name, _)) = split_assignment(logical) {
            if remove.contains(&name) {
                continue;
            }
        }
        out.push_str(line);
    }
    out
}

/// Private credential file: regular, owner-only, not a symlink, at most 64 KiB.
/// Missing is empty. The bytes are migration input or the plaintext opt-in, never a log line.
pub(crate) fn read_private_text(path: &Path) -> anyhow::Result<String> {
    use std::io::Read;
    use std::path::Component;
    // rust/path-injection's only barrier is str::contains of these literals.
    // Component::ParentDir is the real check; the string checks are what the query sees.
    let rendered = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("credential file path refused"))?;
    if rendered.contains("../") {
        anyhow::bail!("credential file path refused");
    }
    if rendered.contains("..\\") {
        anyhow::bail!("credential file path refused");
    }
    let path = Path::new(rendered);
    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        anyhow::bail!("credential file path refused");
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(_) => anyhow::bail!("credential file unreadable; check its owner, access and symlink status"),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        anyhow::bail!("credential file must be a regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: getuid has no arguments or memory effects.
        if metadata.uid() != unsafe { libc::getuid() } || metadata.mode() & 0o077 != 0 || metadata.nlink() != 1 {
            anyhow::bail!("credential file must be private, singly linked, and owned by this user (0600)");
        }
    }
    #[cfg(windows)]
    windows_private::validate_file(&file)?;
    let mut text = String::new();
    file.take(65537)
        .read_to_string(&mut text)
        .map_err(|_| anyhow::anyhow!("credential file unreadable or not UTF-8"))?;
    if text.len() > 65536 {
        anyhow::bail!("credential file exceeds 64 KiB");
    }
    Ok(text)
}

#[cfg(windows)]
mod windows_private {
    use std::ffi::c_void;
    use std::fs::File;
    use std::mem::{offset_of, size_of, size_of_val};
    use std::os::windows::fs::MetadataExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::ptr::{null_mut, read_unaligned};
    use windows_sys::Win32::Foundation::{GetLastError, LocalFree, ERROR_NO_TOKEN};
    use windows_sys::Win32::Security::Authorization::{GetSecurityInfo, SE_FILE_OBJECT};
    use windows_sys::Win32::Security::{
        EqualSid, GetAce, GetSecurityDescriptorDacl, GetSecurityDescriptorOwner, GetTokenInformation, IsValidAcl,
        IsValidSecurityDescriptor, IsValidSid, TokenUser, ACCESS_ALLOWED_ACE, ACE_HEADER, ACL,
        DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PSID, SECURITY_MAX_SID_SIZE, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::Storage::FileSystem::{GetFileType, FILE_ATTRIBUTE_REPARSE_POINT, FILE_TYPE_DISK};
    use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken,
    };

    const REFUSED: &str = "credential file privacy could not be verified for this Windows user";
    const TOKEN_WORDS: usize = (size_of::<TOKEN_USER>() + SECURITY_MAX_SID_SIZE as usize).div_ceil(size_of::<usize>());

    struct LocalAllocation(*mut c_void);

    impl Drop for LocalAllocation {
        fn drop(&mut self) {
            // SAFETY: allocations held here come from Win32 APIs documented to use LocalFree.
            unsafe { LocalFree(self.0) };
        }
    }

    struct CurrentUser {
        storage: [usize; TOKEN_WORDS],
        sid_offset: usize,
    }

    impl CurrentUser {
        fn query() -> anyhow::Result<Self> {
            let mut token = null_mut();
            // SAFETY: pseudo-handles need no close; token is a valid output slot.
            unsafe {
                if OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) == 0
                    && (GetLastError() != ERROR_NO_TOKEN
                        || OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0)
                {
                    anyhow::bail!(REFUSED);
                }
            }
            // SAFETY: the successful token open transferred one owned handle.
            let token = unsafe { OwnedHandle::from_raw_handle(token) };
            let mut user = Self {
                storage: [0; TOKEN_WORDS],
                sid_offset: 0,
            };
            let mut length = 0;
            // SAFETY: the aligned buffer has room for TOKEN_USER and the maximum Windows SID.
            if unsafe {
                GetTokenInformation(
                    token.as_raw_handle(),
                    TokenUser,
                    user.storage.as_mut_ptr().cast(),
                    size_of_val(&user.storage) as u32,
                    &mut length,
                )
            } == 0
                || (length as usize) < size_of::<TOKEN_USER>()
                || (length as usize) > size_of_val(&user.storage)
            {
                anyhow::bail!(REFUSED);
            }
            // TOKEN_USER contains a pointer into this buffer. Moving the buffer invalidates it,
            // so retain the SID offset instead and reconstruct the pointer when needed.
            let sid = unsafe { (*(user.storage.as_ptr().cast::<TOKEN_USER>())).User.Sid };
            let offset = (sid as usize)
                .checked_sub(user.storage.as_ptr() as usize)
                .ok_or_else(|| anyhow::anyhow!(REFUSED))?;
            if offset < size_of::<TOKEN_USER>() || offset > length as usize {
                anyhow::bail!(REFUSED);
            }
            // SAFETY: the SID pointer is inside the initialized output buffer; bounds are checked below.
            if !unsafe { valid_sid_in(sid, length as usize - offset) } {
                anyhow::bail!(REFUSED);
            }
            user.sid_offset = offset;
            Ok(user)
        }

        fn sid(&self) -> PSID {
            // SAFETY: query verified this offset and SID within the owned buffer.
            unsafe {
                self.storage
                    .as_ptr()
                    .cast::<u8>()
                    .add(self.sid_offset)
                    .cast_mut()
                    .cast()
            }
        }
    }

    unsafe fn valid_sid_in(sid: PSID, available: usize) -> bool {
        if sid.is_null() || available < 8 {
            return false;
        }
        // SAFETY: the caller provides available bytes, including the 8-byte SID header.
        let count = unsafe { *sid.cast::<u8>().add(1) } as usize;
        available >= 8 + count * size_of::<u32>() && unsafe { IsValidSid(sid) } != 0
    }

    pub(super) fn validate_file(file: &File) -> anyhow::Result<()> {
        let metadata = file.metadata().map_err(|_| anyhow::anyhow!(REFUSED))?;
        // SAFETY: File owns a live handle for the full validation and subsequent read.
        if unsafe { GetFileType(file.as_raw_handle()) } != FILE_TYPE_DISK
            || !metadata.is_file()
            || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        {
            anyhow::bail!(REFUSED);
        }
        let user = CurrentUser::query()?;
        let mut descriptor = null_mut();
        // SAFETY: the handle is live; only the owned descriptor output is requested.
        let result = unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                null_mut(),
                null_mut(),
                &mut descriptor,
            )
        };
        let descriptor = LocalAllocation(descriptor);
        if result != 0 {
            anyhow::bail!(REFUSED);
        }
        // SAFETY: GetSecurityInfo returned an allocated descriptor kept alive by the guard.
        unsafe { validate_descriptor(descriptor.0, user.sid()) }
    }

    unsafe fn validate_descriptor(descriptor: *mut c_void, current_user: PSID) -> anyhow::Result<()> {
        // SAFETY: callers supply OS-allocated descriptors and a validated live current-user SID.
        unsafe {
            if descriptor.is_null() || IsValidSecurityDescriptor(descriptor) == 0 {
                anyhow::bail!(REFUSED);
            }
            let mut owner = null_mut();
            let mut defaulted = 0;
            if GetSecurityDescriptorOwner(descriptor, &mut owner, &mut defaulted) == 0
                || owner.is_null()
                || IsValidSid(owner) == 0
                || EqualSid(owner, current_user) == 0
            {
                anyhow::bail!(REFUSED);
            }
            let mut present = 0;
            let mut acl = null_mut();
            if GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) == 0
                || present == 0
                || acl.is_null()
                || IsValidAcl(acl) == 0
            {
                anyhow::bail!(REFUSED);
            }
            let acl_header = read_unaligned(acl);
            for index in 0..u32::from(acl_header.AceCount) {
                let mut ace = null_mut();
                if GetAce(acl, index, &mut ace) == 0 || ace.is_null() {
                    anyhow::bail!(REFUSED);
                }
                let offset = (ace as usize)
                    .checked_sub(acl as usize)
                    .ok_or_else(|| anyhow::anyhow!(REFUSED))?;
                if offset < size_of::<ACL>()
                    || offset > usize::from(acl_header.AclSize)
                    || size_of::<ACE_HEADER>() > usize::from(acl_header.AclSize) - offset
                {
                    anyhow::bail!(REFUSED);
                }
                let header = read_unaligned(ace.cast::<ACE_HEADER>());
                let sid_offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
                let ace_size = usize::from(header.AceSize);
                // Unknown, conditional and object ACEs cannot establish the owner-only contract.
                if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE
                    || ace_size < sid_offset
                    || ace_size > usize::from(acl_header.AclSize) - offset
                {
                    anyhow::bail!(REFUSED);
                }
                let sid = ace.cast::<u8>().add(sid_offset).cast();
                if !valid_sid_in(sid, ace_size - sid_offset) || EqualSid(sid, current_user) == 0 {
                    anyhow::bail!(REFUSED);
                }
            }
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::common::credential_store::{read_private_text, tests::MockStore};
        use crate::server::env_keys;
        use std::collections::BTreeMap;
        use std::io::Read;
        use std::os::windows::ffi::OsStrExt;
        use std::os::windows::fs::{symlink_file, OpenOptionsExt};
        use std::path::{Path, PathBuf};
        use windows_sys::Win32::Security::Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW,
            SDDL_REVISION_1,
        };
        use windows_sys::Win32::Security::{
            ImpersonateSelf, RevertToSelf, SecurityImpersonation, SetFileSecurityW, SetTokenInformation, TokenOwner,
            PROTECTED_DACL_SECURITY_INFORMATION, TOKEN_ADJUST_DEFAULT, TOKEN_OWNER,
        };
        use windows_sys::Win32::Storage::FileSystem::{FILE_READ_ATTRIBUTES, FILE_READ_DATA};

        fn user_sid() -> String {
            let user = CurrentUser::query().unwrap();
            let mut text = null_mut();
            // SAFETY: the SID is valid and text receives a LocalFree allocation.
            assert_ne!(unsafe { ConvertSidToStringSidW(user.sid(), &mut text) }, 0);
            let allocation = LocalAllocation(text.cast());
            let mut length = 0;
            // SAFETY: successful conversion returns a NUL-terminated UTF-16 string.
            unsafe {
                while *text.add(length) != 0 {
                    length += 1;
                }
                let sid = String::from_utf16(std::slice::from_raw_parts(text, length)).unwrap();
                drop(allocation);
                sid
            }
        }

        fn descriptor(sddl: &str) -> LocalAllocation {
            let wide: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
            let mut descriptor = null_mut();
            // SAFETY: wide is NUL-terminated and the output is owned by LocalAllocation.
            assert_ne!(
                unsafe {
                    ConvertStringSecurityDescriptorToSecurityDescriptorW(
                        wide.as_ptr(),
                        SDDL_REVISION_1,
                        &mut descriptor,
                        null_mut(),
                    )
                },
                0,
                "SDDL fixture conversion failed"
            );
            LocalAllocation(descriptor)
        }

        fn set_dacl(path: &Path, sddl: &str) {
            let descriptor = descriptor(&format!("O:{}{sddl}", user_sid()));
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            // SAFETY: path and descriptor remain live for the synchronous API call.
            assert_ne!(
                unsafe {
                    SetFileSecurityW(
                        wide.as_ptr(),
                        OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                        descriptor.0,
                    )
                },
                0,
                "DACL fixture application failed"
            );
        }

        fn private_file(contents: &[u8]) -> (tempfile::TempDir, PathBuf, String) {
            let dir = tempfile::tempdir().unwrap();
            let sid = user_sid();
            set_dacl(dir.path(), &format!("D:P(A;OICI;FA;;;{sid})"));
            let path = dir.path().join("private.env");
            std::fs::write(&path, contents).unwrap();
            set_dacl(&path, &format!("D:P(A;;FA;;;{sid})"));
            (dir, path, sid)
        }

        struct UserOwnedCreation;

        impl UserOwnedCreation {
            fn enter() -> Self {
                // SAFETY: impersonation is confined to this test thread and undone by Drop.
                assert_ne!(unsafe { ImpersonateSelf(SecurityImpersonation) }, 0);
                let guard = Self;
                let mut token = null_mut();
                // SAFETY: this thread now owns an impersonation token; the output slot is valid.
                assert_ne!(
                    unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY | TOKEN_ADJUST_DEFAULT, 1, &mut token) },
                    0
                );
                // SAFETY: OpenThreadToken transferred an owned token handle.
                let token = unsafe { OwnedHandle::from_raw_handle(token) };
                let user = CurrentUser::query().unwrap();
                let owner = TOKEN_OWNER { Owner: user.sid() };
                // Elevated CI can default new objects to Administrators. Model an ordinary user
                // without changing the process token or bypassing the production ownership check.
                assert_ne!(
                    unsafe {
                        SetTokenInformation(
                            token.as_raw_handle(),
                            TokenOwner,
                            (&owner as *const TOKEN_OWNER).cast(),
                            size_of::<TOKEN_OWNER>() as u32,
                        )
                    },
                    0
                );
                guard
            }
        }

        impl Drop for UserOwnedCreation {
            fn drop(&mut self) {
                // SAFETY: only this test thread's impersonation state is reset.
                assert_ne!(unsafe { RevertToSelf() }, 0);
            }
        }

        #[test]
        fn private_regular_file_and_missing_file_are_accepted() {
            let (dir, path, _) = private_file(b"private-value");
            assert_eq!(read_private_text(&path).unwrap(), "private-value");
            assert_eq!(read_private_text(&dir.path().join("missing.env")).unwrap(), "");
        }

        #[test]
        fn grants_to_other_principals_are_refused() {
            for principal in ["WD", "BU", "BA", "SY"] {
                let (_dir, path, sid) = private_file(b"must-not-load");
                set_dacl(&path, &format!("D:P(A;;FA;;;{sid})(A;;GR;;;{principal})"));
                assert!(read_private_text(&path).is_err(), "accepted grant to {principal}");
            }
        }

        #[test]
        fn null_dacl_file_is_refused() {
            let (_dir, path, _) = private_file(b"must-not-load");
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            // SAFETY: a null DACL intentionally makes this temporary fixture unrestricted.
            assert_eq!(
                unsafe {
                    SetNamedSecurityInfoW(
                        wide.as_ptr(),
                        SE_FILE_OBJECT,
                        DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                        null_mut(),
                        null_mut(),
                        null_mut(),
                        null_mut(),
                    )
                },
                0
            );
            assert!(read_private_text(&path).is_err());
        }

        #[test]
        fn empty_dacl_grants_nobody_access_and_cannot_be_read() {
            let user = CurrentUser::query().unwrap();
            let (dir, path, sid) = private_file(b"must-not-load");
            let empty = descriptor(&format!("O:{sid}D:P"));
            // SAFETY: this OS-allocated descriptor and the current-user SID remain live.
            assert!(unsafe { validate_descriptor(empty.0, user.sid()) }.is_ok());
            set_dacl(&path, "D:P");
            assert!(read_private_text(&path).is_err());
            // The owner can restore the DACL so the temporary fixture can be removed.
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let descriptor = descriptor(&format!("D:P(A;;FA;;;{sid})"));
            // SAFETY: restore only the DACL; an empty DACL does not grant WRITE_OWNER.
            assert_ne!(
                unsafe { SetFileSecurityW(wide.as_ptr(), DACL_SECURITY_INFORMATION, descriptor.0) },
                0
            );
            drop(dir);
        }

        #[test]
        fn foreign_owner_missing_dacl_and_unsupported_ace_are_refused() {
            let user = CurrentUser::query().unwrap();
            let sid = user_sid();
            for sddl in [
                format!("O:WDD:P(A;;FA;;;{sid})"),
                format!("O:{sid}"),
                format!("O:{sid}D:P(D;;GW;;;WD)(A;;FA;;;{sid})"),
            ] {
                let descriptor = descriptor(&sddl);
                // SAFETY: the fixture is OS-allocated and both SIDs remain live.
                assert!(unsafe { validate_descriptor(descriptor.0, user.sid()) }.is_err());
            }
            // SAFETY: null is an explicitly rejected input and is never dereferenced.
            assert!(unsafe { validate_descriptor(null_mut(), user.sid()) }.is_err());
        }

        #[test]
        fn invalid_sid_acl_and_unknown_ace_are_refused() {
            let user = CurrentUser::query().unwrap();
            let sid = user_sid();
            for defect in ["owner SID", "ACL", "ACE type"] {
                let descriptor = descriptor(&format!("O:{sid}D:P(A;;FA;;;{sid})"));
                // SAFETY: each mutation stays inside its fresh OS-allocated descriptor.
                unsafe {
                    let mut defaulted = 0;
                    if defect == "owner SID" {
                        let mut owner = null_mut();
                        assert_ne!(GetSecurityDescriptorOwner(descriptor.0, &mut owner, &mut defaulted), 0);
                        *owner.cast::<u8>() = 0;
                    } else {
                        let mut acl = null_mut();
                        let mut present = 0;
                        assert_ne!(
                            GetSecurityDescriptorDacl(descriptor.0, &mut present, &mut acl, &mut defaulted),
                            0
                        );
                        if defect == "ACL" {
                            (*acl).AclRevision = 0;
                        } else {
                            let mut ace = null_mut();
                            assert_ne!(GetAce(acl, 0, &mut ace), 0);
                            (*ace.cast::<ACE_HEADER>()).AceType = u8::MAX;
                        }
                    }
                    assert!(
                        validate_descriptor(descriptor.0, user.sid()).is_err(),
                        "accepted invalid {defect}"
                    );
                }
            }
        }

        #[test]
        fn directory_symlink_and_dangling_symlink_are_refused() {
            let (dir, path, _) = private_file(b"must-not-follow");
            assert!(read_private_text(dir.path()).is_err());
            let link = dir.path().join("link.env");
            symlink_file(&path, &link).expect("Windows test runner must support file symlinks");
            assert!(read_private_text(&link).is_err());
            std::fs::remove_file(&path).unwrap();
            assert!(
                read_private_text(&link).is_err(),
                "dangling reparse point was treated as missing"
            );
        }

        #[test]
        fn readable_handle_without_security_query_access_is_refused() {
            let (_dir, path, _) = private_file(b"readable-but-unverified");
            let mut file = std::fs::OpenOptions::new()
                .access_mode(FILE_READ_DATA | FILE_READ_ATTRIBUTES)
                .open(path)
                .unwrap();
            let mut text = String::new();
            file.read_to_string(&mut text).unwrap();
            assert_eq!(text, "readable-but-unverified");
            assert!(validate_file(&file).is_err());
        }

        #[test]
        fn private_file_preserves_utf8_and_size_limits() {
            let (_dir, path, _) = private_file(&[0xff]);
            assert!(read_private_text(&path).is_err());
            std::fs::write(&path, vec![b'x'; 65536]).unwrap();
            assert_eq!(read_private_text(&path).unwrap().len(), 65536);
            std::fs::write(&path, vec![b'x'; 65537]).unwrap();
            assert!(read_private_text(&path).is_err());
        }

        #[test]
        fn private_legacy_write_opt_in_and_migration_round_trip() {
            let _identity = UserOwnedCreation::enter();
            let (_dir, path, _) = private_file(b"# retain\n");
            let values = BTreeMap::from([("GROK_API_KEY".to_owned(), "fixture-private-provider-key".to_owned())]);
            env_keys::update_keys(&path, &values, &[]).unwrap();
            let original = read_private_text(&path).unwrap();
            let store = MockStore::new();
            let plaintext = env_keys::load_startup(&path, true, &store).unwrap();
            assert_eq!(plaintext.values, values);
            assert!(plaintext.warnings.is_empty());
            assert!(store.values.lock().unwrap().is_empty());
            assert_eq!(read_private_text(&path).unwrap(), original);
            let migrated = env_keys::load_startup(&path, false, &store).unwrap();
            assert_eq!(migrated.values, values);
            assert!(migrated.warnings.is_empty());
            assert_eq!(read_private_text(&path).unwrap(), "# retain\n");
            assert_eq!(*store.values.lock().unwrap(), values);
            let reloaded = env_keys::load_startup(&path, false, &store).unwrap();
            assert_eq!(reloaded.values, values);
            assert!(reloaded.warnings.is_empty());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    pub(super) struct MockStore {
        pub(super) values: Mutex<BTreeMap<String, String>>,
        fail_write: Mutex<BTreeSet<String>>,
        mismatch: Mutex<BTreeSet<String>>,
        unavailable: Mutex<bool>,
    }

    impl MockStore {
        pub(super) fn new() -> Self {
            Self {
                values: Mutex::new(BTreeMap::new()),
                fail_write: Mutex::new(BTreeSet::new()),
                mismatch: Mutex::new(BTreeSet::new()),
                unavailable: Mutex::new(false),
            }
        }
    }

    impl CredentialStore for MockStore {
        fn get(&self, name: &str) -> Result<Option<String>, StoreError> {
            if *self.unavailable.lock().unwrap() {
                return Err(StoreError::Unavailable);
            }
            if self.mismatch.lock().unwrap().contains(name) {
                return Ok(Some(format!("mismatch-{name}")));
            }
            Ok(self.values.lock().unwrap().get(name).cloned())
        }

        fn set(&self, name: &str, value: &str) -> Result<(), StoreError> {
            if *self.unavailable.lock().unwrap() {
                return Err(StoreError::Unavailable);
            }
            if self.fail_write.lock().unwrap().contains(name) {
                return Err(StoreError::WriteFailed);
            }
            self.values.lock().unwrap().insert(name.to_string(), value.to_string());
            Ok(())
        }

        fn delete(&self, name: &str) -> Result<(), StoreError> {
            self.values.lock().unwrap().remove(name);
            Ok(())
        }
    }

    fn sample() -> (String, BTreeMap<String, String>) {
        let text = "\
# keep me\n\
export OTHER='leave'\n\
export GROK_API_KEY='grok-secret-value-1111'\n\
export ANTHROPIC_API_KEY='anthropic-secret-value-2222'\n";
        let mut assignments = BTreeMap::new();
        assignments.insert("GROK_API_KEY".into(), "grok-secret-value-1111".into());
        assignments.insert("ANTHROPIC_API_KEY".into(), "anthropic-secret-value-2222".into());
        (text.to_string(), assignments)
    }

    #[test]
    fn migration_removes_only_verified_keys_and_keeps_unknown_lines() {
        let (text, assignments) = sample();
        let store = MockStore::new();
        let report = migrate_text(&text, &assignments, &store);
        assert_eq!(report.failed, Vec::new());
        assert_eq!(
            report.migrated,
            vec!["ANTHROPIC_API_KEY".to_string(), "GROK_API_KEY".to_string()]
        );
        assert!(report.text.contains("# keep me"));
        assert!(report.text.contains("export OTHER='leave'"));
        assert!(!report.text.contains("grok-secret-value-1111"));
        assert!(!report.text.contains("anthropic-secret-value-2222"));
        assert_eq!(
            store.values.lock().unwrap().get("GROK_API_KEY").map(String::as_str),
            Some("grok-secret-value-1111")
        );
        let warning = migration_warning("GROK_API_KEY", StoreError::WriteFailed);
        assert!(!warning.contains("grok-secret-value-1111"));
    }

    #[test]
    fn migration_keeps_the_line_when_the_write_fails() {
        let (text, assignments) = sample();
        let store = MockStore::new();
        store.fail_write.lock().unwrap().insert("GROK_API_KEY".into());
        store.fail_write.lock().unwrap().insert("ANTHROPIC_API_KEY".into());
        let report = migrate_text(&text, &assignments, &store);
        assert!(report.text.contains("grok-secret-value-1111"));
        assert!(report.text.contains("anthropic-secret-value-2222"));
        assert!(store.values.lock().unwrap().is_empty());
        assert_eq!(report.failed.len(), 2);
        assert!(report.failed.iter().all(|(_, error)| *error == StoreError::WriteFailed));
    }

    #[test]
    fn migration_keeps_the_line_when_read_back_mismatches() {
        let (text, assignments) = sample();
        let store = MockStore::new();
        store.mismatch.lock().unwrap().insert("GROK_API_KEY".into());
        store.mismatch.lock().unwrap().insert("ANTHROPIC_API_KEY".into());
        let report = migrate_text(&text, &assignments, &store);
        assert!(report.text.contains("grok-secret-value-1111"));
        assert!(store.values.lock().unwrap().get("GROK_API_KEY").is_none());
        assert!(report
            .failed
            .iter()
            .all(|(_, error)| *error == StoreError::VerifyMismatch));
        let warning = migration_warning("GROK_API_KEY", StoreError::VerifyMismatch);
        assert!(!warning.contains("grok-secret-value-1111"));
        assert!(!warning.contains("mismatch-GROK_API_KEY"));
    }

    #[test]
    fn migration_is_partial_when_only_one_key_verifies() {
        let (text, assignments) = sample();
        let store = MockStore::new();
        store.fail_write.lock().unwrap().insert("ANTHROPIC_API_KEY".into());
        let report = migrate_text(&text, &assignments, &store);
        assert_eq!(report.migrated, vec!["GROK_API_KEY".to_string()]);
        assert!(!report.text.contains("grok-secret-value-1111"));
        assert!(report.text.contains("anthropic-secret-value-2222"));
        assert!(report.text.contains("export OTHER='leave'"));
        assert_eq!(
            store.values.lock().unwrap().get("GROK_API_KEY").map(String::as_str),
            Some("grok-secret-value-1111")
        );
        assert!(store.values.lock().unwrap().get("ANTHROPIC_API_KEY").is_none());
        assert_eq!(
            report.failed,
            vec![("ANTHROPIC_API_KEY".into(), StoreError::WriteFailed)]
        );
    }

    #[test]
    fn migration_leaves_every_line_when_the_store_is_unavailable() {
        let (text, assignments) = sample();
        let store = MockStore::new();
        *store.unavailable.lock().unwrap() = true;
        let report = migrate_text(&text, &assignments, &store);
        assert_eq!(report.text, text);
        assert!(report.migrated.is_empty());
        assert!(report.failed.iter().all(|(_, error)| *error == StoreError::Unavailable));
        let warning = migration_warning("SERPAPI_API_KEY", StoreError::Unavailable);
        assert!(!warning.contains("secret"));
        assert!(warning.contains(PLAINTEXT_KEY_FILE));
    }

    #[test]
    fn credential_file_parent_components_are_refused_without_echoing_the_path() {
        let err = read_private_text(Path::new("/tmp/not-a-home/../.env")).unwrap_err();
        let message = err.to_string();
        assert_eq!(message, "credential file path refused");
        assert!(!message.contains(".env"));
    }

    #[cfg(unix)]
    #[test]
    fn private_unix_file_preserves_mode_symlink_and_hardlink_checks() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private.env");
        std::fs::write(&path, "private-value").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_private_text(&path).unwrap(), "private-value");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read_private_text(&path).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join("symlink.env");
        symlink(&path, &link).unwrap();
        assert!(read_private_text(&link).is_err());
        std::fs::hard_link(&path, dir.path().join("hardlink.env")).unwrap();
        assert!(read_private_text(&path).is_err());
    }

    #[test]
    fn dotted_filename_is_not_a_parent_segment() {
        let path = std::env::temp_dir().join(format!("cgagentharness-foo..bar-{}.env", std::process::id()));
        assert_eq!(read_private_text(&path).unwrap(), "");
    }

    #[test]
    fn backslash_parent_segment_is_refused_without_echoing_the_path() {
        let err = read_private_text(Path::new(r"safe\..\secret")).unwrap_err();
        let message = err.to_string();
        assert_eq!(message, "credential file path refused");
        assert!(!message.contains("secret"));
    }

    #[test]
    fn service_name_isolates_homes_without_using_a_platform_target() {
        let home = r"C:\Users\operator\.CGagentHarness";
        let name = service_name(home);
        assert!(name.starts_with(SERVICE));
        assert!(name.contains("C:/Users/operator/.CGagentHarness"));
        assert!(!name.contains('\\'));
        assert_ne!(name, service_name("/tmp/other-home"));
        // Building the entry does not contact Keychain, Credential Manager, or D-Bus.
        assert!(keyring::Entry::new(&name, "GROK_API_KEY").is_ok());
        #[cfg(target_os = "macos")]
        assert!(
            keyring::Entry::new_with_target(home, SERVICE, "GROK_API_KEY").is_err(),
            "a filesystem path is not a macOS keychain domain"
        );
    }

    #[test]
    fn os_store_round_trip_runs_only_when_requested() {
        if std::env::var("CGAGENTHARNESS_KEYRING_TEST").ok().as_deref() != Some("1") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let store = OsCredentialStore::for_home(dir.path());
        let name = "CGAGENTHARNESS_KEYRING_TEST_KEY";
        let value = "os-store-round-trip-value";
        store.delete(name).unwrap();
        store.set(name, value).unwrap();
        let got = store.get(name).unwrap().unwrap();
        assert!(secret_eq(&got, value));
        store.delete(name).unwrap();
        assert!(store.get(name).unwrap().is_none());
    }
}
