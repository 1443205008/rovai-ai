//! Typed, transport-neutral authentication fixtures for remote Workers.
//!
//! This module deliberately does not open a socket, spawn a process, resolve a
//! filesystem path, or carry a credential value.  The three opaque values are
//! validated at the boundary and the in-memory fixture uses a lease token only
//! to calculate a proof.  The token itself is never a field of the envelope
//! and is therefore never serialized onto a transport.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha2::{Digest, Sha256};

const MAX_TYPED_VALUE_LEN: usize = 512;
const ENVELOPE_PROTOCOL_VERSION: u32 = 1;
const FIXTURE_DOMAIN: &[u8] = b"rovai.remote-worker.authenticated-envelope.fixture.v1";

/// Errors returned when a transport identifier is not an opaque, bounded
/// value.  Path separators and shell metacharacters are rejected so callers
/// cannot smuggle a path or command through a field that is meant to be an ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransportValueError {
    Empty,
    Whitespace,
    TooLong,
    NonAscii,
    Control,
    PathLike,
    ShellLike,
}

impl TransportValueError {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Empty => "transport_value_required",
            Self::Whitespace => "transport_value_whitespace",
            Self::TooLong => "transport_value_too_long",
            Self::NonAscii => "transport_value_non_ascii",
            Self::Control => "transport_value_control",
            Self::PathLike => "transport_value_path_like",
            Self::ShellLike => "transport_value_shell_like",
        }
    }
}

impl fmt::Display for TransportValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for TransportValueError {}

fn validate_opaque(value: &str) -> Result<(), TransportValueError> {
    if value.is_empty() {
        return Err(TransportValueError::Empty);
    }
    if value.trim() != value {
        return Err(TransportValueError::Whitespace);
    }
    if value.len() > MAX_TYPED_VALUE_LEN {
        return Err(TransportValueError::TooLong);
    }
    if !value.is_ascii() {
        return Err(TransportValueError::NonAscii);
    }
    if value.chars().any(char::is_control) {
        return Err(TransportValueError::Control);
    }
    if value == "." || value == ".." || value.contains('/') || value.contains('\\') {
        return Err(TransportValueError::PathLike);
    }
    // IDs may contain the punctuation used by digests and versioned IDs, but
    // no shell operators, quotes, expansion markers, or whitespace.
    if value
        .chars()
        .any(|ch| !(ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':' | '@')))
    {
        return Err(TransportValueError::ShellLike);
    }
    Ok(())
}

macro_rules! typed_transport_value {
    ($name:ident, $debug_name:literal) => {
        #[derive(Clone, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl AsRef<str>) -> Result<Self, TransportValueError> {
                let value = value.as_ref();
                validate_opaque(value)?;
                Ok(Self(value.to_owned()))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl TryFrom<&str> for $name {
            type Error = TransportValueError;

            fn try_from(value: &str) -> Result<Self, Self::Error> {
                Self::parse(value)
            }
        }

        impl TryFrom<String> for $name {
            type Error = TransportValueError;

            fn try_from(value: String) -> Result<Self, Self::Error> {
                Self::parse(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(serde::de::Error::custom)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_tuple($debug_name).field(&"<redacted>").finish()
            }
        }
    };
}

/// One-time pairing challenge.  This is an opaque ID, not a credential.
typed_transport_value!(RegistrationNonce, "RegistrationNonce");
/// Stable reference to a machine credential.  The credential material never
/// enters this value or an [`AuthenticatedEnvelope`].
typed_transport_value!(CredentialId, "CredentialId");
/// Core-issued task lease proof input.  It is consumed only by the fixture's
/// local proof calculation and is intentionally absent from the wire shape.
typed_transport_value!(LeaseToken, "LeaseToken");

/// Hex encoded proof emitted by the fixture.  It authenticates the envelope's
/// metadata and payload for tests; it is not a production network handshake.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EnvelopeProof(String);

impl EnvelopeProof {
    fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest.iter().map(|byte| format!("{byte:02x}")).collect())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A serializable authenticated envelope fixture.
///
/// `lease_token` is deliberately not a field.  `fixture` and `verify` accept
/// it by reference to calculate/compare the proof, while serialized data only
/// contains the credential ID, nonce, metadata and payload.  This makes it
/// impossible for this type to transmit credential material accidentally.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AuthenticatedEnvelope<T> {
    pub protocol_version: u32,
    pub worker_id: String,
    pub message_id: String,
    pub nonce: RegistrationNonce,
    pub credential_id: CredentialId,
    pub payload: T,
    pub proof: EnvelopeProof,
}

#[derive(Debug, Eq, PartialEq)]
pub enum EnvelopeError {
    InvalidWorkerId(TransportValueError),
    InvalidMessageId(TransportValueError),
    InvalidNonce(TransportValueError),
    InvalidCredentialId(TransportValueError),
    InvalidLeaseToken(TransportValueError),
    PayloadSerialization,
    ProofMismatch,
}

impl fmt::Display for EnvelopeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidWorkerId(error) => write!(f, "worker_id_invalid:{error}"),
            Self::InvalidMessageId(error) => write!(f, "message_id_invalid:{error}"),
            Self::InvalidNonce(error) => write!(f, "nonce_invalid:{error}"),
            Self::InvalidCredentialId(error) => write!(f, "credential_id_invalid:{error}"),
            Self::InvalidLeaseToken(error) => write!(f, "lease_token_invalid:{error}"),
            Self::PayloadSerialization => f.write_str("payload_serialization_failed"),
            Self::ProofMismatch => f.write_str("envelope_proof_mismatch"),
        }
    }
}

impl std::error::Error for EnvelopeError {}

fn update_component(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

fn proof_for<T: Serialize>(
    worker_id: &str,
    message_id: &str,
    nonce: &RegistrationNonce,
    credential_id: &CredentialId,
    payload: &T,
    lease_token: &LeaseToken,
) -> Result<EnvelopeProof, EnvelopeError> {
    let payload = serde_json::to_vec(payload).map_err(|_| EnvelopeError::PayloadSerialization)?;
    let mut hasher = Sha256::new();
    hasher.update(FIXTURE_DOMAIN);
    update_component(&mut hasher, worker_id.as_bytes());
    update_component(&mut hasher, message_id.as_bytes());
    update_component(&mut hasher, nonce.as_str().as_bytes());
    update_component(&mut hasher, credential_id.as_str().as_bytes());
    update_component(&mut hasher, &payload);
    update_component(&mut hasher, lease_token.as_str().as_bytes());
    Ok(EnvelopeProof::from_digest(hasher.finalize().into()))
}

impl<T: Serialize> AuthenticatedEnvelope<T> {
    /// Build an in-memory authenticated fixture.  No network or process is
    /// contacted, and the lease token is not retained after proof creation.
    pub fn fixture(
        worker_id: impl AsRef<str>,
        message_id: impl AsRef<str>,
        nonce: RegistrationNonce,
        credential_id: CredentialId,
        lease_token: &LeaseToken,
        payload: T,
    ) -> Result<Self, EnvelopeError> {
        let worker_id = worker_id.as_ref();
        let message_id = message_id.as_ref();
        validate_opaque(worker_id).map_err(EnvelopeError::InvalidWorkerId)?;
        validate_opaque(message_id).map_err(EnvelopeError::InvalidMessageId)?;
        // Re-validate values even though typed constructors already enforce
        // the invariant; this keeps the boundary explicit for future changes.
        RegistrationNonce::parse(nonce.as_str()).map_err(EnvelopeError::InvalidNonce)?;
        CredentialId::parse(credential_id.as_str()).map_err(EnvelopeError::InvalidCredentialId)?;
        LeaseToken::parse(lease_token.as_str()).map_err(EnvelopeError::InvalidLeaseToken)?;
        let proof = proof_for(
            worker_id,
            message_id,
            &nonce,
            &credential_id,
            &payload,
            lease_token,
        )?;
        Ok(Self {
            protocol_version: ENVELOPE_PROTOCOL_VERSION,
            worker_id: worker_id.to_owned(),
            message_id: message_id.to_owned(),
            nonce,
            credential_id,
            payload,
            proof,
        })
    }

    /// Alias that makes the test-only nature explicit at call sites.
    pub fn new_fixture(
        worker_id: impl AsRef<str>,
        message_id: impl AsRef<str>,
        nonce: RegistrationNonce,
        credential_id: CredentialId,
        lease_token: &LeaseToken,
        payload: T,
    ) -> Result<Self, EnvelopeError> {
        Self::fixture(
            worker_id,
            message_id,
            nonce,
            credential_id,
            lease_token,
            payload,
        )
    }

    pub fn verify(&self, lease_token: &LeaseToken) -> Result<(), EnvelopeError> {
        if self.protocol_version != ENVELOPE_PROTOCOL_VERSION {
            return Err(EnvelopeError::ProofMismatch);
        }
        LeaseToken::parse(lease_token.as_str()).map_err(EnvelopeError::InvalidLeaseToken)?;
        let expected = proof_for(
            &self.worker_id,
            &self.message_id,
            &self.nonce,
            &self.credential_id,
            &self.payload,
            lease_token,
        )?;
        if expected == self.proof {
            Ok(())
        } else {
            Err(EnvelopeError::ProofMismatch)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_values_accept_ids_and_reject_paths_or_shell() {
        assert_eq!(
            RegistrationNonce::parse("nonce-1").unwrap().as_str(),
            "nonce-1"
        );
        assert!(matches!(
            CredentialId::parse("/tmp/credential"),
            Err(TransportValueError::PathLike)
        ));
        assert!(matches!(
            LeaseToken::parse("lease;rm"),
            Err(TransportValueError::ShellLike)
        ));
        assert!(matches!(
            LeaseToken::parse(" lease"),
            Err(TransportValueError::Whitespace)
        ));
    }

    #[test]
    fn typed_values_round_trip_through_json_with_validation() {
        let nonce = RegistrationNonce::parse("nonce-1").unwrap();
        let json = serde_json::to_string(&nonce).unwrap();
        assert_eq!(json, "\"nonce-1\"");
        assert_eq!(
            serde_json::from_str::<RegistrationNonce>(&json).unwrap(),
            nonce
        );
        assert!(serde_json::from_str::<LeaseToken>("\"lease\\n\"").is_err());
    }

    #[test]
    fn fixture_serialization_never_contains_lease_token() {
        let nonce = RegistrationNonce::parse("nonce-1").unwrap();
        let credential = CredentialId::parse("machine-credential-1").unwrap();
        let lease = LeaseToken::parse("lease-secret").unwrap();
        let envelope = AuthenticatedEnvelope::fixture(
            "worker-1",
            "message-1",
            nonce,
            credential,
            &lease,
            serde_json::json!({"kind":"heartbeat"}),
        )
        .unwrap();
        let wire = serde_json::to_string(&envelope).unwrap();
        assert!(wire.contains("machine-credential-1"));
        assert!(!wire.contains("lease-secret"));
    }

    #[test]
    fn fixture_verifies_and_detects_tampering() {
        let nonce = RegistrationNonce::parse("nonce-1").unwrap();
        let credential = CredentialId::parse("credential-1").unwrap();
        let lease = LeaseToken::parse("lease-1").unwrap();
        let mut envelope = AuthenticatedEnvelope::fixture(
            "worker-1",
            "message-1",
            nonce,
            credential,
            &lease,
            "heartbeat",
        )
        .unwrap();
        assert!(envelope.verify(&lease).is_ok());
        envelope.message_id = "message-2".to_owned();
        assert_eq!(envelope.verify(&lease), Err(EnvelopeError::ProofMismatch));
    }

    #[test]
    fn wrong_lease_token_does_not_verify() {
        let nonce = RegistrationNonce::parse("nonce-1").unwrap();
        let credential = CredentialId::parse("credential-1").unwrap();
        let lease = LeaseToken::parse("lease-1").unwrap();
        let other_lease = LeaseToken::parse("lease-2").unwrap();
        let envelope = AuthenticatedEnvelope::new_fixture(
            "worker-1",
            "message-1",
            nonce,
            credential,
            &lease,
            "heartbeat",
        )
        .unwrap();
        assert_eq!(
            envelope.verify(&other_lease),
            Err(EnvelopeError::ProofMismatch)
        );
    }
}
