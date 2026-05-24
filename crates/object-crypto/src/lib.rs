use aes_gcm::{
    Aes256Gcm,
    aead::{AeadInPlace, KeyInit, generic_array::GenericArray},
};
use blob_core::{BlobError, ObjectBody, ObjectBodyStream};
use bytes::{Bytes, BytesMut};
use chacha20poly1305::ChaCha20Poly1305;
use futures_util::StreamExt;
use rand::{RngCore, rngs::OsRng};
use thiserror::Error;

const ENVELOPE_MAGIC: &[u8; 8] = b"CCBGENC1";
const ENVELOPE_FIXED_HEADER_LEN: usize = 50;
const AEAD_TAG_LEN: usize = 16;
const NONCE_LEN: usize = 12;
const CHUNK_NONCE_PREFIX_LEN: usize = 4;
const DATA_KEY_LEN: usize = 32;

pub const STORED_ENCRYPTED_CONTENT_TYPE: &str = "application/vnd.ccbg.encrypted";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlgorithmPreference {
    Auto,
    Aes256Gcm,
    ChaCha20Poly1305,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeAlgorithm {
    Aes256Gcm,
    ChaCha20Poly1305,
}

impl RuntimeAlgorithm {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aes256Gcm => "aes_256_gcm",
            Self::ChaCha20Poly1305 => "chacha20_poly1305",
        }
    }

    fn id(self) -> u8 {
        match self {
            Self::Aes256Gcm => 1,
            Self::ChaCha20Poly1305 => 2,
        }
    }

    fn from_id(value: u8) -> Result<Self, CryptoError> {
        match value {
            1 => Ok(Self::Aes256Gcm),
            2 => Ok(Self::ChaCha20Poly1305),
            other => Err(CryptoError::InvalidObject(format!(
                "unsupported encrypted object algorithm id: {other}"
            ))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptRequest {
    pub profile_id: String,
    pub key_id: String,
    pub algorithm_preference: AlgorithmPreference,
    pub chunk_plaintext_bytes: u64,
    pub plaintext_size: u64,
    pub logical_content_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedObjectMetadata {
    pub profile_id: String,
    pub key_id: String,
    pub algorithm: RuntimeAlgorithm,
    pub chunk_plaintext_bytes: u64,
    pub plaintext_size: u64,
    pub stored_size: u64,
    pub logical_content_type: Option<String>,
}

pub struct EncryptedObject {
    pub body: ObjectBody,
    pub metadata: EncryptedObjectMetadata,
}

pub struct PreparedDecryptDownload {
    pub body: ObjectBody,
    pub metadata: EncryptedObjectMetadata,
}

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("invalid encryption request: {0}")]
    InvalidRequest(String),
    #[error("invalid encrypted object: {0}")]
    InvalidObject(String),
    #[error("failed to read encrypted stream: {0}")]
    BodyStream(String),
    #[error("failed to wrap object key: {0}")]
    KeyWrap(String),
    #[error("failed to unwrap object key: {0}")]
    KeyUnwrap(String),
}

impl EncryptRequest {
    pub fn planned_metadata(&self) -> Result<EncryptedObjectMetadata, CryptoError> {
        validate_encrypt_request(self)?;
        let algorithm = choose_runtime_algorithm(self.algorithm_preference);
        let header_len = envelope_header_len(self)?;
        let chunk_count = chunk_count(self.plaintext_size, self.chunk_plaintext_bytes);
        let per_chunk_overhead = u64::try_from(4 + AEAD_TAG_LEN)
            .expect("per-chunk encryption overhead should fit into u64");
        let chunk_overhead = chunk_count.checked_mul(per_chunk_overhead).ok_or_else(|| {
            CryptoError::InvalidRequest(format!(
                "object size {} with chunk size {} overflows encrypted envelope sizing",
                self.plaintext_size, self.chunk_plaintext_bytes
            ))
        })?;
        let stored_size = u64::try_from(header_len)
            .map_err(|_| CryptoError::InvalidRequest("header length overflow".to_string()))?
            .checked_add(self.plaintext_size)
            .and_then(|value| value.checked_add(chunk_overhead))
            .ok_or_else(|| {
                CryptoError::InvalidRequest(format!(
                    "object size {} with chunk size {} overflows stored size accounting",
                    self.plaintext_size, self.chunk_plaintext_bytes
                ))
            })?;
        Ok(EncryptedObjectMetadata {
            profile_id: self.profile_id.clone(),
            key_id: self.key_id.clone(),
            algorithm,
            chunk_plaintext_bytes: self.chunk_plaintext_bytes,
            plaintext_size: self.plaintext_size,
            stored_size,
            logical_content_type: normalize_optional_string(self.logical_content_type.clone()),
        })
    }
}

pub fn encrypt_upload(
    body: ObjectBody,
    request: EncryptRequest,
    kek: [u8; DATA_KEY_LEN],
) -> Result<EncryptedObject, CryptoError> {
    let metadata = request.planned_metadata()?;
    let mut data_key = [0u8; DATA_KEY_LEN];
    OsRng.fill_bytes(&mut data_key);
    let mut key_wrap_nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut key_wrap_nonce);
    let mut chunk_nonce_prefix = [0u8; CHUNK_NONCE_PREFIX_LEN];
    OsRng.fill_bytes(&mut chunk_nonce_prefix);

    let wrap_cipher = CipherInstance::new(metadata.algorithm, &kek)?;
    let wrapped_data_key = wrap_cipher.encrypt(&key_wrap_nonce, &[], &data_key, "data key wrap")?;
    let header_bytes = build_envelope_header(
        &request,
        &metadata,
        &wrapped_data_key,
        &key_wrap_nonce,
        &chunk_nonce_prefix,
    )?;
    let header_for_aad = Bytes::from(header_bytes.clone());
    let data_cipher = CipherInstance::new(metadata.algorithm, &data_key)?;
    let chunk_plaintext_bytes = usize::try_from(metadata.chunk_plaintext_bytes).map_err(|_| {
        CryptoError::InvalidRequest(format!(
            "chunk size {} does not fit into usize",
            metadata.chunk_plaintext_bytes
        ))
    })?;

    let stream = futures_util::stream::unfold(
        EncryptStreamState {
            header: Some(Bytes::from(header_bytes)),
            header_aad: header_for_aad,
            chunker: PlaintextChunker::new(body.into_stream()),
            cipher: data_cipher,
            chunk_plaintext_bytes,
            chunk_nonce_prefix,
            next_chunk_index: 0,
        },
        |mut state| async move {
            if let Some(header) = state.header.take() {
                return Some((Ok(header), state));
            }

            match state.chunker.next_chunk(state.chunk_plaintext_bytes).await {
                Ok(Some(plaintext)) => {
                    let nonce = chunk_nonce(state.chunk_nonce_prefix, state.next_chunk_index);
                    state.next_chunk_index = state.next_chunk_index.saturating_add(1);
                    let encrypted = match state.cipher.encrypt(
                        &nonce,
                        state.header_aad.as_ref(),
                        &plaintext,
                        "object chunk",
                    ) {
                        Ok(encrypted) => encrypted,
                        Err(error) => {
                            return Some((Err(BlobError::BodyStream(error.to_string())), state));
                        }
                    };
                    let frame_len = match u32::try_from(encrypted.len()) {
                        Ok(value) => value,
                        Err(_) => {
                            return Some((
                                Err(BlobError::BodyStream(
                                    "encrypted chunk length exceeds u32".to_string(),
                                )),
                                state,
                            ));
                        }
                    };
                    let mut frame = Vec::with_capacity(4 + encrypted.len());
                    frame.extend_from_slice(&frame_len.to_be_bytes());
                    frame.extend_from_slice(&encrypted);
                    Some((Ok(Bytes::from(frame)), state))
                }
                Ok(None) => None,
                Err(error) => Some((Err(error), state)),
            }
        },
    );

    Ok(EncryptedObject {
        body: ObjectBody::from_stream(stream),
        metadata,
    })
}

pub async fn prepare_decrypt_download(
    body: ObjectBody,
    kek: [u8; DATA_KEY_LEN],
) -> Result<PreparedDecryptDownload, CryptoError> {
    let mut cursor = StreamCursor::new(body.into_stream());
    let header = read_envelope_header(&mut cursor).await?;
    let wrap_cipher = CipherInstance::new(header.metadata.algorithm, &kek)?;
    let data_key = wrap_cipher.decrypt(
        &header.key_wrap_nonce,
        &[],
        &header.wrapped_data_key,
        "data key unwrap",
    )?;
    if data_key.len() != DATA_KEY_LEN {
        return Err(CryptoError::KeyUnwrap(format!(
            "expected {} decrypted data-key bytes, got {}",
            DATA_KEY_LEN,
            data_key.len()
        )));
    }
    let mut data_key_bytes = [0u8; DATA_KEY_LEN];
    data_key_bytes.copy_from_slice(&data_key);
    let data_cipher = CipherInstance::new(header.metadata.algorithm, &data_key_bytes)?;

    if header.metadata.plaintext_size == 0 {
        cursor.ensure_eof().await?;
        return Ok(PreparedDecryptDownload {
            body: ObjectBody::from_bytes(Bytes::new()),
            metadata: header.metadata,
        });
    }

    let stream = futures_util::stream::unfold(
        DecryptStreamState {
            cursor,
            cipher: data_cipher,
            header_aad: header.header_bytes,
            chunk_nonce_prefix: header.chunk_nonce_prefix,
            next_chunk_index: 0,
            remaining_plaintext_bytes: header.metadata.plaintext_size,
        },
        |mut state| async move {
            if state.remaining_plaintext_bytes == 0 {
                return None;
            }

            let frame_len = match state.cursor.read_exact(4).await {
                Ok(bytes) => u32::from_be_bytes(bytes.as_ref().try_into().expect("frame prefix")),
                Err(error) => {
                    return Some((Err(BlobError::BodyStream(error.to_string())), state));
                }
            };
            if frame_len < AEAD_TAG_LEN as u32 {
                return Some((
                    Err(BlobError::BodyStream(
                        CryptoError::InvalidObject(format!(
                            "encrypted chunk length {frame_len} is smaller than authentication tag"
                        ))
                        .to_string(),
                    )),
                    state,
                ));
            }

            let encrypted = match state.cursor.read_exact(frame_len as usize).await {
                Ok(bytes) => bytes,
                Err(error) => {
                    return Some((Err(BlobError::BodyStream(error.to_string())), state));
                }
            };
            let nonce = chunk_nonce(state.chunk_nonce_prefix, state.next_chunk_index);
            state.next_chunk_index = state.next_chunk_index.saturating_add(1);
            let plaintext = match state.cipher.decrypt(
                &nonce,
                state.header_aad.as_ref(),
                encrypted.as_ref(),
                "object chunk",
            ) {
                Ok(plaintext) => plaintext,
                Err(error) => {
                    return Some((Err(BlobError::BodyStream(error.to_string())), state));
                }
            };

            let plaintext_len = plaintext.len() as u64;
            if plaintext_len == 0 || plaintext_len > state.remaining_plaintext_bytes {
                return Some((
                    Err(BlobError::BodyStream(
                        CryptoError::InvalidObject(format!(
                            "decrypted chunk length {} is invalid for remaining plaintext {}",
                            plaintext_len, state.remaining_plaintext_bytes
                        ))
                        .to_string(),
                    )),
                    state,
                ));
            }
            state.remaining_plaintext_bytes -= plaintext_len;
            if state.remaining_plaintext_bytes == 0 {
                if let Err(error) = state.cursor.ensure_eof().await {
                    return Some((Err(BlobError::BodyStream(error.to_string())), state));
                }
            }

            Some((Ok(Bytes::from(plaintext)), state))
        },
    );

    Ok(PreparedDecryptDownload {
        body: ObjectBody::from_stream(stream),
        metadata: header.metadata,
    })
}

fn validate_encrypt_request(request: &EncryptRequest) -> Result<(), CryptoError> {
    if request.profile_id.trim().is_empty() {
        return Err(CryptoError::InvalidRequest(
            "profile_id must not be empty".to_string(),
        ));
    }
    if request.key_id.trim().is_empty() {
        return Err(CryptoError::InvalidRequest(
            "key_id must not be empty".to_string(),
        ));
    }
    if request.chunk_plaintext_bytes == 0 {
        return Err(CryptoError::InvalidRequest(
            "chunk_plaintext_bytes must be greater than zero".to_string(),
        ));
    }
    if request.chunk_plaintext_bytes > u32::MAX as u64 {
        return Err(CryptoError::InvalidRequest(format!(
            "chunk_plaintext_bytes {} exceeds u32::MAX",
            request.chunk_plaintext_bytes
        )));
    }
    let profile_id_len = request.profile_id.trim().as_bytes().len();
    if profile_id_len > u16::MAX as usize {
        return Err(CryptoError::InvalidRequest(format!(
            "profile_id is too long: {} bytes",
            profile_id_len
        )));
    }
    let key_id_len = request.key_id.trim().as_bytes().len();
    if key_id_len > u16::MAX as usize {
        return Err(CryptoError::InvalidRequest(format!(
            "key_id is too long: {} bytes",
            key_id_len
        )));
    }
    if request
        .logical_content_type
        .as_deref()
        .unwrap_or_default()
        .as_bytes()
        .len()
        > u16::MAX as usize
    {
        return Err(CryptoError::InvalidRequest(
            "logical_content_type is too long".to_string(),
        ));
    }
    Ok(())
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn choose_runtime_algorithm(preference: AlgorithmPreference) -> RuntimeAlgorithm {
    match preference {
        AlgorithmPreference::Auto => auto_runtime_algorithm(),
        AlgorithmPreference::Aes256Gcm => RuntimeAlgorithm::Aes256Gcm,
        AlgorithmPreference::ChaCha20Poly1305 => RuntimeAlgorithm::ChaCha20Poly1305,
    }
}

fn auto_runtime_algorithm() -> RuntimeAlgorithm {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        if std::is_x86_feature_detected!("aes") && std::is_x86_feature_detected!("pclmulqdq") {
            return RuntimeAlgorithm::Aes256Gcm;
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if std::arch::is_aarch64_feature_detected!("aes")
            && std::arch::is_aarch64_feature_detected!("pmull")
        {
            return RuntimeAlgorithm::Aes256Gcm;
        }
    }
    RuntimeAlgorithm::ChaCha20Poly1305
}

fn chunk_count(plaintext_size: u64, chunk_plaintext_bytes: u64) -> u64 {
    if plaintext_size == 0 {
        0
    } else {
        ((plaintext_size - 1) / chunk_plaintext_bytes) + 1
    }
}

fn envelope_header_len(request: &EncryptRequest) -> Result<usize, CryptoError> {
    validate_encrypt_request(request)?;
    Ok(ENVELOPE_FIXED_HEADER_LEN
        + request.key_id.trim().len()
        + request.profile_id.trim().len()
        + request
            .logical_content_type
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::len)
            .unwrap_or(0)
        + DATA_KEY_LEN
        + AEAD_TAG_LEN)
}

fn build_envelope_header(
    request: &EncryptRequest,
    metadata: &EncryptedObjectMetadata,
    wrapped_data_key: &[u8],
    key_wrap_nonce: &[u8; NONCE_LEN],
    chunk_nonce_prefix: &[u8; CHUNK_NONCE_PREFIX_LEN],
) -> Result<Vec<u8>, CryptoError> {
    let profile_id = request.profile_id.trim().as_bytes();
    let key_id = request.key_id.trim().as_bytes();
    let logical_content_type = metadata
        .logical_content_type
        .as_deref()
        .map(str::as_bytes)
        .unwrap_or_default();
    let header_len = envelope_header_len(request)?;
    let mut header = Vec::with_capacity(header_len);
    header.extend_from_slice(ENVELOPE_MAGIC);
    header.extend_from_slice(
        &u32::try_from(header_len)
            .map_err(|_| CryptoError::InvalidRequest("header length overflow".to_string()))?
            .to_be_bytes(),
    );
    header.push(metadata.algorithm.id());
    header.push(0);
    header.extend_from_slice(
        &u16::try_from(key_id.len())
            .map_err(|_| CryptoError::InvalidRequest("key_id length overflow".to_string()))?
            .to_be_bytes(),
    );
    header.extend_from_slice(
        &u16::try_from(profile_id.len())
            .map_err(|_| CryptoError::InvalidRequest("profile_id length overflow".to_string()))?
            .to_be_bytes(),
    );
    header.extend_from_slice(
        &u16::try_from(logical_content_type.len())
            .map_err(|_| {
                CryptoError::InvalidRequest("logical_content_type length overflow".to_string())
            })?
            .to_be_bytes(),
    );
    header.extend_from_slice(
        &u16::try_from(wrapped_data_key.len())
            .map_err(|_| CryptoError::InvalidRequest("wrapped key length overflow".to_string()))?
            .to_be_bytes(),
    );
    header.extend_from_slice(
        &u32::try_from(metadata.chunk_plaintext_bytes)
            .map_err(|_| CryptoError::InvalidRequest("chunk size overflow".to_string()))?
            .to_be_bytes(),
    );
    header.extend_from_slice(&metadata.plaintext_size.to_be_bytes());
    header.extend_from_slice(chunk_nonce_prefix);
    header.extend_from_slice(key_wrap_nonce);
    header.extend_from_slice(key_id);
    header.extend_from_slice(profile_id);
    header.extend_from_slice(logical_content_type);
    header.extend_from_slice(wrapped_data_key);
    Ok(header)
}

struct ParsedEnvelopeHeader {
    metadata: EncryptedObjectMetadata,
    key_wrap_nonce: [u8; NONCE_LEN],
    chunk_nonce_prefix: [u8; CHUNK_NONCE_PREFIX_LEN],
    wrapped_data_key: Bytes,
    header_bytes: Bytes,
}

async fn read_envelope_header(
    cursor: &mut StreamCursor,
) -> Result<ParsedEnvelopeHeader, CryptoError> {
    let fixed = cursor.read_exact(ENVELOPE_FIXED_HEADER_LEN).await?;
    if &fixed[..ENVELOPE_MAGIC.len()] != ENVELOPE_MAGIC {
        return Err(CryptoError::InvalidObject(
            "missing CCBGENC1 envelope header".to_string(),
        ));
    }

    let header_len = u32::from_be_bytes(fixed[8..12].try_into().expect("header length"));
    if header_len < ENVELOPE_FIXED_HEADER_LEN as u32 {
        return Err(CryptoError::InvalidObject(format!(
            "encrypted object header length {} is smaller than fixed prefix {}",
            header_len, ENVELOPE_FIXED_HEADER_LEN
        )));
    }

    let algorithm = RuntimeAlgorithm::from_id(fixed[12])?;
    let key_id_len = u16::from_be_bytes(fixed[14..16].try_into().expect("key id length")) as usize;
    let profile_id_len =
        u16::from_be_bytes(fixed[16..18].try_into().expect("profile id length")) as usize;
    let content_type_len =
        u16::from_be_bytes(fixed[18..20].try_into().expect("content-type length")) as usize;
    let wrapped_key_len =
        u16::from_be_bytes(fixed[20..22].try_into().expect("wrapped key length")) as usize;
    let chunk_plaintext_bytes =
        u32::from_be_bytes(fixed[22..26].try_into().expect("chunk size")) as u64;
    let plaintext_size = u64::from_be_bytes(fixed[26..34].try_into().expect("plaintext size"));
    let mut chunk_nonce_prefix = [0u8; CHUNK_NONCE_PREFIX_LEN];
    chunk_nonce_prefix.copy_from_slice(&fixed[34..38]);
    let mut key_wrap_nonce = [0u8; NONCE_LEN];
    key_wrap_nonce.copy_from_slice(&fixed[38..50]);

    let variable_len = header_len as usize - ENVELOPE_FIXED_HEADER_LEN;
    let variable = cursor.read_exact(variable_len).await?;
    if variable.len() != key_id_len + profile_id_len + content_type_len + wrapped_key_len {
        return Err(CryptoError::InvalidObject(format!(
            "header payload length {} does not match encoded field sizes {}",
            variable.len(),
            key_id_len + profile_id_len + content_type_len + wrapped_key_len
        )));
    }

    let key_id_end = key_id_len;
    let profile_id_end = key_id_end + profile_id_len;
    let content_type_end = profile_id_end + content_type_len;
    let wrapped_key_end = content_type_end + wrapped_key_len;

    let key_id = std::str::from_utf8(&variable[..key_id_end]).map_err(|error| {
        CryptoError::InvalidObject(format!("key_id is not valid UTF-8: {error}"))
    })?;
    let profile_id =
        std::str::from_utf8(&variable[key_id_end..profile_id_end]).map_err(|error| {
            CryptoError::InvalidObject(format!("profile_id is not valid UTF-8: {error}"))
        })?;
    let logical_content_type = if content_type_len == 0 {
        None
    } else {
        Some(
            std::str::from_utf8(&variable[profile_id_end..content_type_end])
                .map_err(|error| {
                    CryptoError::InvalidObject(format!(
                        "logical_content_type is not valid UTF-8: {error}"
                    ))
                })?
                .trim()
                .to_string(),
        )
    };
    let wrapped_data_key = variable[content_type_end..wrapped_key_end].to_vec();
    if wrapped_data_key.len() < AEAD_TAG_LEN {
        return Err(CryptoError::InvalidObject(
            "wrapped data key is too short".to_string(),
        ));
    }

    let mut header = Vec::with_capacity(header_len as usize);
    header.extend_from_slice(fixed.as_ref());
    header.extend_from_slice(variable.as_ref());
    let metadata = EncryptRequest {
        profile_id: profile_id.to_string(),
        key_id: key_id.to_string(),
        algorithm_preference: match algorithm {
            RuntimeAlgorithm::Aes256Gcm => AlgorithmPreference::Aes256Gcm,
            RuntimeAlgorithm::ChaCha20Poly1305 => AlgorithmPreference::ChaCha20Poly1305,
        },
        chunk_plaintext_bytes,
        plaintext_size,
        logical_content_type,
    }
    .planned_metadata()?;

    Ok(ParsedEnvelopeHeader {
        metadata,
        key_wrap_nonce,
        chunk_nonce_prefix,
        wrapped_data_key: Bytes::from(wrapped_data_key),
        header_bytes: Bytes::from(header),
    })
}

fn chunk_nonce(prefix: [u8; CHUNK_NONCE_PREFIX_LEN], index: u64) -> [u8; NONCE_LEN] {
    let mut nonce = [0u8; NONCE_LEN];
    nonce[..CHUNK_NONCE_PREFIX_LEN].copy_from_slice(&prefix);
    nonce[CHUNK_NONCE_PREFIX_LEN..].copy_from_slice(&index.to_be_bytes());
    nonce
}

enum CipherInstance {
    Aes256Gcm(Aes256Gcm),
    ChaCha20Poly1305(ChaCha20Poly1305),
}

impl CipherInstance {
    fn new(algorithm: RuntimeAlgorithm, key: &[u8; DATA_KEY_LEN]) -> Result<Self, CryptoError> {
        match algorithm {
            RuntimeAlgorithm::Aes256Gcm => Ok(Self::Aes256Gcm(
                Aes256Gcm::new_from_slice(key).map_err(|error| {
                    CryptoError::InvalidRequest(format!(
                        "failed to construct AES-256-GCM cipher: {error}"
                    ))
                })?,
            )),
            RuntimeAlgorithm::ChaCha20Poly1305 => Ok(Self::ChaCha20Poly1305(
                ChaCha20Poly1305::new_from_slice(key).map_err(|error| {
                    CryptoError::InvalidRequest(format!(
                        "failed to construct ChaCha20-Poly1305 cipher: {error}"
                    ))
                })?,
            )),
        }
    }

    fn encrypt(
        &self,
        nonce: &[u8; NONCE_LEN],
        aad: &[u8],
        plaintext: &[u8],
        label: &str,
    ) -> Result<Vec<u8>, CryptoError> {
        let mut buffer = plaintext.to_vec();
        let nonce = GenericArray::clone_from_slice(nonce);
        let tag =
            match self {
                Self::Aes256Gcm(cipher) => cipher
                    .encrypt_in_place_detached(&nonce, aad, &mut buffer)
                    .map_err(|error| CryptoError::KeyWrap(format!("{label} failed: {error}")))?,
                Self::ChaCha20Poly1305(cipher) => cipher
                    .encrypt_in_place_detached(&nonce, aad, &mut buffer)
                    .map_err(|error| CryptoError::KeyWrap(format!("{label} failed: {error}")))?,
            };
        buffer.extend_from_slice(tag.as_slice());
        Ok(buffer)
    }

    fn decrypt(
        &self,
        nonce: &[u8; NONCE_LEN],
        aad: &[u8],
        ciphertext: &[u8],
        label: &str,
    ) -> Result<Vec<u8>, CryptoError> {
        if ciphertext.len() < AEAD_TAG_LEN {
            return Err(CryptoError::InvalidObject(format!(
                "{label} is shorter than the authentication tag"
            )));
        }
        let split_at = ciphertext.len() - AEAD_TAG_LEN;
        let mut buffer = ciphertext[..split_at].to_vec();
        let tag = GenericArray::clone_from_slice(&ciphertext[split_at..]);
        let nonce = GenericArray::clone_from_slice(nonce);
        match self {
            Self::Aes256Gcm(cipher) => cipher
                .decrypt_in_place_detached(&nonce, aad, &mut buffer, &tag)
                .map_err(|error| CryptoError::KeyUnwrap(format!("{label} failed: {error}")))?,
            Self::ChaCha20Poly1305(cipher) => cipher
                .decrypt_in_place_detached(&nonce, aad, &mut buffer, &tag)
                .map_err(|error| CryptoError::KeyUnwrap(format!("{label} failed: {error}")))?,
        }
        Ok(buffer)
    }
}

struct PlaintextChunker {
    inner: ObjectBodyStream,
    pending: BytesMut,
}

impl PlaintextChunker {
    fn new(inner: ObjectBodyStream) -> Self {
        Self {
            inner,
            pending: BytesMut::new(),
        }
    }

    async fn next_chunk(&mut self, max_len: usize) -> Result<Option<Vec<u8>>, BlobError> {
        if max_len == 0 {
            return Err(BlobError::BodyStream(
                "plaintext chunk size must be greater than zero".to_string(),
            ));
        }

        loop {
            if self.pending.len() >= max_len {
                return Ok(Some(self.pending.split_to(max_len).to_vec()));
            }

            match self.inner.next().await {
                Some(Ok(bytes)) => self.pending.extend_from_slice(bytes.as_ref()),
                Some(Err(error)) => return Err(error),
                None => {
                    if self.pending.is_empty() {
                        return Ok(None);
                    }
                    let final_chunk = self.pending.split().to_vec();
                    return Ok(Some(final_chunk));
                }
            }
        }
    }
}

struct StreamCursor {
    inner: ObjectBodyStream,
    pending: BytesMut,
}

impl StreamCursor {
    fn new(inner: ObjectBodyStream) -> Self {
        Self {
            inner,
            pending: BytesMut::new(),
        }
    }

    async fn read_exact(&mut self, len: usize) -> Result<Bytes, CryptoError> {
        while self.pending.len() < len {
            match self.inner.next().await {
                Some(Ok(bytes)) => self.pending.extend_from_slice(bytes.as_ref()),
                Some(Err(error)) => return Err(CryptoError::BodyStream(error.to_string())),
                None => {
                    return Err(CryptoError::InvalidObject(format!(
                        "unexpected EOF while reading {} encrypted bytes",
                        len
                    )));
                }
            }
        }
        Ok(self.pending.split_to(len).freeze())
    }

    async fn ensure_eof(&mut self) -> Result<(), CryptoError> {
        if !self.pending.is_empty() {
            return Err(CryptoError::InvalidObject(
                "trailing encrypted bytes remain after plaintext completed".to_string(),
            ));
        }
        while let Some(item) = self.inner.next().await {
            let bytes = item.map_err(|error| CryptoError::BodyStream(error.to_string()))?;
            if !bytes.is_empty() {
                return Err(CryptoError::InvalidObject(
                    "trailing encrypted bytes remain after plaintext completed".to_string(),
                ));
            }
        }
        Ok(())
    }
}

struct EncryptStreamState {
    header: Option<Bytes>,
    header_aad: Bytes,
    chunker: PlaintextChunker,
    cipher: CipherInstance,
    chunk_plaintext_bytes: usize,
    chunk_nonce_prefix: [u8; CHUNK_NONCE_PREFIX_LEN],
    next_chunk_index: u64,
}

struct DecryptStreamState {
    cursor: StreamCursor,
    cipher: CipherInstance,
    header_aad: Bytes,
    chunk_nonce_prefix: [u8; CHUNK_NONCE_PREFIX_LEN],
    next_chunk_index: u64,
    remaining_plaintext_bytes: u64,
}

#[cfg(test)]
mod tests {
    use blob_core::ObjectBody;
    use bytes::Bytes;
    use futures_util::stream;

    use super::{
        AlgorithmPreference, EncryptRequest, STORED_ENCRYPTED_CONTENT_TYPE, encrypt_upload,
        prepare_decrypt_download,
    };

    #[tokio::test]
    async fn encrypted_upload_round_trips_through_streaming_download() {
        let request = EncryptRequest {
            profile_id: "router-default".to_string(),
            key_id: "kek-2026-01".to_string(),
            algorithm_preference: AlgorithmPreference::ChaCha20Poly1305,
            chunk_plaintext_bytes: 5,
            plaintext_size: 19,
            logical_content_type: Some("text/plain".to_string()),
        };
        let plaintext = b"hello streaming enc".to_vec();
        let upload = encrypt_upload(
            ObjectBody::from_stream(stream::iter([
                Ok(Bytes::from_static(b"hello ")),
                Ok(Bytes::from_static(b"streaming ")),
                Ok(Bytes::from_static(b"enc")),
            ])),
            request,
            [0x22; 32],
        )
        .expect("encryption should succeed");
        assert_eq!(
            upload.metadata.logical_content_type.as_deref(),
            Some("text/plain")
        );
        assert!(upload.metadata.stored_size > plaintext.len() as u64);

        let encrypted_bytes = upload
            .body
            .collect()
            .await
            .expect("encrypted body should collect");
        assert_ne!(encrypted_bytes.as_ref(), plaintext.as_slice());
        assert!(encrypted_bytes.starts_with(b"CCBGENC1"));
        assert_eq!(
            STORED_ENCRYPTED_CONTENT_TYPE,
            "application/vnd.ccbg.encrypted"
        );

        let prepared = prepare_decrypt_download(encrypted_bytes.into(), [0x22; 32])
            .await
            .expect("decryption should prepare");
        let decrypted = prepared
            .body
            .collect()
            .await
            .expect("decrypted body should collect");
        assert_eq!(decrypted.as_ref(), plaintext.as_slice());
        assert_eq!(
            prepared.metadata.logical_content_type.as_deref(),
            Some("text/plain")
        );
        assert_eq!(prepared.metadata.plaintext_size, plaintext.len() as u64);
    }
}
