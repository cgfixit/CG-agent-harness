#![cfg(windows)]

#[path = "../src/common/credential_file_writer.rs"]
mod writer_stage_probe;

use std::collections::BTreeMap;
use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::mem::{offset_of, size_of, size_of_val};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use cgagentharness::common::atomic::write_atomic;
use cgagentharness::common::credential_store::{CredentialStore, StoreError};
use cgagentharness::server::env_keys::{load_startup, update_keys};
use windows_sys::Win32::Foundation::{
    LocalFree, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_TOKEN, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken, SetThreadToken,
};

struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: these allocations come only from APIs documented to use LocalFree.
        unsafe { LocalFree(self.0) };
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn descriptor(sddl: &str) -> LocalAllocation {
    let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let mut raw = null_mut();
    // SAFETY: text is terminated and raw is a writable descriptor out-pointer.
    assert_ne!(
        unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(text.as_ptr(), 1, &mut raw, null_mut()) },
        0
    );
    LocalAllocation(raw)
}

fn sid_text(sid: PSID) -> String {
    let mut raw = null_mut();
    // SAFETY: callers pass a SID from a live Windows-owned security/token structure.
    assert_ne!(unsafe { ConvertSidToStringSidW(sid, &mut raw) }, 0);
    let allocation = LocalAllocation(raw.cast());
    let mut length = 0;
    // SAFETY: the successful API returns a terminated UTF-16 allocation.
    unsafe {
        while *raw.add(length) != 0 {
            length += 1;
        }
        let result = String::from_utf16(std::slice::from_raw_parts(raw, length)).unwrap();
        drop(allocation);
        result
    }
}

fn effective_token() -> OwnedHandle {
    let mut token = null_mut();
    // SAFETY: pseudo handle and writable out-pointer satisfy the token API.
    if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut token) } == 0 {
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(ERROR_NO_TOKEN as i32)
        );
        // SAFETY: no thread token exists, so use the process token.
        assert_ne!(
            unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) },
            0
        );
    }
    // SAFETY: successful token APIs transfer an owned kernel handle.
    unsafe { OwnedHandle::from_raw_handle(token) }
}

fn token_information(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Vec<usize> {
    let mut length = 0;
    // SAFETY: the first call only queries the required buffer length.
    assert_eq!(
        unsafe { GetTokenInformation(token, class, null_mut(), 0, &mut length) },
        0
    );
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(ERROR_INSUFFICIENT_BUFFER as i32)
    );
    assert!(length > 0 && length <= 1024 * 1024);
    let mut buffer = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
    // SAFETY: usize storage has adequate pointer alignment and at least length bytes.
    assert_ne!(
        unsafe { GetTokenInformation(token, class, buffer.as_mut_ptr().cast(), length, &mut length) },
        0
    );
    buffer
}

fn user_sid() -> String {
    let token = effective_token();
    let info = token_information(token.as_raw_handle(), TokenUser);
    assert!(size_of_val(info.as_slice()) >= size_of::<TOKEN_USER>());
    // SAFETY: TokenUser returned a complete, aligned TOKEN_USER and backing SID.
    sid_text(unsafe { (*(info.as_ptr().cast::<TOKEN_USER>())).User.Sid })
}

#[derive(Debug)]
struct Security {
    owner: String,
    protected: bool,
    aces: Vec<(u8, u8, u32, String)>,
}

fn inspect(path: &Path) -> Security {
    let file = File::open(path).unwrap();
    let mut owner = null_mut();
    let mut acl = null_mut();
    let mut sd = null_mut();
    // SAFETY: the file handle is live and all result pointers are writable.
    assert_eq!(
        unsafe {
            GetSecurityInfo(
                file.as_raw_handle(),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut acl,
                null_mut(),
                &mut sd,
            )
        },
        0
    );
    let _allocation = LocalAllocation(sd);
    assert!(!owner.is_null() && !acl.is_null());
    let mut control = 0;
    let mut revision = 0;
    // SAFETY: sd remains alive through _allocation.
    assert_ne!(
        unsafe { GetSecurityDescriptorControl(sd, &mut control, &mut revision) },
        0
    );
    let mut aces = Vec::new();
    // SAFETY: Windows returned a valid ACL; GetAce validates each index.
    for index in 0..unsafe { (*acl).AceCount } {
        let mut raw = null_mut();
        assert_ne!(unsafe { GetAce(acl, index as u32, &mut raw) }, 0);
        let header = unsafe { &*raw.cast::<ACE_HEADER>() };
        assert_eq!(header.AceType, 0, "unexpected ACE in {path:?}");
        let sid_offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
        assert!(header.AceSize as usize >= sid_offset + 8);
        let sid = unsafe { raw.cast::<u8>().add(sid_offset).cast() };
        assert_ne!(unsafe { IsValidSid(sid) }, 0);
        assert!(unsafe { GetLengthSid(sid) } as usize <= header.AceSize as usize - sid_offset);
        let mask = unsafe { (*raw.cast::<ACCESS_ALLOWED_ACE>()).Mask };
        aces.push((header.AceType, header.AceFlags, mask, sid_text(sid)));
    }
    Security {
        owner: sid_text(owner),
        protected: control & SE_DACL_PROTECTED != 0,
        aces,
    }
}

fn assert_private(path: &Path, user: &str) {
    let security = inspect(path);
    assert_eq!(security.owner, user, "{security:?}");
    assert!(security.protected, "{security:?}");
    assert_eq!(
        security.aces,
        vec![(0, 0, FILE_ALL_ACCESS, user.to_owned())],
        "{security:?}"
    );
    let metadata = std::fs::metadata(path).unwrap();
    assert!(metadata.is_file());
    use std::os::windows::fs::MetadataExt;
    assert_eq!(metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT, 0);
}

fn broad_parent() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let sd = descriptor(&format!("D:P(A;OICI;FA;;;{})(A;OICI;GR;;;WD)", user_sid()));
    // SAFETY: the directory exists and the descriptor remains live for the call.
    assert_ne!(
        unsafe {
            SetFileSecurityW(
                wide(dir.path()).as_ptr(),
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                sd.0,
            )
        },
        0
    );
    dir
}

fn private_fixture(path: &Path, bytes: &[u8]) {
    let user = user_sid();
    let sd = descriptor(&format!("O:{user}D:P(A;;FA;;;{user})"));
    let security = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd.0,
        bInheritHandle: 0,
    };
    // SAFETY: path is terminated and security points to a live descriptor.
    let raw = unsafe {
        CreateFileW(
            wide(path).as_ptr(),
            FILE_GENERIC_READ | FILE_GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            &security,
            CREATE_NEW,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    assert_ne!(raw, INVALID_HANDLE_VALUE);
    // SAFETY: CreateFileW transferred an owned file handle.
    let mut file = unsafe { File::from_raw_handle(raw) };
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
    drop(file);
    assert_private(path, &user);
}

fn updates(value: &str) -> BTreeMap<String, String> {
    BTreeMap::from([("DEEPAGENT_API_KEY".into(), value.into())])
}

fn no_staged_files(parent: &Path) {
    let leftovers: Vec<_> = std::fs::read_dir(parent)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .filter(|name| name.to_string_lossy().starts_with(".staged."))
        .collect();
    assert!(leftovers.is_empty(), "staged files survived: {leftovers:?}");
}

#[test]
fn legacy_control_inherits_foreign_access_but_real_save_is_private() {
    let dir = broad_parent();
    let control = dir.path().join("legacy-control");
    write_atomic(&control, b"synthetic-control", Some(0o600)).unwrap();
    let security = inspect(&control);
    assert!(
        security
            .aces
            .iter()
            .any(|(_, flags, _, sid)| sid == "S-1-1-0" && u32::from(*flags) & INHERITED_ACE != 0),
        "{security:?}"
    );
    let path = dir.path().join(".env");
    update_keys(&path, &updates("synthetic-save"), &[]).unwrap();
    assert_private(&path, &user_sid());
    assert!(std::fs::read_to_string(&path).unwrap().contains("synthetic-save"));
    let store = MockStore::default();
    let loaded = load_startup(&path, true, &store).unwrap();
    assert_eq!(
        loaded.values.get("DEEPAGENT_API_KEY").map(String::as_str),
        Some("synthetic-save")
    );
    assert_eq!(store.1.load(Ordering::Relaxed), 0);
    no_staged_files(dir.path());
}

#[test]
fn production_writer_source_publishes_private_bytes() {
    let dir = broad_parent();
    let path = dir.path().join(".env");
    writer_stage_probe::write(&path, b"synthetic-probe").unwrap();
    assert_private(&path, &user_sid());
    assert_eq!(std::fs::read(&path).unwrap(), b"synthetic-probe");
    no_staged_files(dir.path());
}

#[test]
fn canonical_unicode_parent_supports_private_publication() {
    let dir = broad_parent();
    let parent = dir.path().join("keys-ø-汉字-🔑");
    std::fs::create_dir(&parent).unwrap();
    let parent = std::fs::canonicalize(parent).unwrap();
    let path = parent.join(".env");
    update_keys(&path, &updates("unicode-path-value"), &[]).unwrap();
    assert_private(&path, &user_sid());
    assert!(std::fs::read_to_string(&path).unwrap().contains("unicode-path-value"));
    no_staged_files(&parent);
}

#[test]
fn replacement_keeps_unknown_lines_and_private_security() {
    let dir = broad_parent();
    let path = dir.path().join(".env");
    private_fixture(
        &path,
        b"# retained\nexport OTHER='untouched'\nexport DEEPAGENT_API_KEY='old-value'\n",
    );
    update_keys(&path, &updates("replacement-value"), &[]).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("# retained\nexport OTHER='untouched'\n"));
    assert!(text.contains("replacement-value"));
    assert!(!text.contains("old-value"));
    assert_private(&path, &user_sid());
    no_staged_files(dir.path());
}

#[derive(Default)]
struct MockStore(Mutex<BTreeMap<String, String>>, AtomicUsize);

impl CredentialStore for MockStore {
    fn get(&self, name: &str) -> Result<Option<String>, StoreError> {
        self.1.fetch_add(1, Ordering::Relaxed);
        Ok(self.0.lock().unwrap().get(name).cloned())
    }
    fn set(&self, name: &str, value: &str) -> Result<(), StoreError> {
        self.1.fetch_add(1, Ordering::Relaxed);
        if name == "GROK_API_KEY" {
            return Err(StoreError::WriteFailed);
        }
        self.0.lock().unwrap().insert(name.into(), value.into());
        Ok(())
    }
    fn delete(&self, name: &str) -> Result<(), StoreError> {
        self.1.fetch_add(1, Ordering::Relaxed);
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}

#[test]
fn migration_scrubs_only_verified_keys_and_keeps_private_security() {
    let dir = broad_parent();
    let path = dir.path().join(".env");
    private_fixture(&path, b"# retained\nexport OTHER='untouched'\nexport DEEPAGENT_API_KEY='verified-value'\nexport GROK_API_KEY='failed-value'\n");
    let store = MockStore::default();
    let loaded = load_startup(&path, false, &store).unwrap();
    assert_eq!(
        loaded.values.get("DEEPAGENT_API_KEY").map(String::as_str),
        Some("verified-value")
    );
    assert!(!loaded.values.contains_key("GROK_API_KEY"));
    assert!(!loaded.warnings.is_empty());
    assert!(loaded
        .warnings
        .iter()
        .all(|warning| !warning.contains("failed-value") && !warning.contains("verified-value")));
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("# retained\nexport OTHER='untouched'\n"));
    assert!(text.contains("failed-value"));
    assert!(!text.contains("verified-value"));
    assert_private(&path, &user_sid());
    no_staged_files(dir.path());
}

#[test]
fn locked_target_preserves_old_bytes_and_cleans_stage() {
    let dir = broad_parent();
    let path = dir.path().join(".env");
    let prior = b"export DEEPAGENT_API_KEY='old-value'\n";
    private_fixture(&path, prior);
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .unwrap();
    let error = update_keys(&path, &updates("must-not-publish"), &[]).unwrap_err();
    assert!(!error.to_string().contains("must-not-publish"));
    drop(lock);
    assert_eq!(std::fs::read(&path).unwrap(), prior);
    assert_private(&path, &user_sid());
    no_staged_files(dir.path());
}

#[test]
fn failed_migration_cleanup_keeps_original_file_and_redacted_warning() {
    let dir = broad_parent();
    let path = dir.path().join(".env");
    let prior = b"export DEEPAGENT_API_KEY='verified-value'\n";
    private_fixture(&path, prior);
    let lock = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .open(&path)
        .unwrap();
    let store = MockStore::default();
    let loaded = load_startup(&path, false, &store).unwrap();
    assert_eq!(
        loaded.values.get("DEEPAGENT_API_KEY").map(String::as_str),
        Some("verified-value")
    );
    assert!(loaded
        .warnings
        .iter()
        .any(|warning| warning.contains("could not be removed")));
    assert!(loaded
        .warnings
        .iter()
        .all(|warning| !warning.contains("verified-value")));
    drop(lock);
    assert_eq!(std::fs::read(&path).unwrap(), prior);
    assert_private(&path, &user_sid());
    no_staged_files(dir.path());
}

#[test]
fn directory_target_is_preserved_without_staging() {
    let dir = broad_parent();
    let path = dir.path().join(".env");
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("sentinel"), b"unchanged").unwrap();
    assert!(update_keys(&path, &updates("must-not-publish"), &[]).is_err());
    assert_eq!(std::fs::read(path.join("sentinel")).unwrap(), b"unchanged");
    no_staged_files(dir.path());
}

#[test]
fn rendered_output_accepts_64_kib_and_refuses_one_byte_more() {
    let dir = broad_parent();
    let path = dir.path().join(".env");
    let assignment = "export DEEPAGENT_API_KEY='x'\n";
    let prior = format!("{}\n", "#".repeat(65536 - assignment.len() - 1));
    private_fixture(&path, prior.as_bytes());
    update_keys(&path, &updates("x"), &[]).unwrap();
    let at_limit = std::fs::read(&path).unwrap();
    assert_eq!(at_limit.len(), 65536);
    assert_private(&path, &user_sid());
    assert!(update_keys(&path, &updates("xx"), &[]).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), at_limit);
    no_staged_files(dir.path());
}

struct Impersonation;
impl Drop for Impersonation {
    fn drop(&mut self) {
        // SAFETY: this guard is scoped to the thread that installed the token.
        assert_ne!(unsafe { RevertToSelf() }, 0);
    }
}

#[test]
fn impersonated_user_is_explicit_owner_even_with_foreign_default_owner() {
    let user = user_sid();
    let mut raw = null_mut();
    // SAFETY: process pseudo handle and writable token out-pointer are valid.
    assert_ne!(
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE | TOKEN_QUERY, &mut raw) },
        0
    );
    let process = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut duplicate = null_mut();
    // SAFETY: the source token is alive and duplicate is an output handle.
    assert_ne!(
        unsafe {
            DuplicateTokenEx(
                process.as_raw_handle(),
                TOKEN_ALL_ACCESS,
                null(),
                SecurityImpersonation,
                TokenImpersonation,
                &mut duplicate,
            )
        },
        0
    );
    let duplicate = unsafe { OwnedHandle::from_raw_handle(duplicate) };
    let sd = descriptor("O:BA");
    let mut administrators = null_mut();
    let mut defaulted = 0;
    assert_ne!(
        unsafe { GetSecurityDescriptorOwner(sd.0, &mut administrators, &mut defaulted) },
        0
    );
    let owner = TOKEN_OWNER { Owner: administrators };
    // Administrators can be an owner only when the runner token contains that group.
    if unsafe {
        SetTokenInformation(
            duplicate.as_raw_handle(),
            TokenOwner,
            (&owner as *const TOKEN_OWNER).cast(),
            size_of::<TOKEN_OWNER>() as u32,
        )
    } == 0
    {
        let error = std::io::Error::last_os_error();
        assert_eq!(
            error.raw_os_error(),
            Some(1307),
            "unexpected TokenOwner failure: {error}"
        );
        assert!(
            std::env::var_os("CGAH_REQUIRE_WINDOWS_CREDENTIALS").is_none(),
            "native CI requires a token with a foreign default owner"
        );
        eprintln!("foreign TokenOwner subcase unavailable: Administrators is not an eligible token owner");
        return;
    }
    let owner_info = token_information(duplicate.as_raw_handle(), TokenOwner);
    assert!(size_of_val(owner_info.as_slice()) >= size_of::<TOKEN_OWNER>());
    let foreign = sid_text(unsafe { (*owner_info.as_ptr().cast::<TOKEN_OWNER>()).Owner });
    assert_ne!(foreign, user);
    assert_ne!(unsafe { SetThreadToken(null(), duplicate.as_raw_handle()) }, 0);
    let _impersonation = Impersonation;
    assert_eq!(user_sid(), user);
    let dir = broad_parent();
    let path = dir.path().join(".env");
    update_keys(&path, &updates("impersonated-value"), &[]).unwrap();
    assert_private(&path, &user);
    no_staged_files(dir.path());
}
