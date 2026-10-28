//! Authentication and device-backed P-256 signing.

use std::{fmt, sync::Arc};

use async_trait::async_trait;
use rsp_transport::identity::ExternalSigner;
use russh::{client, keys::PrivateKeyWithHashAlg, MethodKind};
use ssh_key::{private::Ed25519Keypair, public::EcdsaPublicKey, PrivateKey, PublicKey};
use x509_parser::{
    oid_registry::{OID_EC_P256, OID_KEY_TYPE_EC_PUBLIC_KEY},
    prelude::*,
};
use zeroize::Zeroizing;

use crate::SshError;

/// Secrets are erased on drop and never included in `Debug` output.
pub enum Credential {
    External(Arc<dyn ExternalSigner>),
    Ed25519 { secret: Zeroizing<[u8; 32]> },
    Password(Zeroizing<String>),
    KeyboardInteractive,
}

impl fmt::Debug for Credential {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::External(_) => "External([REDACTED])",
            Self::Ed25519 { .. } => "Ed25519([REDACTED])",
            Self::Password(_) => "Password([REDACTED])",
            Self::KeyboardInteractive => "KeyboardInteractive",
        })
    }
}

impl Credential {
    /// Import an unencrypted OpenSSH Ed25519 key. RSA is deliberately unsupported.
    /// Encrypted key files must be decrypted by the platform's secret store first.
    pub fn from_openssh(pem: &str) -> Result<Self, SshError> {
        let key = PrivateKey::from_openssh(pem).map_err(|_| SshError::InvalidKey)?;
        if key.algorithm().is_rsa() {
            return Err(SshError::Unsupported("RSA user keys are not supported"));
        }
        match key.key_data().ed25519() {
            Some(key) => Ok(Self::Ed25519 {
                secret: Zeroizing::new(key.private.to_bytes()),
            }),
            None => Err(SshError::Unsupported(
                "only Ed25519 user key files are supported; use External for P-256",
            )),
        }
    }

    fn method(&self) -> MethodKind {
        match self {
            Self::External(_) | Self::Ed25519 { .. } => MethodKind::PublicKey,
            Self::Password(_) => MethodKind::Password,
            Self::KeyboardInteractive => MethodKind::KeyboardInteractive,
        }
    }
}

/// UI callbacks. `None` cancels a keyboard-interactive exchange and disconnects.
/// Password prompts use the same `keyboard_interactive` callback.
#[async_trait]
pub trait Prompter: Send + Sync {
    async fn confirm_new_host_key(&self, host: &str, fp_sha256: &str, algo: &str) -> bool;
    async fn keyboard_interactive(
        &self,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<String>>;
    fn auth_url(&self, url: &str);
}

/// OpenSSH authorized_keys representation, suitable for the App's install-key flow.
pub fn public_key_openssh(cred: &Credential, comment: &str) -> Result<String, SshError> {
    if comment.contains(['\r', '\n', '\0']) {
        return Err(SshError::InvalidOptions("invalid public key comment"));
    }
    let mut key = match cred {
        Credential::External(signer) => external_public_key(signer.as_ref())?,
        Credential::Ed25519 { secret } => ed25519_private_key(secret)?.public_key().clone(),
        _ => {
            return Err(SshError::Unsupported(
                "credential does not contain a public key",
            ))
        }
    };
    key.set_comment(comment);
    key.to_openssh().map_err(|_| SshError::InvalidKey)
}

fn ed25519_private_key(secret: &[u8; 32]) -> Result<PrivateKey, SshError> {
    PrivateKey::new(Ed25519Keypair::from_seed(secret).into(), "").map_err(|_| SshError::InvalidKey)
}

fn external_public_key(signer: &dyn ExternalSigner) -> Result<PublicKey, SshError> {
    let der = signer.spki_der();
    let (rest, spki) = SubjectPublicKeyInfo::from_der(&der).map_err(|_| SshError::InvalidKey)?;
    if !rest.is_empty() {
        return Err(SshError::InvalidKey);
    }
    if spki.algorithm.algorithm != OID_KEY_TYPE_EC_PUBLIC_KEY
        || spki
            .algorithm
            .parameters
            .as_ref()
            .and_then(|p| p.as_oid().ok())
            != Some(OID_EC_P256)
    {
        return Err(SshError::Unsupported(
            "external keys must be ECDSA P-256; RSA is not supported",
        ));
    }
    if spki.subject_public_key.unused_bits != 0 {
        return Err(SshError::InvalidKey);
    }
    // Validate the point, not just its SEC1 encoding length.
    p256::PublicKey::from_sec1_bytes(&spki.subject_public_key.data)
        .map_err(|_| SshError::InvalidKey)?;
    let key = EcdsaPublicKey::from_sec1_bytes(&spki.subject_public_key.data)
        .map_err(|_| SshError::InvalidKey)?;
    Ok(PublicKey::new(key.into(), ""))
}

pub(crate) async fn authenticate(
    handle: &mut client::Handle<crate::ClientHandler>,
    user: &str,
    creds: &[Credential],
    prompter: &dyn Prompter,
) -> Result<(), SshError> {
    // Probe once to learn the server's advertised methods. It may accept "none"
    // (Tailscale SSH does this while waiting for browser approval).
    let mut methods = match handle.authenticate_none(user).await? {
        client::AuthResult::Success => return Ok(()),
        client::AuthResult::Failure {
            remaining_methods, ..
        } => remaining_methods,
    };
    let mut tried = Vec::new();
    // Some SSH servers advertise only `password`, while the caller may have
    // supplied only the interactive credential. Defer that prompt until all
    // other credentials have had a chance to authenticate.
    let mut deferred_password = false;
    let mut keyboard_interactive_offered = methods.contains(&MethodKind::KeyboardInteractive);
    for cred in creds {
        let method = cred.method();
        if !methods.contains(&method) {
            if matches!(cred, Credential::KeyboardInteractive)
                && !keyboard_interactive_offered
                && methods.contains(&MethodKind::Password)
            {
                deferred_password = true;
            }
            continue;
        }
        tried.push(<&'static str>::from(&method));
        let result = match cred {
            Credential::Password(password) => {
                handle
                    .authenticate_password(user, password.as_str())
                    .await?
            }
            Credential::Ed25519 { secret } => {
                handle
                    .authenticate_publickey(
                        user,
                        PrivateKeyWithHashAlg::new(Arc::new(ed25519_private_key(secret)?), None),
                    )
                    .await?
            }
            Credential::External(signer) => {
                let public = external_public_key(signer.as_ref())?;
                handle
                    .authenticate_publickey_with(
                        user,
                        public,
                        None,
                        &mut DeviceSigner(signer.clone()),
                    )
                    .await?
            }
            Credential::KeyboardInteractive => {
                let mut response = handle
                    .authenticate_keyboard_interactive_start(user, None)
                    .await?;
                loop {
                    match response {
                        client::KeyboardInteractiveAuthResponse::Success => return Ok(()),
                        client::KeyboardInteractiveAuthResponse::Failure {
                            remaining_methods,
                            partial_success,
                        } => {
                            break client::AuthResult::Failure {
                                remaining_methods,
                                partial_success,
                            };
                        }
                        client::KeyboardInteractiveAuthResponse::InfoRequest {
                            name,
                            instructions,
                            prompts,
                        } => {
                            let prompts: Vec<_> =
                                prompts.into_iter().map(|p| (p.prompt, p.echo)).collect();
                            let answers = prompter
                                .keyboard_interactive(&name, &instructions, &prompts)
                                .await
                                .ok_or(SshError::Cancelled)?;
                            if answers.len() != prompts.len() {
                                return Err(SshError::InvalidOptions(
                                    "keyboard-interactive answer count does not match prompts",
                                ));
                            }
                            // russh takes ownership to encode the protocol response; never log it.
                            response = handle
                                .authenticate_keyboard_interactive_respond(answers)
                                .await?;
                        }
                    }
                }
            }
        };
        match result {
            client::AuthResult::Success => return Ok(()),
            client::AuthResult::Failure {
                remaining_methods, ..
            } => {
                keyboard_interactive_offered |=
                    remaining_methods.contains(&MethodKind::KeyboardInteractive);
                methods = remaining_methods;
            }
        }
    }
    if deferred_password && !keyboard_interactive_offered && methods.contains(&MethodKind::Password)
    {
        // Like a rejected keyboard-interactive exchange, this gets one attempt
        // per connection; a wrong password does not trigger another prompt.
        let answers = prompter
            .keyboard_interactive("", "", &[("Password:".into(), false)])
            .await
            .ok_or(SshError::Cancelled)?;
        let [password]: [String; 1] = answers.try_into().map_err(|_| {
            SshError::InvalidOptions("keyboard-interactive answer count does not match prompts")
        })?;
        let password = Zeroizing::new(password);
        tried.push("password");
        if handle
            .authenticate_password(user, password.as_str())
            .await?
            .success()
        {
            return Ok(());
        }
    }
    Err(SshError::AuthFailed { tried })
}

struct DeviceSigner(Arc<dyn ExternalSigner>);

impl russh::Signer for DeviceSigner {
    type Error = SshError;

    async fn auth_sign(
        &mut self,
        _key: &russh::keys::agent::AgentIdentity,
        _hash_alg: Option<ssh_key::HashAlg>,
        mut to_sign: Vec<u8>,
    ) -> Result<Vec<u8>, SshError> {
        let signer = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let der = signer.sign(&to_sign).map_err(|_| SshError::SigningFailed)?;
            let signature =
                p256::ecdsa::Signature::from_der(&der).map_err(|_| SshError::SigningFailed)?;
            let (r, s) = signature.split_bytes();
            let mut scalars = Vec::with_capacity(74);
            put_mpint(&mut scalars, &r);
            put_mpint(&mut scalars, &s);
            let mut encoded = Vec::with_capacity(104);
            put_string(&mut encoded, b"ecdsa-sha2-nistp256");
            put_string(&mut encoded, &scalars);
            // russh's Signer contract appends the SSH signature to the original
            // session-id + USERAUTH_REQUEST buffer, as an SSH agent would.
            put_string(&mut to_sign, &encoded);
            Ok(to_sign)
        })
        .await
        .map_err(|_| SshError::SigningFailed)?
    }
}

fn put_string(out: &mut Vec<u8>, value: &[u8]) {
    out.extend_from_slice(&(value.len() as u32).to_be_bytes());
    out.extend_from_slice(value);
}

fn put_mpint(out: &mut Vec<u8>, scalar: &[u8]) {
    let scalar = scalar
        .iter()
        .position(|v| *v != 0)
        .map_or(&[][..], |i| &scalar[i..]);
    let leading_zero = scalar.first().is_some_and(|v| v & 0x80 != 0);
    out.extend_from_slice(&((scalar.len() + usize::from(leading_zero)) as u32).to_be_bytes());
    if leading_zero {
        out.push(0);
    }
    out.extend_from_slice(scalar);
}
