// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Native OpenSSH key generation. Only encrypted private keys reach disk;
//! passphrases stay in the existing in-memory account session.
use super::super::{Result, failure};
use super::{
    Remote, credentials,
    model::{AuthInput, GenerateKeyInput, GeneratedKey},
};
use russh::keys::ssh_key;
use std::{io::Write, path::Path};

pub(crate) fn generate_key(
    user: &Path,
    remote: &Remote,
    request: &GenerateKeyInput,
) -> Result<GeneratedKey> {
    if !matches!(remote, Remote::Ssh(_)) {
        return Err(failure("Generate a key for an SSH destination"));
    }
    if request.name.is_empty()
        || request.name.len() > 48
        || !request
            .name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(failure(
            "Use a key name of 1–48 letters, numbers, hyphens or underscores",
        ));
    }
    if request.passphrase.trim().is_empty()
        || request.account.trim().is_empty()
        || request.account.chars().any(char::is_control)
    {
        return Err(failure(
            "An account and passphrase are required for a generated private key",
        ));
    }
    let directory = user.join("ssh");
    if directory.try_exists()?
        && std::fs::symlink_metadata(&directory)?
            .file_type()
            .is_symlink()
    {
        return Err(failure("The SSH key directory cannot be a symbolic link"));
    }
    std::fs::create_dir_all(&directory)?;
    let path = directory.join(format!("{}.key", request.name));
    let public_path = directory.join(format!("{}.key.pub", request.name));
    if path.try_exists()? || public_path.try_exists()? {
        return Err(failure("A key with that name already exists"));
    }
    // The seed, bcrypt salt and check integer come from the system generator
    // behind ring, which TLS already uses. The encryption matches OpenSSH and
    // ssh-key defaults: aes256-ctr with 16 bcrypt rounds over a 16-byte salt.
    let random = rustls::crypto::ring::default_provider().secure_random;
    let mut seed = [0; 32];
    let mut salt = vec![0; 16];
    let mut check = [0; 4];
    for bytes in [&mut seed[..], &mut salt[..], &mut check[..]] {
        random
            .fill(bytes)
            .map_err(|_| failure("SSH key generation failed"))?;
    }
    let key = ssh_key::PrivateKey::from(ssh_key::private::Ed25519Keypair::from_seed(&seed));
    let public_key = key
        .public_key()
        .to_openssh()
        .map_err(|_| failure("SSH public key encoding failed"))?;
    let fingerprint = key.fingerprint(ssh_key::HashAlg::Sha256).to_string();
    let encrypted = key
        .encrypt_with(
            ssh_key::Cipher::Aes256Ctr,
            ssh_key::Kdf::Bcrypt { salt, rounds: 16 },
            u32::from_le_bytes(check),
            request.passphrase.as_bytes(),
        )
        .map_err(|_| failure("SSH key encryption failed"))?;
    let pem = encrypted
        .to_openssh(ssh_key::LineEnding::LF)
        .map_err(|_| failure("SSH private key encoding failed"))?;
    write_new(&directory, &path, pem.as_bytes())?;
    write_new(&directory, &public_path, public_key.as_bytes())?;
    credentials::configure_auth(
        user,
        remote,
        AuthInput {
            account: request.account.clone(),
            token: None,
            private_key: Some(path.to_string_lossy().into_owned()),
            passphrase: Some(request.passphrase.clone()),
            forget: false,
            use_agent: Some(false),
        },
    )?;
    Ok(GeneratedKey {
        private_key: path.to_string_lossy().into_owned(),
        public_key,
        fingerprint,
    })
}

fn write_new(directory: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    let mut temporary = tempfile::NamedTempFile::new_in(directory)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    Ok(())
}
