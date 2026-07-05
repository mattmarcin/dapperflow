//! At-rest sealing for vault values against the OS credential store
//! (`environments.md` / Env Vault: "a key held in the OS credential store").
//!
//! # Windows (DPAPI)
//!
//! The Windows backend uses DPAPI (`CryptProtectData` / `CryptUnprotectData`) at the
//! `CRYPTPROTECT_LOCAL_MACHINE`-off, per-user scope. DPAPI derives the encryption key
//! from the logged-in user's credentials and keeps it inside LSASS - **the key is
//! never written to disk by us or by DPAPI**, satisfying `environments.md` ("key never
//! on disk"). We store only the ciphertext blob it returns. Only the same user on the
//! same machine can unseal, which is exactly the local threat model
//! (`security.md` / Non-goals: an attacker with the user's own OS account is out of
//! scope).
//!
//! # Cross-platform seam (macOS / Linux, deferred)
//!
//! [`VaultCrypto`] is the seam. macOS should back it with Keychain
//! (`SecItemAdd`/`SecItemCopyMatching`, a Keychain-generated symmetric key) and Linux
//! with the Secret Service API (libsecret) - both keep the key in the OS keystore, so
//! `env_entries.ciphertext` and `key_id` do not change and no migration is needed when
//! they land. Until then, non-Windows builds use [`StubCrypto`], which is
//! **obfuscation only, not encryption**, and logs a loud warning: it exists so the
//! crate compiles and its logic is testable off Windows, never as a real at-rest
//! control. The flagship target is Windows (`phase6-mcp.md`).

/// A backend that seals and unseals vault values against an OS credential store.
pub trait VaultCrypto: Send + Sync {
    /// Backend id recorded in `env_entries.key_id` (`"dpapi"`, later `"keychain"` /
    /// `"secret-service"`), so a blob is always opened by the backend that sealed it.
    fn key_id(&self) -> &'static str;

    /// Encrypt `plaintext` at rest, returning the opaque ciphertext to persist.
    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError>;

    /// Decrypt a blob previously produced by [`VaultCrypto::seal`] on this machine.
    fn open(&self, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError>;

    /// Whether this backend provides real at-rest encryption. `false` for the
    /// non-Windows developer stub, so callers can surface the weaker guarantee.
    fn is_secure(&self) -> bool {
        true
    }
}

/// Errors sealing or unsealing a vault value.
#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("credential-store seal failed: {0}")]
    Seal(String),
    #[error("credential-store unseal failed: {0}")]
    Open(String),
}

/// The default backend for this platform: DPAPI on Windows, the developer stub
/// elsewhere (see the module note).
pub fn default_crypto() -> Box<dyn VaultCrypto> {
    #[cfg(windows)]
    {
        Box::new(dpapi::DpapiCrypto)
    }
    #[cfg(not(windows))]
    {
        tracing::warn!(
            "no OS credential-store backend for this platform yet; vault values use an \
             INSECURE developer obfuscation stub (see env::crypto). Do not ship non-Windows."
        );
        Box::new(StubCrypto)
    }
}

#[cfg(windows)]
mod dpapi {
    use super::{CryptoError, VaultCrypto};
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    /// The Windows DPAPI backend (`environments.md`: DPAPI on Windows).
    pub struct DpapiCrypto;

    impl VaultCrypto for DpapiCrypto {
        fn key_id(&self) -> &'static str {
            "dpapi"
        }

        fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
            dpapi_call(plaintext, Op::Protect).map_err(CryptoError::Seal)
        }

        fn open(&self, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
            dpapi_call(ciphertext, Op::Unprotect).map_err(CryptoError::Open)
        }
    }

    enum Op {
        Protect,
        Unprotect,
    }

    /// Run one DPAPI protect/unprotect call over `input`, copying the result out of the
    /// LocalAlloc'd blob and freeing it. `CRYPTPROTECT_UI_FORBIDDEN` guarantees the call
    /// never blocks on a UI prompt (the daemon is headless).
    fn dpapi_call(input: &[u8], op: Op) -> Result<Vec<u8>, String> {
        // The input blob is passed as `*const`: DPAPI does not mutate our buffer. `buf`
        // is a local copy so the `&[u8]` contract holds (`as_mut_ptr` needs a mut buffer,
        // but the blob struct itself is immutable).
        let mut buf = input.to_vec();
        let in_blob = CRYPT_INTEGER_BLOB { cbData: buf.len() as u32, pbData: buf.as_mut_ptr() };
        let mut out_blob = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };

        // SAFETY: both blobs are valid for the duration of the call; all optional
        // pointer args are null (no description, no entropy, no prompt). On success DPAPI
        // fills `out_blob` with a LocalAlloc'd buffer we copy from and then free.
        let ok = unsafe {
            match op {
                Op::Protect => CryptProtectData(
                    &in_blob,
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut out_blob,
                ),
                Op::Unprotect => CryptUnprotectData(
                    &in_blob,
                    // ppszdatadescr is an out-param (`*mut PWSTR`); we discard it.
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                    std::ptr::null(),
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &mut out_blob,
                ),
            }
        };

        if ok == 0 {
            let code = std::io::Error::last_os_error();
            return Err(format!("DPAPI returned failure: {code}"));
        }
        if out_blob.pbData.is_null() {
            return Err("DPAPI returned a null output blob".into());
        }

        // SAFETY: DPAPI guarantees `pbData` points to `cbData` valid bytes on success.
        let out = unsafe {
            let slice = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize);
            let copy = slice.to_vec();
            LocalFree(out_blob.pbData as *mut core::ffi::c_void);
            copy
        };
        Ok(out)
    }
}

/// A non-Windows developer stub: **obfuscation only, not encryption**. It XORs with a
/// fixed pad and reverses, so `open(seal(x)) == x` and the persisted bytes are not the
/// literal plaintext - enough to exercise the vault's logic in CI off Windows, and
/// nothing more (see the module note). Never compiled on Windows.
#[cfg(not(windows))]
pub struct StubCrypto;

#[cfg(not(windows))]
impl VaultCrypto for StubCrypto {
    fn key_id(&self) -> &'static str {
        "stub-insecure"
    }

    fn seal(&self, plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        Ok(stub_transform(plaintext))
    }

    fn open(&self, ciphertext: &[u8]) -> Result<Vec<u8>, CryptoError> {
        Ok(stub_transform(ciphertext))
    }

    fn is_secure(&self) -> bool {
        false
    }
}

#[cfg(not(windows))]
fn stub_transform(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().map(|b| b ^ 0x5a).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_open_round_trips_on_this_platform() {
        let crypto = default_crypto();
        let secret = b"sk-live-abc123-super-secret";
        let sealed = crypto.seal(secret).expect("seal");
        assert_ne!(sealed.as_slice(), secret, "ciphertext must not be the plaintext");
        let opened = crypto.open(&sealed).expect("open");
        assert_eq!(opened.as_slice(), secret, "open(seal(x)) must recover x");
    }

    #[test]
    fn empty_value_round_trips() {
        let crypto = default_crypto();
        let sealed = crypto.seal(b"").expect("seal empty");
        let opened = crypto.open(&sealed).expect("open empty");
        assert!(opened.is_empty());
    }
}
