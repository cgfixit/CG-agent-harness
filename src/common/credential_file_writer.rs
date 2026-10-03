//! Windows credential staging keeps creation, verification and publication on one handle.

use std::ffi::c_void;
use std::fs::File;
use std::io::{self, Write};
use std::mem::{offset_of, size_of};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::{
    LocalFree, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_TOKEN, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SE_FILE_OBJECT,
};
use windows_sys::Win32::Security::*;
use windows_sys::Win32::Storage::FileSystem::*;
use windows_sys::Win32::System::SystemServices::ACCESS_ALLOWED_ACE_TYPE;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread, OpenProcessToken, OpenThreadToken};

fn refused() -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, "private credential file write refused")
}

struct LocalAllocation(*mut c_void);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: each allocation is returned by an API requiring LocalFree.
        unsafe { LocalFree(self.0) };
    }
}

struct EffectiveUser {
    storage: Vec<usize>,
    sid_offset: usize,
}

impl EffectiveUser {
    fn query() -> io::Result<Self> {
        let mut raw = null_mut();
        // SAFETY: pseudo handle and writable out-pointer satisfy the token API.
        if unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, 1, &mut raw) } == 0 {
            if io::Error::last_os_error().raw_os_error() != Some(ERROR_NO_TOKEN as i32) {
                return Err(refused());
            }
            // SAFETY: fallback is allowed only when there is no effective thread token.
            if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut raw) } == 0 {
                return Err(refused());
            }
        }
        // SAFETY: the successful token API transfers an owned handle.
        let token = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut length = 0;
        // SAFETY: this call queries the required allocation size without reading a buffer.
        if unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut length) } != 0
            || io::Error::last_os_error().raw_os_error() != Some(ERROR_INSUFFICIENT_BUFFER as i32)
            || !(size_of::<TOKEN_USER>()..=1024 * 1024).contains(&(length as usize))
        {
            return Err(refused());
        }
        let capacity = length;
        let mut storage = vec![0usize; (capacity as usize).div_ceil(size_of::<usize>())];
        // SAFETY: the aligned allocation has at least capacity bytes.
        if unsafe {
            GetTokenInformation(
                token.as_raw_handle(),
                TokenUser,
                storage.as_mut_ptr().cast(),
                capacity,
                &mut length,
            )
        } == 0
            || length > capacity
            || (length as usize) < size_of::<TOKEN_USER>()
        {
            return Err(refused());
        }
        // SAFETY: the successful call supplied a complete aligned TOKEN_USER.
        let sid = unsafe { (*storage.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        let base = storage.as_ptr() as usize;
        let sid_offset = (sid as usize).checked_sub(base).ok_or_else(refused)?;
        if sid_offset % 4 != 0 || sid_offset.checked_add(8).is_none_or(|end| end > length as usize) {
            return Err(refused());
        }
        // SAFETY: the SID header lies inside the returned buffer. Bound the variable tail first.
        let subauthorities = unsafe { *sid.cast::<u8>().add(1) } as usize;
        let sid_length = 8 + subauthorities * 4;
        if sid_offset.checked_add(sid_length).is_none_or(|end| end > length as usize)
            // SAFETY: the complete variable SID is now bounded by the allocation.
            || unsafe { IsValidSid(sid) } == 0
        {
            return Err(refused());
        }
        Ok(Self { storage, sid_offset })
    }

    fn sid(&self) -> PSID {
        // SAFETY: query verified the offset and the allocation is retained by self.
        unsafe {
            self.storage
                .as_ptr()
                .cast::<u8>()
                .add(self.sid_offset)
                .cast_mut()
                .cast()
        }
    }

    fn descriptor(&self) -> io::Result<LocalAllocation> {
        let mut raw = null_mut();
        // SAFETY: self owns a validated SID and raw is writable.
        if unsafe { ConvertSidToStringSidW(self.sid(), &mut raw) } == 0 {
            return Err(refused());
        }
        let _text = LocalAllocation(raw.cast());
        let mut length = 0;
        // SAFETY: the API returns a terminated UTF-16 SID string.
        unsafe {
            while *raw.add(length) != 0 {
                length += 1;
            }
        }
        // SAFETY: length was measured inside the API-owned terminated string.
        let sid = String::from_utf16(unsafe { std::slice::from_raw_parts(raw, length) }).map_err(|_| refused())?;
        security_descriptor(&format!("O:{sid}D:P(A;;FA;;;{sid})"))
    }
}

fn security_descriptor(sddl: &str) -> io::Result<LocalAllocation> {
    let text: Vec<u16> = sddl.encode_utf16().chain(Some(0)).collect();
    let mut raw = null_mut();
    // SAFETY: text is terminated and raw receives a LocalFree-owned descriptor.
    if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(text.as_ptr(), 1, &mut raw, null_mut()) } == 0 {
        return Err(refused());
    }
    Ok(LocalAllocation(raw))
}

struct PrivateStage {
    file: File,
    published: bool,
}

impl PrivateStage {
    fn create(parent: &Path, descriptor: &LocalAllocation) -> io::Result<Self> {
        let named = tempfile::Builder::new()
            .prefix(".staged.")
            .suffix(".tmp")
            .make_in(parent, |path| {
                let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
                let attributes = SECURITY_ATTRIBUTES {
                    nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                    lpSecurityDescriptor: descriptor.0,
                    bInheritHandle: 0,
                };
                // SAFETY: the path and descriptor remain valid for the call. CREATE_NEW never opens a collision.
                let handle = unsafe {
                    CreateFileW(
                        path.as_ptr(),
                        GENERIC_READ | GENERIC_WRITE | READ_CONTROL | DELETE,
                        0,
                        &attributes,
                        CREATE_NEW,
                        FILE_ATTRIBUTE_NORMAL,
                        null_mut(),
                    )
                };
                if handle == INVALID_HANDLE_VALUE {
                    return Err(io::Error::last_os_error());
                }
                // SAFETY: CreateFileW returned a uniquely owned live file handle.
                Ok(unsafe { File::from_raw_handle(handle) })
            })?;
        // No fallible work may precede disarming pathname cleanup. Only the held object is ours to delete.
        let (file, mut path) = named.into_parts();
        path.disable_cleanup(true);
        drop(path);
        Ok(Self { file, published: false })
    }

    fn validate_empty(&self, user: &EffectiveUser) -> io::Result<()> {
        let handle = self.file.as_raw_handle();
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // SAFETY: the handle is live and info has the API's required writable size.
        if unsafe { GetFileType(handle) } != FILE_TYPE_DISK
            || unsafe { GetFileInformationByHandle(handle, info.as_mut_ptr()) } == 0
        {
            return Err(refused());
        }
        // SAFETY: successful GetFileInformationByHandle initialized info.
        let info = unsafe { info.assume_init() };
        if info.dwFileAttributes & (FILE_ATTRIBUTE_DIRECTORY | FILE_ATTRIBUTE_REPARSE_POINT) != 0
            || info.nNumberOfLinks != 1
            || info.nFileSizeHigh != 0
            || info.nFileSizeLow != 0
        {
            return Err(refused());
        }
        let mut owner = null_mut();
        let mut dacl = null_mut();
        let mut raw = null_mut();
        // SAFETY: all out-pointers are writable and the file handle remains open.
        let status = unsafe {
            GetSecurityInfo(
                handle,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut raw,
            )
        };
        if status != 0 {
            return Err(refused());
        }
        let _descriptor = LocalAllocation(raw);
        let mut control = 0;
        let mut revision = 0;
        // SAFETY: successful GetSecurityInfo supplies a self-contained security descriptor.
        unsafe {
            if raw.is_null()
                || IsValidSecurityDescriptor(raw) == 0
                || owner.is_null()
                || IsValidSid(owner) == 0
                || EqualSid(owner, user.sid()) == 0
                || GetSecurityDescriptorControl(raw, &mut control, &mut revision) == 0
                || control & SE_DACL_PRESENT == 0
                || control & SE_DACL_PROTECTED == 0
                || dacl.is_null()
                || IsValidAcl(dacl) == 0
                || (*dacl).AceCount != 1
            {
                return Err(refused());
            }
            let mut ace = null_mut();
            if GetAce(dacl, 0, &mut ace) == 0 || ace.is_null() {
                return Err(refused());
            }
            let header = &*ace.cast::<ACE_HEADER>();
            let offset = offset_of!(ACCESS_ALLOWED_ACE, SidStart);
            // Reject every other ACE layout before casting it.
            if header.AceType != ACCESS_ALLOWED_ACE_TYPE as u8
                || header.AceFlags != 0
                || (header.AceSize as usize) < offset + 8
            {
                return Err(refused());
            }
            let sid: PSID = ace.cast::<u8>().add(offset).cast();
            let sid_length = 8 + (*sid.cast::<u8>().add(1) as usize) * 4;
            if offset + sid_length != header.AceSize as usize
                || IsValidSid(sid) == 0
                || EqualSid(sid, user.sid()) == 0
                || (*ace.cast::<ACCESS_ALLOWED_ACE>()).Mask != FILE_ALL_ACCESS
            {
                return Err(refused());
            }
        }
        Ok(())
    }

    fn publish(mut self, target: &Path, bytes: &[u8]) -> io::Result<()> {
        let name: Vec<u16> = target.as_os_str().encode_wide().collect();
        if !target.is_absolute() || name.contains(&0) {
            return Err(refused());
        }
        let name_bytes = name.len().checked_mul(size_of::<u16>()).ok_or_else(refused)?;
        let length = offset_of!(FILE_RENAME_INFO, FileName)
            .checked_add(name_bytes)
            .ok_or_else(refused)?;
        let length_u32 = u32::try_from(length).map_err(|_| refused())?;
        let mut buffer = vec![0usize; length.max(size_of::<FILE_RENAME_INFO>()).div_ceil(size_of::<usize>())];
        let rename = buffer.as_mut_ptr().cast::<FILE_RENAME_INFO>();
        // SAFETY: aligned storage covers the fixed header and complete variable UTF-16 name.
        unsafe {
            (*rename).Anonymous.ReplaceIfExists = 1;
            (*rename).RootDirectory = null_mut();
            (*rename).FileNameLength = u32::try_from(name_bytes).map_err(|_| refused())?;
            std::ptr::copy_nonoverlapping(
                name.as_ptr(),
                std::ptr::addr_of_mut!((*rename).FileName).cast(),
                name.len(),
            );
        }
        self.file.write_all(bytes)?;
        self.file.flush()?;
        self.file.sync_all()?;
        // SAFETY: rename storage remains live and DELETE access was requested at creation.
        if unsafe { SetFileInformationByHandle(self.file.as_raw_handle(), FileRenameInfo, rename.cast(), length_u32) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        self.published = true;
        Ok(())
    }
}

impl Drop for PrivateStage {
    fn drop(&mut self) {
        if !self.published {
            let disposition = FILE_DISPOSITION_INFO { DeleteFile: 1 };
            // SAFETY: delete the owned object by its live DELETE-capable handle, never a replaceable path.
            // Cleanup can fail; preserve the primary error without claiming removal succeeded.
            unsafe {
                SetFileInformationByHandle(
                    self.file.as_raw_handle(),
                    FileDispositionInfo,
                    (&disposition as *const FILE_DISPOSITION_INFO).cast(),
                    size_of::<FILE_DISPOSITION_INFO>() as u32,
                )
            };
        }
    }
}

pub(super) fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let target = std::path::absolute(path)?;
    let parent = target.parent().ok_or_else(refused)?;
    let user = EffectiveUser::query()?;
    let descriptor = user.descriptor()?;
    let staged = PrivateStage::create(parent, &descriptor)?;
    staged.validate_empty(&user)?;
    staged.publish(&target, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_private_stage_denies_other_handles_and_deletes_by_handle() {
        let dir = tempfile::tempdir().unwrap();
        let user = EffectiveUser::query().unwrap();
        let descriptor = user.descriptor().unwrap();
        let staged = PrivateStage::create(dir.path(), &descriptor).unwrap();
        staged.validate_empty(&user).unwrap();
        assert_eq!(staged.file.metadata().unwrap().len(), 0);
        let path = std::fs::read_dir(dir.path()).unwrap().next().unwrap().unwrap().path();
        assert_eq!(File::open(&path).unwrap_err().raw_os_error(), Some(32));
        assert_eq!(std::fs::remove_file(&path).unwrap_err().raw_os_error(), Some(32));
        drop(staged);
        assert!(!path.exists());
    }

    #[test]
    fn invalid_descriptor_is_refused_while_stage_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let user = EffectiveUser::query().unwrap();
        let descriptor = security_descriptor("D:P(A;;FA;;;WD)").unwrap();
        let staged = PrivateStage::create(dir.path(), &descriptor).unwrap();
        assert!(staged.validate_empty(&user).is_err());
        assert_eq!(staged.file.metadata().unwrap().len(), 0);
        drop(staged);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}
