// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

use super::super::{
    Result, failure,
    lfs::{self, Pointer},
};
use super::{Auth, Direction};
use reqwest::{
    blocking::{Client, RequestBuilder, Response},
    header::{HeaderMap, HeaderName, HeaderValue},
};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, Write},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Deserialize)]
pub(super) struct Endpoint {
    pub(super) href: String,
    #[serde(default)]
    pub(super) header: BTreeMap<String, String>,
    pub(super) expires_at: Option<String>,
    pub(super) expires_in: Option<i64>,
}

#[derive(Deserialize)]
struct Batch {
    transfer: Option<String>,
    hash_algo: Option<String>,
    objects: Vec<Object>,
}

#[derive(Deserialize)]
struct Object {
    oid: String,
    size: u64,
    error: Option<ObjectError>,
    #[serde(default)]
    actions: BTreeMap<String, Endpoint>,
}

#[derive(Deserialize)]
struct ObjectError {
    code: u16,
}

#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "The .git suffix belongs to a case-sensitive remote URL, not a local file extension."
)]
pub(super) fn transfer(
    repo: &git2::Repository,
    remote: &url::Url,
    auth: &Auth,
    pointers: &[Pointer],
    direction: Direction,
) -> Result<()> {
    use base64::Engine as _;
    let mut endpoint = remote.clone();
    let path = remote.path().trim_end_matches('/');
    endpoint.set_path(&format!(
        "{}{}{}",
        path,
        if path.ends_with(".git") { "" } else { ".git" },
        "/info/lfs"
    ));
    let mut header = BTreeMap::new();
    if let Some(token) = &auth.token {
        header.insert(
            "Authorization".into(),
            format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD
                    .encode(format!("{}:{token}", auth.account))
            ),
        );
    }
    transfer_endpoint(
        repo,
        auth,
        &Endpoint {
            href: endpoint.to_string(),
            header,
            expires_at: None,
            expires_in: None,
        },
        pointers,
        direction,
    )
}

pub(super) fn checked_url(value: &str) -> Result<url::Url> {
    let url = url::Url::parse(value).map_err(|_| failure("LFS supplied an invalid endpoint"))?;
    let secure = url.scheme() == "https";
    #[cfg(test)]
    let secure = secure
        || (url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")));
    if !secure
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.host_str().is_none()
    {
        return Err(failure(
            "LFS requires a secure endpoint without embedded credentials",
        ));
    }
    Ok(url)
}

fn headers(values: &BTreeMap<String, String>) -> Result<HeaderMap> {
    let mut result = HeaderMap::new();
    for (key, value) in values {
        if matches!(
            key.to_ascii_lowercase().as_str(),
            "host" | "content-length" | "transfer-encoding" | "connection"
        ) {
            return Err(failure("LFS supplied an unsafe transfer header"));
        }
        result.insert(
            HeaderName::from_bytes(key.as_bytes())
                .map_err(|_| failure("Invalid LFS header name"))?,
            HeaderValue::from_str(value).map_err(|_| failure("Invalid LFS header value"))?,
        );
    }
    Ok(result)
}

fn check_expiry(endpoint: &Endpoint, received: Instant) -> Result<()> {
    if endpoint.expires_in.is_some_and(|seconds| {
        seconds <= 0 || received.elapsed().as_secs() >= u64::try_from(seconds).unwrap_or_default()
    }) {
        return Err(failure("LFS authorization expired; retry the transfer"));
    }
    if let Some(expires) = &endpoint.expires_at {
        let expires = chrono::DateTime::parse_from_rfc3339(expires)
            .map_err(|_| failure("Invalid LFS authorization expiry"))?;
        if expires <= chrono::Utc::now() {
            return Err(failure("LFS authorization expired; retry the transfer"));
        }
    }
    Ok(())
}

fn send(request: RequestBuilder) -> Result<Response> {
    let response = request.send().map_err(|error| {
        failure("Attachment connection failed; retry when connected").classified(
            if error.is_connect() || error.is_timeout() {
                "offline"
            } else {
                "attachments_unavailable"
            },
        )
    })?;
    if !response.status().is_success() {
        return Err(service_error(response.status().as_u16()));
    }
    Ok(response)
}

fn service_error(code: u16) -> super::super::Error {
    match code {
        401 => failure("Needs sign-in: attachment credentials were rejected")
            .classified("needs_sign_in"),
        403 => {
            failure("No access: attachment service denied this operation").classified("no_access")
        }
        507 => failure("The attachment destination has insufficient storage")
            .classified("storage_full"),
        _ => failure(format!(
            "Attachment service returned HTTP {code}; local changes are preserved"
        ))
        .classified("attachments_unavailable"),
    }
}

/// Git and attachments verify servers with the operating system's trust
/// store and policy, as libgit2's native HTTPS transports did.
fn tls() -> Result<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    #[cfg(test)]
    let verifier = super::tests::trusted_certificate_der().map_or_else(
        || rustls_platform_verifier::Verifier::new(Arc::clone(&provider)),
        |der| {
            rustls_platform_verifier::Verifier::new_with_extra_roots(
                [rustls::pki_types::CertificateDer::from(der)],
                Arc::clone(&provider),
            )
        },
    );
    #[cfg(not(test))]
    let verifier = rustls_platform_verifier::Verifier::new(Arc::clone(&provider));
    let verifier = verifier.map_err(|_| failure("Unable to read the system certificate store"))?;
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| failure("Unable to initialize secure transport"))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

/// `None` leaves only the connect limit, for Git transfers that progress for
/// longer than any fixed total.
fn client(timeout: Option<Duration>) -> Result<Client> {
    Client::builder()
        .use_preconfigured_tls(tls()?)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(15))
        .timeout(timeout)
        .build()
        .map_err(|_| failure("Unable to initialize secure transport"))
}

/// The Git stream reports response failures through libgit2 as generic
/// errors. Recheck only the read-only service advertisement, never a push.
pub(super) fn diagnose_git(
    origin: &url::Url,
    auth: &Auth,
    push: bool,
) -> Option<super::super::Error> {
    let mut endpoint = origin.clone();
    endpoint.set_path(&format!(
        "{}/info/refs",
        origin.path().trim_end_matches('/')
    ));
    endpoint.set_query(Some(if push {
        "service=git-receive-pack"
    } else {
        "service=git-upload-pack"
    }));
    let mut request = client(Some(Duration::from_secs(15))).ok()?.get(endpoint);
    if let Some(token) = &auth.token {
        request = request.basic_auth(&auth.account, Some(token));
    }
    let response = match request.send() {
        Ok(response) => response,
        Err(error) if error.is_connect() || error.is_timeout() => {
            return Some(failure("Git destination could not be reached").classified("offline"));
        }
        Err(_) => return None,
    };
    match response.status().as_u16() {
        401 => Some(failure("Git credentials were rejected").classified("needs_sign_in")),
        403 => Some(
            failure("The destination denied access to this repository").classified("no_access"),
        ),
        507 => {
            Some(failure("The Git destination has insufficient storage").classified("storage_full"))
        }
        _ => None,
    }
}

pub(super) fn transfer_endpoint(
    repo: &git2::Repository,
    auth: &Auth,
    endpoint: &Endpoint,
    pointers: &[Pointer],
    direction: Direction,
) -> Result<()> {
    let started = Instant::now();
    let mut batch_url = checked_url(&endpoint.href)?;
    batch_url.set_path(&format!(
        "{}/objects/batch",
        batch_url.path().trim_end_matches('/')
    ));
    let client = client(Some(Duration::from_secs(300)))?;
    for chunk in pointers.chunks(100) {
        auth.check_cancelled()?;
        check_expiry(endpoint, started)?;
        let response = send(client.post(batch_url.clone()).headers(headers(&endpoint.header)?)
            .header("Accept", "application/vnd.git-lfs+json").header("Content-Type", "application/vnd.git-lfs+json")
            .json(&json!({"operation":direction.name(),"transfers":["basic"],"hash_algo":"sha256","ref":{"name":"refs/heads/main"},"objects":chunk.iter().map(|pointer| { let (oid,size)=pointer.fingerprint(); json!({"oid":oid,"size":size}) }).collect::<Vec<_>>()})))?;
        let mut bytes = Vec::new();
        response.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
        if bytes.len() > 8 * 1024 * 1024 {
            return Err(failure("Attachment service response is too large"));
        }
        let batch: Batch = serde_json::from_slice(&bytes)
            .map_err(|_| failure("Invalid attachment service response"))?;
        if batch
            .transfer
            .as_deref()
            .is_some_and(|value| value != "basic")
            || batch
                .hash_algo
                .as_deref()
                .is_some_and(|value| value != "sha256")
        {
            return Err(failure(
                "Attachment service selected an unsupported transfer",
            ));
        }
        let received = Instant::now();
        let mut requested: BTreeMap<_, _> = chunk
            .iter()
            .map(|pointer| (pointer.fingerprint().0, pointer))
            .collect();
        for object in batch.objects {
            auth.check_cancelled()?;
            let pointer = requested.remove(&object.oid).ok_or_else(|| {
                failure("Attachment service returned an unexpected or duplicate object")
            })?;
            if let Some(error) = &object.error {
                return Err(service_error(error.code));
            }
            if object.size != pointer.fingerprint().1 {
                return Err(failure(
                    "Attachment is unavailable or has inconsistent size",
                ));
            }
            transfer_object(&client, repo, pointer, &object, direction, received)?;
        }
        if !requested.is_empty() {
            return Err(failure("Attachment service omitted requested objects"));
        }
    }
    Ok(())
}

fn action(
    client: &Client,
    endpoint: &Endpoint,
    method: reqwest::Method,
    received: Instant,
) -> Result<RequestBuilder> {
    check_expiry(endpoint, received)?;
    Ok(client
        .request(method, checked_url(&endpoint.href)?)
        .headers(headers(&endpoint.header)?))
}

fn transfer_object(
    client: &Client,
    repo: &git2::Repository,
    pointer: &Pointer,
    object: &Object,
    direction: Direction,
    received: Instant,
) -> Result<()> {
    if let Some(endpoint) = object.actions.get(direction.name()) {
        if direction == Direction::Download {
            let mut response = send(action(client, endpoint, reqwest::Method::GET, received)?)?;
            lfs::receive(repo.path(), pointer, &mut response)?;
        } else {
            let source = pointer.verified_path(repo.path())?;
            // The client limit covers the whole upload, so scale it with size;
            // downloads are limited per read instead.
            let allowance = Duration::from_secs(300 + object.size / (32 * 1024));
            send(
                action(client, endpoint, reqwest::Method::PUT, received)?
                    .timeout(allowance)
                    .body(File::open(source)?)
                    .header("Content-Length", object.size),
            )?;
            if let Some(endpoint) = object.actions.get("verify") {
                send(
                    action(client, endpoint, reqwest::Method::POST, received)?
                        .json(&json!({"oid":object.oid,"size":object.size})),
                )?;
            }
        }
    } else if direction == Direction::Download {
        return Err(failure("Attachment service omitted the download action"));
    }
    Ok(())
}

const SERVICES: [(git2::transport::Service, &str, &str); 4] = [
    (
        git2::transport::Service::UploadPackLs,
        "info/refs?service=git-upload-pack",
        "application/x-git-upload-pack-advertisement",
    ),
    (
        git2::transport::Service::UploadPack,
        "git-upload-pack",
        "application/x-git-upload-pack-result",
    ),
    (
        git2::transport::Service::ReceivePackLs,
        "info/refs?service=git-receive-pack",
        "application/x-git-receive-pack-advertisement",
    ),
    (
        git2::transport::Service::ReceivePack,
        "git-receive-pack",
        "application/x-git-receive-pack-result",
    ),
];

/// Smart HTTP for libgit2 on the attachment client. Credentials follow
/// libgit2's flow: anonymous first, the stored token once after a 401, then
/// that token for the rest of the operation.
pub(super) struct GitSubtransport {
    operation: Option<Arc<super::smart::Operation>>,
    connection: Mutex<Option<Arc<Connection>>>,
}

struct Connection {
    operation: Arc<super::smart::Operation>,
    client: Client,
    /// Set once the token has been sent; a later 401 is final.
    supplied: Mutex<bool>,
}

fn lock_failed<T>(_: T) -> git2::Error {
    git2::Error::from_str("Git transport lock failed")
}

impl GitSubtransport {
    pub(super) const fn new(operation: Option<Arc<super::smart::Operation>>) -> Self {
        Self {
            operation,
            connection: Mutex::new(None),
        }
    }

    fn connection(&self, url: &str) -> std::result::Result<Arc<Connection>, git2::Error> {
        let operation = super::smart::require(self.operation.as_ref(), url)?;
        let mut slot = self.connection.lock().map_err(lock_failed)?;
        let connection = if let Some(existing) = slot.as_ref() {
            Arc::clone(existing)
        } else {
            let created = Arc::new(Connection {
                operation,
                client: client(None).map_err(|_| {
                    git2::Error::from_str("Unable to initialize secure Git transport")
                })?,
                supplied: Mutex::new(false),
            });
            *slot = Some(Arc::clone(&created));
            created
        };
        drop(slot);
        Ok(connection)
    }
}

impl git2::transport::SmartSubtransport for GitSubtransport {
    fn action(
        &self,
        url: &str,
        action: git2::transport::Service,
    ) -> std::result::Result<Box<dyn git2::transport::SmartSubtransportStream>, git2::Error> {
        let connection = self.connection(url)?;
        let (_, path, response_type) = SERVICES
            .into_iter()
            .find(|(service, ..)| *service == action)
            .ok_or_else(|| git2::Error::from_str("Unsupported Git service"))?;
        let endpoint = url::Url::parse(&format!("{}/{path}", url.trim_end_matches('/')))
            .map_err(|_| git2::Error::from_str("Invalid Git service URL"))?;
        let mut stream = GitStream {
            connection,
            endpoint,
            response_type,
            request: None,
            response: None,
        };
        if matches!(
            action,
            git2::transport::Service::UploadPackLs | git2::transport::Service::ReceivePackLs
        ) {
            // Request the advertisement here, so authentication and host
            // failures keep their libgit2 error codes.
            stream.response = Some(stream.connection.send(
                &stream.endpoint,
                response_type,
                None,
            )?);
        } else {
            stream.request = Some(Spool::default());
        }
        Ok(Box::new(stream))
    }

    fn close(&self) -> std::result::Result<(), git2::Error> {
        Ok(())
    }
}

impl Connection {
    fn send(
        &self,
        endpoint: &url::Url,
        response_type: &str,
        mut body: Option<&mut Spool>,
    ) -> std::result::Result<Response, git2::Error> {
        let auth = &self.operation.auth;
        loop {
            let supplied = *self.supplied.lock().map_err(lock_failed)?;
            let mut request = match body.as_deref_mut() {
                Some(spool) => {
                    self.client
                        .post(endpoint.clone())
                        .header("Content-Type", response_type.replace("-result", "-request"))
                        .body(spool.body().map_err(|_| {
                            git2::Error::from_str("Git request could not be buffered")
                        })?)
                }
                None => self.client.get(endpoint.clone()),
            }
            .header("User-Agent", "git/2.0 (bluink)")
            .header("Accept", response_type);
            if supplied {
                request = request.basic_auth(&auth.account, auth.token.as_deref());
            }
            let response = request
                .send()
                .map_err(|_| git2::Error::from_str("Git destination connection failed"))?;
            match response.status().as_u16() {
                401 if supplied || auth.token.is_none() => return Err(super::smart::auth_error()),
                401 => *self.supplied.lock().map_err(lock_failed)? = true,
                200 => {
                    // libgit2 requires the exact smart media type; anything
                    // else is a dumb or non-Git server.
                    if response
                        .headers()
                        .get("Content-Type")
                        .is_none_or(|value| value.as_bytes() != response_type.as_bytes())
                    {
                        return Err(git2::Error::from_str(
                            "Git destination does not offer smart HTTP",
                        ));
                    }
                    return Ok(response);
                }
                status => {
                    return Err(git2::Error::from_str(&format!(
                        "Git destination returned HTTP {status}"
                    )));
                }
            }
        }
    }
}

/// Advertisements are requested by `action`; RPC requests are buffered until
/// libgit2 first reads the response.
struct GitStream {
    connection: Arc<Connection>,
    endpoint: url::Url,
    response_type: &'static str,
    request: Option<Spool>,
    response: Option<Response>,
}

impl GitStream {
    fn check_cancelled(&self) -> std::io::Result<()> {
        self.connection
            .operation
            .auth
            .check_cancelled()
            .map_err(|_| std::io::Error::other("Synchronization cancelled"))
    }
}

impl Read for GitStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.check_cancelled()?;
        if self.response.is_none() {
            let response = self
                .connection
                .send(&self.endpoint, self.response_type, self.request.as_mut())
                .map_err(|error| std::io::Error::other(error.message().to_owned()))?;
            self.request = None;
            self.response = Some(response);
        }
        self.response
            .as_mut()
            .map_or(Ok(0), |response| response.read(buffer))
    }
}

impl Write for GitStream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.check_cancelled()?;
        self.request
            .as_mut()
            .ok_or_else(|| std::io::Error::other("Git request was already sent"))?
            .write_all(bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A request body that can be replayed after a 401. Large pushes spill to a
/// private temporary file instead of memory.
#[derive(Default)]
struct Spool {
    memory: Vec<u8>,
    file: Option<File>,
    length: u64,
}

const SPOOL_MEMORY: usize = 8 * 1024 * 1024;

impl Spool {
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        if self.file.is_none() && self.memory.len() + bytes.len() > SPOOL_MEMORY {
            let mut file = tempfile::tempfile()?;
            file.write_all(&std::mem::take(&mut self.memory))?;
            self.file = Some(file);
        }
        match &mut self.file {
            Some(file) => file.write_all(bytes)?,
            None => self.memory.extend_from_slice(bytes),
        }
        self.length += bytes.len() as u64;
        Ok(())
    }

    fn body(&mut self) -> std::io::Result<reqwest::blocking::Body> {
        match &mut self.file {
            Some(file) => {
                file.flush()?;
                let mut copy = file.try_clone()?;
                copy.rewind()?;
                Ok(reqwest::blocking::Body::sized(copy, self.length))
            }
            None => Ok(reqwest::blocking::Body::from(self.memory.clone())),
        }
    }
}
