use serde::Serialize;
use sha2::{Digest, Sha256 as Sha256Hasher};

use super::{RepositoryError, RequestEnvelope, Sha256};

const DOMAIN: &[u8] = b"cutex/role-seat-request-digest/v1\0";

#[derive(Serialize)]
struct CanonicalRequest<'a> {
    schema: super::super::RequestSchema,
    request_id: &'a super::super::RequestId,
    expected_store_revision: super::super::StoreRevision,
    request: &'a super::super::MutationRequest,
}

pub(super) fn canonical_request_digest(
    envelope: &RequestEnvelope,
) -> Result<Sha256, RepositoryError> {
    let material = CanonicalRequest {
        schema: envelope.schema,
        request_id: &envelope.request_id,
        expected_store_revision: envelope.expected_store_revision,
        request: &envelope.request,
    };
    let bytes = serde_json::to_vec(&material).map_err(|_| RepositoryError::Serialization)?;
    let mut hasher = Sha256Hasher::new();
    hasher.update(DOMAIN);
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    Sha256::new(encoded).map_err(|_| RepositoryError::Serialization)
}
