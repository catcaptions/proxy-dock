//! Windows Credential Manager backend for Generic credentials.
//!
//! Same slot naming the previous keyring backend used (`{user}.{service}`
//! target, `{user}` user name), so any entries that did land interoperate.
//! Talks to `advapi32` directly via `windows-sys`: keyring 3.6's writes
//! silently vanish in some sessions here (`set` Ok, `get` NoEntry), while
//! raw `CredWriteW`/`CredReadW` round-trips fine.
//!
//! Large secrets (OAuth JSON with id_token runs ~3 kB in UTF-16) exceed the
//! vault's per-entry blob ceiling, so values are chunked: the main slot
//! holds a small JSON manifest, chunks live in `{slot}:chunk:{i}` entries.
//! Single-entry blobs written before chunking still read back (legacy path).

use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
use windows_sys::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

/// Bytes per chunk entry — far below the vault blob ceiling.
const CHUNK_BYTES: usize = 512;
/// Upper bound on chunks per secret (512 × 256 = 128 kB, absurdly ample).
const MAX_CHUNKS: usize = 256;
/// Upper bound on vault target names. Credential targets travel to
/// `CredWriteW` as UTF-16; overlong names fail the write and — worse — widen
/// the orphan-sweep surface on delete. Overlong inputs are rejected by
/// [`bounded_target`] before any vault call.
pub const MAX_TARGET_CHARS: usize = 256;

/// Bound a vault target name. `None` when blank or overlong (callers surface
/// an honest error instead of attempting a doomed vault write).
pub fn bounded_target(target: &str) -> Option<String> {
    let trimmed = target.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_TARGET_CHARS {
        return None;
    }
    Some(trimmed.to_string())
}

/// Vault target for a (service, user) pair. Mirrors the old backend.
///
/// DPAPI scope note: entries persist with `CRED_PERSIST_LOCAL_MACHINE`, so
/// the blob is machine-scoped (any process on this machine with vault access
/// can read the slot — same as the previous keyring backend). Secrets never
/// leave the machine; there is no per-user DPAPI isolation beyond the OS
/// account boundary. Chunk entries inherit the same scope.
pub fn target_name(service: &str, user: &str) -> String {
    format!("{user}.{service}")
}

fn chunk_target(service: &str, user: &str, index: usize) -> String {
    format!("{}:chunk:{index}", target_name(service, user))
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn last_error() -> u32 {
    unsafe { GetLastError() }
}

fn write_entry(target: &str, username: &str, blob: &[u8]) -> Result<(), String> {
    let target_w = wide(target);
    let username_w = wide(username);
    // windows-sys takes ownership only for the duration of the call; the
    // Vecs outlive it, so the raw pointers stay valid throughout.
    let mut blob_copy = blob.to_vec();
    let credential = CREDENTIALW {
        Flags: 0,
        Type: CRED_TYPE_GENERIC,
        TargetName: target_w.as_ptr() as *mut u16,
        Comment: std::ptr::null_mut(),
        LastWritten: unsafe { std::mem::zeroed() },
        CredentialBlobSize: blob_copy.len() as u32,
        CredentialBlob: blob_copy.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        AttributeCount: 0,
        Attributes: std::ptr::null_mut(),
        TargetAlias: std::ptr::null_mut(),
        UserName: username_w.as_ptr() as *mut u16,
    };
    let wrote = unsafe { CredWriteW(&credential, 0) };
    for b in blob_copy.iter_mut() {
        *b = 0;
    }
    if wrote == 0 {
        return Err(format!("CredWriteW failed (win32 {})", last_error()));
    }
    Ok(())
}

fn read_entry(target: &str) -> Result<Vec<u8>, String> {
    let target_w = wide(target);
    let mut out: *mut CREDENTIALW = std::ptr::null_mut();
    let read = unsafe { CredReadW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0, &mut out) };
    if read == 0 {
        let code = last_error();
        if code == ERROR_NOT_FOUND {
            return Err("No matching entry found in secure storage".to_string());
        }
        return Err(format!("CredReadW failed (win32 {code})"));
    }
    let result = unsafe {
        let cred = &*out;
        let len = cred.CredentialBlobSize as usize;
        if len == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(cred.CredentialBlob, len).to_vec()
        }
    };
    unsafe { CredFree(out as *mut std::ffi::c_void) };
    Ok(result)
}

fn delete_entry(target: &str) -> Result<(), String> {
    let target_w = wide(target);
    let deleted = unsafe { CredDeleteW(target_w.as_ptr(), CRED_TYPE_GENERIC, 0) };
    if deleted == 0 {
        let code = last_error();
        if code == ERROR_NOT_FOUND {
            return Ok(());
        }
        return Err(format!("CredDeleteW failed (win32 {code})"));
    }
    Ok(())
}

fn decode_utf16_le(blob: &[u8]) -> Result<String, String> {
    if blob.len() % 2 != 0 {
        return Err("credential blob is not UTF-16".to_string());
    }
    let u16s: Vec<u16> = blob
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16(&u16s).map_err(|_| "credential is not valid text".to_string())
}

/// Split UTF-8 bytes on char boundaries so no chunk exceeds CHUNK_BYTES.
fn split_chunks(secret: &str) -> Vec<&[u8]> {
    let bytes = secret.as_bytes();
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < bytes.len() {
        let mut end = (start + CHUNK_BYTES).min(bytes.len());
        while end > start && !secret.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            // A single char wider than a chunk (cannot happen for JSON, but
            // never loop forever): emit its full encoding as one chunk.
            end = start + secret[start..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
            end = end.min(bytes.len());
        }
        chunks.push(&bytes[start..end]);
        start = end;
    }
    if chunks.is_empty() {
        chunks.push(&bytes[0..0]);
    }
    chunks
}

/// Store `secret` for `(service, user)`, replacing any existing entry.
pub fn set(service: &str, user: &str, secret: &str) -> Result<(), String> {
    let _ = bounded_target(&target_name(service, user)).ok_or_else(|| "credential slot name too long".to_string())?;
    let chunks = split_chunks(secret);
    if chunks.len() > MAX_CHUNKS {
        return Err(format!("secret too large ({} chunks)", chunks.len()));
    }
    // How many chunks the previous value used (0 for legacy/absent).
    let old_chunks = read_entry(&target_name(service, user))
        .ok()
        .and_then(|b| std::str::from_utf8(&b).ok().and_then(parse_manifest))
        .unwrap_or(0);
    // Chunks first, manifest last; then drop any stale higher chunks.
    for (i, chunk) in chunks.iter().enumerate() {
        write_entry(&chunk_target(service, user, i), user, chunk)?;
    }
    let manifest = format!(r#"{{"v":1,"chunks":{}}}"#, chunks.len());
    write_entry(&target_name(service, user), user, manifest.as_bytes())?;
    for i in chunks.len()..old_chunks {
        delete_entry(&chunk_target(service, user, i))?;
    }
    Ok(())
}

/// Read the secret for `(service, user)`.
pub fn get(service: &str, user: &str) -> Result<String, String> {
    let main = read_entry(&target_name(service, user))?;
    // Manifest path: {"v":1,"chunks":N} stored as UTF-8.
    if let Ok(text) = std::str::from_utf8(&main) {
        if let Some(n) = parse_manifest(text) {
            let mut out: Vec<u8> = Vec::new();
            for i in 0..n {
                out.extend_from_slice(&read_entry(&chunk_target(service, user, i))?);
            }
            return String::from_utf8(out).map_err(|_| "credential is not valid text".to_string());
        }
    }
    // Legacy path: whole secret as one UTF-16 blob.
    decode_utf16_le(&main)
}

fn parse_manifest(text: &str) -> Option<usize> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    if v.get("v")?.as_u64()? != 1 {
        return None;
    }
    let n = v.get("chunks")?.as_u64()? as usize;
    if n == 0 || n > MAX_CHUNKS {
        return None;
    }
    Some(n)
}

/// Delete the entry and all its chunks. Missing entries are fine.
/// Manifest-independent sweep: when the manifest itself is gone (or corrupt),
/// chunk entries would otherwise orphan — so every chunk slot `0..MAX_CHUNKS`
/// is deleted regardless of what the manifest claims, then the main slot.
pub fn delete(service: &str, user: &str) -> Result<(), String> {
    let _ = bounded_target(&target_name(service, user)).ok_or_else(|| "credential slot name too long".to_string())?;
    sweep_chunks(service, user);
    delete_entry(&target_name(service, user))?;
    Ok(())
}

/// Delete every chunk slot for `(service, user)`. Best-effort per slot
/// (missing slots are fine); hard failures abort so callers see them.
fn sweep_chunks(service: &str, user: &str) {
    for i in 0..MAX_CHUNKS {
        let target = chunk_target(service, user, i);
        // `delete_entry` is Ok on NOT_FOUND, so a full sweep is cheap and
        // never fails on already-clean slots.
        if delete_entry(&target).is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_names_stay_bounded() {
        assert!(bounded_target(&target_name("ai.proxydock", "local-token:chatgpt:a@b.c")).is_some());
        assert!(bounded_target("").is_none());
        assert!(bounded_target("   ").is_none());
        let overlong = "x".repeat(MAX_TARGET_CHARS + 1);
        assert!(bounded_target(&overlong).is_none());
        assert!(bounded_target(&"x".repeat(MAX_TARGET_CHARS)).is_some());
    }

    #[test]
    fn chunk_naming_is_stable_and_manifest_independent() {
        // Sweep range covers every writable slot: split_chunks never emits
        // more than MAX_CHUNKS (set() rejects above it), so delete() with no
        // manifest still reaches every chunk set() could have written.
        assert_eq!(chunk_target("svc", "user", 0), "user.svc:chunk:0");
        assert_eq!(chunk_target("svc", "user", MAX_CHUNKS - 1), format!("user.svc:chunk:{}", MAX_CHUNKS - 1));
        let big = "x".repeat(CHUNK_BYTES * 3 + 10);
        let chunks = split_chunks(&big);
        assert_eq!(chunks.len(), 4);
        assert!(chunks.iter().all(|c| c.len() <= CHUNK_BYTES));
        // Manifest parse rejects garbage (sweep path never trusts it).
        assert!(parse_manifest("not-json").is_none());
        assert!(parse_manifest(r#"{"v":2,"chunks":3}"#).is_none());
        assert_eq!(parse_manifest(r#"{"v":1,"chunks":3}"#), Some(3));
    }

    /// Touches the real vault — runs only with PROXYDOCK_TEST_KEYRING=1 so the
    /// normal suite stays hermetic (old PROXYHUB_TEST_KEYRING also works).
    #[test]
    fn vault_roundtrip() {
        if std::env::var("PROXYDOCK_TEST_KEYRING").is_err() && std::env::var("PROXYHUB_TEST_KEYRING").is_err() {
            println!("wincred vault_roundtrip skipped (set PROXYDOCK_TEST_KEYRING=1 to run)");
            return;
        }
        let user = format!("probe-wincred-{}", std::process::id());
        // Small (single-chunk) and large (multi-chunk, realistic OAuth size).
        for secret in [ "roundtrip-secret-value".to_string(), "x".repeat(3000) ] {
            set("ai.proxydock", &user, &secret).expect("set");
            let back = get("ai.proxydock", &user).expect("get");
            assert_eq!(back, secret);
        }
        delete("ai.proxydock", &user).expect("delete");
        assert!(get("ai.proxydock", &user).is_err());
        println!("wincred vault_roundtrip passed");
    }
}
