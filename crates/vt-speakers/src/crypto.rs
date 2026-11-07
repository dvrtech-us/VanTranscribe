use crate::{MemoryError, Result, SecretKey};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use zeroize::Zeroizing;

const HEADER: &[u8] = b"VTSP\x01";
const MAX_BLOB: usize = 64 * 1024;

pub(crate) fn encrypt(key: &SecretKey, plaintext: &[u8], aad: &[u8]) -> Result<Vec<u8>> {
    let mut nonce = [0; 24];
    getrandom::fill(&mut nonce).map_err(|_| MemoryError::Random)?;
    let ciphertext = XChaCha20Poly1305::new_from_slice(key.as_ref())
        .map_err(|_| MemoryError::Authentication)?
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| MemoryError::Authentication)?;
    if ciphertext.len() + HEADER.len() + nonce.len() > MAX_BLOB {
        return Err(MemoryError::Authentication);
    }
    let mut blob = Vec::with_capacity(HEADER.len() + nonce.len() + ciphertext.len());
    blob.extend_from_slice(HEADER);
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ciphertext);
    Ok(blob)
}
pub(crate) fn decrypt(key: &SecretKey, blob: &[u8], aad: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
    if blob.len() < HEADER.len() + 24 + 16 || blob.len() > MAX_BLOB || !blob.starts_with(HEADER) {
        return Err(MemoryError::Authentication);
    }
    let nonce = &blob[HEADER.len()..HEADER.len() + 24];
    XChaCha20Poly1305::new_from_slice(key.as_ref())
        .map_err(|_| MemoryError::Authentication)?
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: &blob[HEADER.len() + 24..],
                aad,
            },
        )
        .map(Zeroizing::new)
        .map_err(|_| MemoryError::Authentication)
}
