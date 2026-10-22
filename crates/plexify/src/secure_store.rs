//! Secret storage for the plex.tv token.
//!
//! On Windows the token lives in the Credential Manager (a generic credential, protected per
//! user by the OS). If that fails (rare, but e.g. restricted profiles), we fall back to a file
//! encrypted with DPAPI (`CryptProtectData`, current-user scope). Other platforms (development
//! builds only) use a 0600 file.

use std::path::PathBuf;

pub trait SecretStore: Send + Sync {
    fn get(&self) -> Option<String>;
    fn set(&self, secret: &str) -> Result<(), String>;
    fn delete(&self) -> Result<(), String>;
    fn describe(&self) -> &'static str;
}

/// The store used by the app. `name` scopes the secret (tests use their own names).
pub fn default_store(name: &str) -> Box<dyn SecretStore> {
    #[cfg(windows)]
    {
        Box::new(windows_impl::CredManagerWithFallback::new(name))
    }
    #[cfg(not(windows))]
    {
        Box::new(FileStore { path: crate::paths::config_dir().join(format!("{name}.secret")) })
    }
}

/// In-memory store for tests.
#[derive(Default)]
pub struct MemoryStore(std::sync::Mutex<Option<String>>);

impl SecretStore for MemoryStore {
    fn get(&self) -> Option<String> {
        self.0.lock().ok()?.clone()
    }
    fn set(&self, secret: &str) -> Result<(), String> {
        *self.0.lock().map_err(|e| e.to_string())? = Some(secret.to_string());
        Ok(())
    }
    fn delete(&self) -> Result<(), String> {
        *self.0.lock().map_err(|e| e.to_string())? = None;
        Ok(())
    }
    fn describe(&self) -> &'static str {
        "memory"
    }
}

/// Plain file with restrictive permissions (non-Windows development fallback).
pub struct FileStore {
    pub path: PathBuf,
}

impl SecretStore for FileStore {
    fn get(&self) -> Option<String> {
        std::fs::read_to_string(&self.path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }
    fn set(&self, secret: &str) -> Result<(), String> {
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&self.path, secret).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }
    fn delete(&self) -> Result<(), String> {
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
            _ => Ok(()),
        }
    }
    fn describe(&self) -> &'static str {
        "file"
    }
}

#[cfg(windows)]
pub mod windows_impl {
    use super::SecretStore;
    use std::path::PathBuf;
    use windows_sys::Win32::Foundation::{LocalFree, ERROR_NOT_FOUND, FILETIME, GetLastError};
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    pub struct CredManager {
        target: Vec<u16>,
    }

    impl CredManager {
        pub fn new(name: &str) -> Self {
            Self { target: wide(&format!("Plexify/{name}")) }
        }
    }

    impl SecretStore for CredManager {
        fn get(&self) -> Option<String> {
            unsafe {
                let mut pcred: *mut CREDENTIALW = std::ptr::null_mut();
                if CredReadW(self.target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut pcred) == 0 {
                    return None;
                }
                let c = &*pcred;
                let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
                let s = String::from_utf8(bytes.to_vec()).ok();
                CredFree(pcred as _);
                s.filter(|s| !s.is_empty())
            }
        }

        fn set(&self, secret: &str) -> Result<(), String> {
            let user = wide("plex.tv");
            let mut blob = secret.as_bytes().to_vec();
            let cred = CREDENTIALW {
                Flags: 0,
                Type: CRED_TYPE_GENERIC,
                TargetName: self.target.as_ptr() as *mut u16,
                Comment: std::ptr::null_mut(),
                LastWritten: FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 },
                CredentialBlobSize: blob.len() as u32,
                CredentialBlob: blob.as_mut_ptr(),
                Persist: CRED_PERSIST_LOCAL_MACHINE,
                AttributeCount: 0,
                Attributes: std::ptr::null_mut(),
                TargetAlias: std::ptr::null_mut(),
                UserName: user.as_ptr() as *mut u16,
            };
            if unsafe { CredWriteW(&cred, 0) } == 0 {
                return Err(format!("CredWriteW failed: {}", unsafe { GetLastError() }));
            }
            Ok(())
        }

        fn delete(&self) -> Result<(), String> {
            if unsafe { CredDeleteW(self.target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
                let err = unsafe { GetLastError() };
                if err != ERROR_NOT_FOUND {
                    return Err(format!("CredDeleteW failed: {err}"));
                }
            }
            Ok(())
        }

        fn describe(&self) -> &'static str {
            "Windows Credential Manager"
        }
    }

    /// File encrypted with DPAPI (current user).
    pub struct DpapiFile {
        pub path: PathBuf,
    }

    fn dpapi(data: &[u8], protect: bool) -> Result<Vec<u8>, String> {
        unsafe {
            let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
            let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
            let ok = if protect {
                CryptProtectData(&input, std::ptr::null(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            } else {
                CryptUnprotectData(&input, std::ptr::null_mut(), std::ptr::null(), std::ptr::null(), std::ptr::null(), CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            };
            if ok == 0 {
                return Err(format!("DPAPI failed: {}", GetLastError()));
            }
            let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(out.pbData as _);
            Ok(v)
        }
    }

    impl SecretStore for DpapiFile {
        fn get(&self) -> Option<String> {
            let enc = std::fs::read(&self.path).ok()?;
            String::from_utf8(dpapi(&enc, false).ok()?).ok().filter(|s| !s.is_empty())
        }
        fn set(&self, secret: &str) -> Result<(), String> {
            let enc = dpapi(secret.as_bytes(), true)?;
            if let Some(d) = self.path.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            std::fs::write(&self.path, enc).map_err(|e| e.to_string())
        }
        fn delete(&self) -> Result<(), String> {
            match std::fs::remove_file(&self.path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.to_string()),
                _ => Ok(()),
            }
        }
        fn describe(&self) -> &'static str {
            "DPAPI-encrypted file"
        }
    }

    /// Credential Manager first, DPAPI file if the Credential Manager is unavailable.
    pub struct CredManagerWithFallback {
        cred: CredManager,
        file: DpapiFile,
    }

    impl CredManagerWithFallback {
        pub fn new(name: &str) -> Self {
            Self {
                cred: CredManager::new(name),
                file: DpapiFile { path: crate::paths::config_dir().join(format!("{name}.dpapi")) },
            }
        }
    }

    impl SecretStore for CredManagerWithFallback {
        fn get(&self) -> Option<String> {
            self.cred.get().or_else(|| self.file.get())
        }
        fn set(&self, secret: &str) -> Result<(), String> {
            match self.cred.set(secret) {
                Ok(()) => {
                    let _ = self.file.delete();
                    Ok(())
                }
                Err(e) => {
                    log::warn!("{e}; storing token in DPAPI-encrypted file instead");
                    self.file.set(secret)
                }
            }
        }
        fn delete(&self) -> Result<(), String> {
            let a = self.cred.delete();
            let b = self.file.delete();
            a.and(b)
        }
        fn describe(&self) -> &'static str {
            "Windows Credential Manager (DPAPI file fallback)"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(store: &dyn SecretStore) {
        let _ = store.delete();
        assert_eq!(store.get(), None);
        store.set("tok-123").unwrap();
        assert_eq!(store.get().as_deref(), Some("tok-123"));
        store.set("tok-456").unwrap();
        assert_eq!(store.get().as_deref(), Some("tok-456"));
        store.delete().unwrap();
        assert_eq!(store.get(), None);
        store.delete().unwrap(); // idempotent
    }

    #[test]
    fn memory_store_roundtrip() {
        roundtrip(&MemoryStore::default());
    }

    #[test]
    fn file_store_roundtrip() {
        let dir = std::env::temp_dir().join(format!("plexify-secret-{}", std::process::id()));
        roundtrip(&FileStore { path: dir.join("t.secret") });
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn windows_credential_manager_roundtrip() {
        roundtrip(&windows_impl::CredManager::new("unit-test"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_dpapi_file_roundtrip() {
        let dir = std::env::temp_dir().join(format!("plexify-dpapi-{}", std::process::id()));
        let store = windows_impl::DpapiFile { path: dir.join("t.dpapi") };
        roundtrip(&store);
        store.set("secret!").unwrap();
        let raw = std::fs::read(&store.path).unwrap();
        assert!(!raw.windows(7).any(|w| w == b"secret!"), "file must be encrypted");
        let _ = std::fs::remove_dir_all(dir);
    }
}
