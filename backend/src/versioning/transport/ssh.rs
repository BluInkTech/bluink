// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2023–2026 BluInk

//! Embedded SSH on russh for host inspection, Git and LFS. russh is
//! asynchronous while libgit2 and the LFS protocol code are blocking, so every
//! SSH task runs on a private runtime and the calling thread waits on a
//! channel: no application runtime worker ever blocks on SSH I/O, and a
//! caller inside any runtime context can wait without nesting `block_on`.
use super::super::{
    Result, failure,
    lfs::{self, Pointer},
};
use super::{Auth, Direction, Remote, model::HostIdentity, smart, wire};
use russh::{
    ChannelMsg,
    client::{self, Handle},
    keys::{HashAlg, PrivateKeyWithHashAlg, PublicKeyOrCertificate, agent::AgentIdentity},
};
use std::{
    collections::BTreeMap,
    future::Future,
    io::{Read, Write},
    net::ToSocketAddrs,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// The bound libssh2 applied to each blocking attachment operation.
const LFS_TIMEOUT: Duration = Duration::from_secs(30);
/// Writes are batched into channel data packets of at most this size.
const WRITE_BUFFER: usize = 32 * 1024;

pub(super) fn fingerprint(bytes: &[u8]) -> String {
    use base64::Engine as _;
    format!(
        "SHA256:{}",
        base64::engine::general_purpose::STANDARD_NO_PAD.encode(bytes)
    )
}

fn runtime() -> Result<&'static tokio::runtime::Runtime> {
    static RUNTIME: OnceLock<Option<tokio::runtime::Runtime>> = OnceLock::new();
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("bluink-ssh")
                .enable_io()
                .enable_time()
                .build()
                .ok()
        })
        .as_ref()
        .ok_or_else(|| failure("Unable to initialize embedded SSH"))
}

/// Runs `future` on the SSH runtime and waits for it on this thread. A set
/// cancellation flag abandons the wait and the task within a fraction of a
/// second, even while the server is silent.
fn wait<T: Send + 'static>(
    cancel: Option<&Arc<AtomicBool>>,
    future: impl Future<Output = T> + Send + 'static,
) -> Result<T> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let task = runtime()?.spawn(async move {
        let _ = sender.send(future.await);
    });
    loop {
        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(value) => return Ok(value),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if cancel.is_some_and(|cancel| cancel.load(Ordering::Relaxed)) {
                    task.abort();
                    return Err(super::cancelled());
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err(failure("Embedded SSH stopped unexpectedly"));
            }
        }
    }
}

/// Records the server's SHA-256 host-key fingerprint and accepts only the
/// fingerprint the user trusted. russh verifies the key-exchange signature
/// before asking, so a recorded fingerprint belongs to the answering server.
struct Client {
    trusted: Option<String>,
    seen: Arc<OnceLock<String>>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    fn check_server_key(
        &mut self,
        server_public_key: &PublicKeyOrCertificate,
    ) -> impl Future<Output = std::result::Result<bool, Self::Error>> + Send {
        // Certificate host-key algorithms are not offered, as with libssh2.
        let trusted = if let PublicKeyOrCertificate::PublicKey { key, .. } = server_public_key {
            let seen = fingerprint(key.fingerprint(HashAlg::Sha256).as_bytes());
            let trusted = self.trusted.as_deref() == Some(seen.as_str());
            let _ = self.seen.set(seen);
            trusted
        } else {
            false
        };
        std::future::ready(Ok(trusted))
    }
}

enum Handshake {
    Unreachable,
    Failed,
}

/// Connects and exchanges keys. The handle is present only when the server's
/// fingerprint matched `trusted`.
fn handshake(
    remote: &Remote,
    trusted: Option<&str>,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<(Option<Handle<Client>>, HostIdentity)> {
    let Remote::Ssh(url) = remote else {
        return Err(failure("Host verification is only available for SSH"));
    };
    let host = url
        .host_str()
        .ok_or_else(|| failure("SSH host is missing"))?;
    let port = url.port().unwrap_or(22);
    let addresses: Vec<_> = (host, port)
        .to_socket_addrs()
        .map_err(|_| failure("SSH host could not be resolved").classified("offline"))?
        .collect();
    let seen = Arc::new(OnceLock::new());
    let handler = Client {
        trusted: trusted.map(str::to_owned),
        seen: Arc::clone(&seen),
    };
    let connected = wait(cancel, async move {
        let mut socket = None;
        for address in addresses {
            let attempt = tokio::time::timeout(
                Duration::from_secs(15),
                tokio::net::TcpStream::connect(address),
            );
            if let Ok(Ok(stream)) = attempt.await {
                socket = Some(stream);
                break;
            }
        }
        let socket = socket.ok_or(Handshake::Unreachable)?;
        let _ = socket.set_nodelay(true);
        let config = Arc::new(client::Config::default());
        tokio::time::timeout(
            Duration::from_secs(30),
            client::connect_stream(config, socket, handler),
        )
        .await
        .ok()
        .and_then(std::result::Result::ok)
        .ok_or(Handshake::Failed)
    })?;
    let identity = |fingerprint: String| HostIdentity {
        host: host.to_owned(),
        port,
        fingerprint,
    };
    match connected {
        Ok(handle) => Ok((
            Some(handle),
            identity(seen.get().cloned().unwrap_or_default()),
        )),
        Err(Handshake::Unreachable) => {
            Err(failure("SSH host could not be reached").classified("offline"))
        }
        Err(Handshake::Failed) => seen.get().map_or_else(
            || Err(failure("SSH handshake failed").classified("destination_unavailable")),
            |fingerprint| Ok((None, identity(fingerprint.clone()))),
        ),
    }
}

/// With nothing trusted, the server is always refused after its key is seen.
pub(crate) fn inspect_host(remote: &Remote) -> Result<HostIdentity> {
    Ok(handshake(remote, None, None)?.1)
}

/// An authenticated SSH connection. Dropping it disconnects politely.
pub(super) struct Session {
    handle: Option<Arc<Handle<Client>>>,
    cancel: Option<Arc<AtomicBool>>,
}

impl Session {
    fn new(handle: Option<Handle<Client>>, cancel: Option<Arc<AtomicBool>>) -> Self {
        Self {
            handle: handle.map(Arc::new),
            cancel,
        }
    }

    fn channel(&self, timeout: Option<Duration>) -> Result<Channel> {
        let handle = Arc::clone(
            self.handle
                .as_ref()
                .ok_or_else(|| failure("SSH session is closed"))?,
        );
        let opened = wait(self.cancel.as_ref(), async move {
            handle.channel_open_session().await
        })?
        .map_err(|_| failure("SSH channel failed"))?;
        Ok(Channel {
            inner: Some(opened),
            incoming: std::io::Cursor::new(Vec::new()),
            outgoing: Vec::new(),
            exit_status: None,
            ended: false,
            closed: false,
            timeout,
            cancel: self.cancel.clone(),
        })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let (Some(handle), Ok(runtime)) = (self.handle.take(), runtime()) {
            runtime.spawn(async move {
                let _ = handle
                    .disconnect(russh::Disconnect::ByApplication, "", "en")
                    .await;
            });
        }
    }
}

fn connect(remote: &Remote, auth: &Auth) -> Result<Session> {
    auth.check_cancelled()?;
    let (handle, identity) = handshake(
        remote,
        auth.host_fingerprint.as_deref(),
        auth.cancel.as_ref(),
    )?;
    let Some(handle) = handle else {
        if auth.host_fingerprint.as_deref() == Some(identity.fingerprint.as_str()) {
            return Err(failure("SSH handshake failed").classified("destination_unavailable"));
        }
        return Err(failure(format!(
            "Host verification required: {}",
            identity.fingerprint
        ))
        .classified("verify_server"));
    };
    let Remote::Ssh(url) = remote else {
        return Err(failure("Invalid SSH remote"));
    };
    let user = url.username().to_owned();
    let cancel = auth.cancel.as_ref();
    if auth.use_agent {
        let (handle, accepted) = wait(cancel, async move {
            let mut handle = handle;
            let accepted = agent_sign_in(&mut handle, &user).await;
            (handle, accepted)
        })?;
        if !accepted {
            return Err(
                failure("Needs sign-in: no SSH agent key was accepted").classified("needs_sign_in")
            );
        }
        return Ok(Session::new(Some(handle), auth.cancel.clone()));
    }
    let rejected =
        || failure("Needs sign-in: SSH key or passphrase was rejected").classified("needs_sign_in");
    let path = auth.private_key.as_deref().ok_or_else(|| {
        failure("Needs sign-in: choose an SSH private key").classified("needs_sign_in")
    })?;
    let key = Arc::new(
        russh::keys::load_secret_key(path, auth.passphrase.as_deref()).map_err(|_| rejected())?,
    );
    let (handle, accepted) = wait(cancel, async move {
        let mut handle = handle;
        let hash = rsa_hash(&handle).await;
        let accepted = handle
            .authenticate_publickey(user, PrivateKeyWithHashAlg::new(key, hash))
            .await
            .is_ok_and(|result| result.success());
        (handle, accepted)
    })?;
    if !accepted {
        return Err(rejected());
    }
    Ok(Session::new(Some(handle), auth.cancel.clone()))
}

/// The RSA signature hash the server advertised; without the extension this
/// is `ssh-rsa`, as libssh2 chose.
async fn rsa_hash(handle: &Handle<Client>) -> Option<HashAlg> {
    handle
        .best_supported_rsa_hash()
        .await
        .ok()
        .flatten()
        .flatten()
}

type Agent = russh::keys::agent::client::AgentClient<
    Box<dyn russh::keys::agent::client::AgentStream + Send + Unpin>,
>;

#[cfg(unix)]
async fn agents() -> Vec<Agent> {
    russh::keys::agent::client::AgentClient::connect_env()
        .await
        .map(russh::keys::agent::client::AgentClient::dynamic)
        .into_iter()
        .collect()
}

/// Pageant and the Windows OpenSSH agent pipe, the two agents libssh2 knew.
#[cfg(windows)]
async fn agents() -> Vec<Agent> {
    use russh::keys::agent::client::AgentClient;
    let pipe =
        std::env::var_os("SSH_AUTH_SOCK").unwrap_or_else(|| r"\\.\pipe\openssh-ssh-agent".into());
    let mut agents = Vec::new();
    if let Ok(agent) = AgentClient::connect_pageant().await {
        agents.push(agent.dynamic());
    }
    if let Ok(agent) = AgentClient::connect_named_pipe(pipe).await {
        agents.push(agent.dynamic());
    }
    agents
}

/// One sign-in attempt, offering each agent identity in turn as libssh2 did.
async fn agent_sign_in(handle: &mut Handle<Client>, user: &str) -> bool {
    let hash = rsa_hash(handle).await;
    for mut agent in agents().await {
        let Ok(identities) = agent.request_identities().await else {
            continue;
        };
        for identity in identities {
            let AgentIdentity::PublicKey { key, .. } = identity else {
                continue;
            };
            let hash = if key.algorithm().is_rsa() { hash } else { None };
            if handle
                .authenticate_publickey_with(user, key, hash, &mut agent)
                .await
                .is_ok_and(|result| result.success())
            {
                return true;
            }
        }
    }
    false
}

pub(super) fn diagnose_git(remote: &Remote, auth: &Auth) -> Option<super::super::Error> {
    connect(remote, auth).err()
}

/// A blocking view of one SSH channel for the pkt-line and Git protocol code.
/// Writes are batched until a flush or a read.
pub(super) struct Channel {
    inner: Option<russh::Channel<client::Msg>>,
    incoming: std::io::Cursor<Vec<u8>>,
    outgoing: Vec<u8>,
    exit_status: Option<u32>,
    /// No more output will arrive.
    ended: bool,
    /// The server closed the channel.
    closed: bool,
    timeout: Option<Duration>,
    cancel: Option<Arc<AtomicBool>>,
}

impl Channel {
    /// Lends the channel to an SSH task, with this channel's time limit.
    fn run<T: Send + 'static, F>(
        &mut self,
        operation: impl FnOnce(russh::Channel<client::Msg>) -> F,
    ) -> Result<T>
    where
        F: Future<Output = (russh::Channel<client::Msg>, Option<T>)> + Send + 'static,
    {
        let channel = self
            .inner
            .take()
            .ok_or_else(|| failure("SSH channel is closed"))?;
        let future = operation(channel);
        let limit = self.timeout;
        let (channel, value) = wait(self.cancel.as_ref(), async move {
            match limit {
                Some(limit) => tokio::time::timeout(limit, future).await.ok(),
                None => Some(future.await),
            }
        })?
        .ok_or_else(|| failure("SSH connection timed out").classified("offline"))?;
        self.inner = Some(channel);
        value.ok_or_else(|| failure("SSH channel failed"))
    }

    /// Runs a remote command and reports whether the server accepted it.
    fn exec(&mut self, command: &str) -> Result<bool> {
        let command = command.to_owned();
        let (accepted, early) = self.run(|mut channel| async move {
            if channel.exec(true, command).await.is_err() {
                return (channel, Some((false, Vec::new())));
            }
            let mut early = Vec::new();
            loop {
                match channel.wait().await {
                    Some(ChannelMsg::Success) => return (channel, Some((true, early))),
                    Some(ChannelMsg::Data { data }) => early.extend_from_slice(&data),
                    Some(ChannelMsg::Failure | ChannelMsg::Eof | ChannelMsg::Close) | None => {
                        return (channel, Some((false, early)));
                    }
                    Some(_) => {}
                }
            }
        })?;
        self.incoming = std::io::Cursor::new(early);
        Ok(accepted)
    }

    fn send(&mut self) -> Result<()> {
        if self.outgoing.is_empty() {
            return Ok(());
        }
        let bytes = std::mem::take(&mut self.outgoing);
        self.run(|channel| async move {
            let sent = channel.data(bytes.as_slice()).await.is_ok();
            (channel, sent.then_some(()))
        })
    }

    /// The next channel message, recording the exit status and closure.
    fn next(&mut self) -> Result<Option<ChannelMsg>> {
        let message = self.run(|mut channel| async move {
            let message = channel.wait().await;
            (channel, Some(message))
        })?;
        match &message {
            Some(ChannelMsg::ExitStatus { exit_status }) => self.exit_status = Some(*exit_status),
            Some(ChannelMsg::Eof) => self.ended = true,
            Some(ChannelMsg::Close) | None => {
                self.ended = true;
                self.closed = true;
            }
            Some(_) => {}
        }
        Ok(message)
    }

    /// Waits for stdout data, skipping stderr. Returns false at end of output.
    fn receive(&mut self) -> Result<bool> {
        while !self.ended {
            if let Some(ChannelMsg::Data { data }) = self.next()? {
                self.incoming = std::io::Cursor::new(data.to_vec());
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Reads to the end of output, then until the server closes the channel;
    /// the exit status may follow end of output.
    fn wait_close(&mut self) -> Result<()> {
        self.send()?;
        let mut discard = [0; 8192];
        while self.read(&mut discard)? > 0 {}
        while !self.closed {
            self.next()?;
        }
        Ok(())
    }

    /// libssh2 reported 0 when the server sent no exit status.
    const fn exit_status(&self) -> u32 {
        match self.exit_status {
            Some(status) => status,
            None => 0,
        }
    }
}

impl Read for Channel {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.incoming.read(buffer)?;
            if count > 0 {
                return Ok(count);
            }
            self.send().map_err(std::io::Error::other)?;
            if !self.receive().map_err(std::io::Error::other)? {
                return Ok(0);
            }
        }
    }
}

impl Write for Channel {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.outgoing.extend_from_slice(bytes);
        if self.outgoing.len() >= WRITE_BUFFER {
            self.send().map_err(std::io::Error::other)?;
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.send().map_err(std::io::Error::other)
    }
}

fn command(remote: &Remote, executable: &str, direction: Direction) -> Result<String> {
    let Remote::Ssh(url) = remote else {
        return Err(failure("Invalid SSH remote"));
    };
    let path = percent_encoding::percent_decode_str(url.path().trim_start_matches('/'))
        .decode_utf8()
        .map_err(|_| failure("Invalid SSH repository path"))?;
    if path.is_empty() || path.starts_with('-') || path.chars().any(char::is_control) {
        return Err(failure("Invalid SSH repository path"));
    }
    // SSH sends a remote command string, not a local shell invocation. Quote the
    // one data argument so a repository name can never introduce remote commands.
    Ok(format!(
        "{executable} '{}' {}",
        path.replace('\'', "'\\''"),
        direction.name()
    ))
}

/// The command libgit2's SSH transport sent: the URL path as written, with
/// `/~` reduced to `~` and `'` and `!` escaped inside single quotes.
pub(super) fn git_command(remote: &Remote, executable: &str) -> Result<String> {
    let Remote::Ssh(url) = remote else {
        return Err(failure("Invalid SSH remote"));
    };
    let path = url.path();
    let path = if path.starts_with("/~") {
        &path[1..]
    } else {
        path
    };
    if path.is_empty() || path.chars().any(char::is_control) {
        return Err(failure("Invalid SSH repository path"));
    }
    let mut quoted = String::with_capacity(path.len());
    for character in path.chars() {
        if matches!(character, '\'' | '!') {
            quoted.push_str("'\\");
            quoted.push(character);
            quoted.push('\'');
        } else {
            quoted.push(character);
        }
    }
    Ok(format!("{executable} '{quoted}'"))
}

pub(super) fn transfer(
    repo: &git2::Repository,
    remote: &Remote,
    auth: &Auth,
    pointers: &[Pointer],
    direction: Direction,
) -> Result<()> {
    let session = connect(remote, auth)?;
    let mut channel = session
        .channel(Some(LFS_TIMEOUT))
        .map_err(|_| failure("SSH attachment channel failed"))?;
    let available = channel
        .exec(&command(remote, "git-lfs-transfer", direction)?)
        .unwrap_or(false)
        && wire::section(&mut channel)
            .is_ok_and(|(caps, delim)| !delim && caps.iter().any(|cap| cap == "version=1"));
    if available {
        wire::command(&mut channel, "version 1", &[], false)?;
        wire::finish(&mut channel)?;
        pure(repo, &mut channel, auth, pointers, direction)?;
        wire::command(&mut channel, "quit", &[], false)?;
        wire::finish(&mut channel)?;
        #[cfg(test)]
        super::tests::LAST_SSH_TRANSFER.set(Some("pure"));
        tracing::debug!(
            operation = "lfs.transfer",
            transport = "ssh",
            "Attachment transfer completed"
        );
        return Ok(());
    }
    drop(channel);
    let mut channel = session
        .channel(Some(LFS_TIMEOUT))
        .map_err(|_| failure("SSH LFS authentication channel failed"))?;
    if !channel
        .exec(&command(remote, "git-lfs-authenticate", direction)?)
        .unwrap_or(false)
    {
        return Err(failure("SSH server offers no supported LFS transfer"));
    }
    let mut output = Vec::new();
    std::io::Read::by_ref(&mut channel)
        .take(1024 * 1024 + 1)
        .read_to_end(&mut output)?;
    if output.len() > 1024 * 1024 {
        return Err(failure("SSH LFS authentication response is too large"));
    }
    channel
        .wait_close()
        .map_err(|_| failure("SSH LFS authentication did not finish"))?;
    if channel.exit_status() != 0 {
        return Err(failure("SSH server offers no supported LFS transfer"));
    }
    let endpoint: super::http::Endpoint = serde_json::from_slice(&output)
        .map_err(|_| failure("Invalid SSH LFS authentication response"))?;
    super::http::transfer_endpoint(repo, auth, &endpoint, pointers, direction)?;
    #[cfg(test)]
    super::tests::LAST_SSH_TRANSFER.set(Some("hybrid"));
    tracing::debug!(
        operation = "lfs.transfer",
        transport = "ssh-authenticated-https",
        "Attachment transfer completed"
    );
    Ok(())
}

fn pure(
    repo: &git2::Repository,
    channel: &mut Channel,
    auth: &Auth,
    pointers: &[Pointer],
    direction: Direction,
) -> Result<()> {
    for chunk in pointers.chunks(100) {
        auth.check_cancelled()?;
        wire::command(
            channel,
            "batch",
            &[
                "transfer=ssh".into(),
                "hash-algo=sha256".into(),
                "refname=refs/heads/main".into(),
            ],
            true,
        )?;
        for pointer in chunk {
            let (oid, size) = pointer.fingerprint();
            wire::write(channel, format!("{oid} {size}\n").as_bytes())?;
        }
        channel.write_all(b"0000")?;
        channel.flush()?;
        let (_, delimiter) = wire::status(channel)?;
        if !delimiter {
            return Err(failure("SSH attachment batch has no objects"));
        }
        let (objects, delimiter) = wire::section(channel)?;
        if delimiter {
            return Err(failure("Invalid SSH attachment batch delimiter"));
        }
        let requested: BTreeMap<_, _> = chunk.iter().map(|p| (p.fingerprint().0, p)).collect();
        let mut seen = BTreeMap::new();
        for line in objects {
            let words: Vec<_> = line.split(' ').collect();
            let [oid, size, action, args @ ..] = words.as_slice() else {
                return Err(failure("Invalid SSH attachment batch entry"));
            };
            let pointer = requested
                .get(*oid)
                .ok_or_else(|| failure("Unexpected SSH attachment object"))?;
            if size.parse::<u64>().ok() != Some(pointer.fingerprint().1) {
                return Err(failure("SSH attachment size mismatch"));
            }
            let actions = seen
                .entry((*oid).to_owned())
                .or_insert_with(std::collections::BTreeSet::new);
            if !actions.insert((*action).to_owned()) {
                return Err(failure("Duplicate SSH attachment action"));
            }
            let arguments = args
                .iter()
                .filter(|v| v.starts_with("id=") || v.starts_with("token="))
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            pure_object(repo, channel, pointer, action, &arguments, direction)?;
        }
        if seen.len() != requested.len() {
            return Err(failure("SSH attachment service omitted requested objects"));
        }
    }
    Ok(())
}

fn pure_object(
    repo: &git2::Repository,
    channel: &mut Channel,
    pointer: &Pointer,
    action: &str,
    args: &[String],
    direction: Direction,
) -> Result<()> {
    let (oid, size) = pointer.fingerprint();
    match (direction, action) {
        (Direction::Download, "download") => {
            wire::command(channel, &format!("get-object {oid}"), args, false)?;
            let (headers, delimiter) = wire::status(channel)?;
            if !delimiter || !headers.iter().any(|line| line == &format!("size={size}")) {
                return Err(failure("SSH attachment response size mismatch"));
            }
            lfs::receive(repo.path(), pointer, &mut wire::Body::new(channel))?;
        }
        (Direction::Upload, "upload") => {
            let mut args = args.to_vec();
            args.push(format!("size={size}"));
            wire::command(channel, &format!("put-object {oid}"), &args, true)?;
            let mut source = std::fs::File::open(pointer.verified_path(repo.path())?)?;
            let mut bytes = vec![0; 65_516];
            loop {
                let count = source.read(&mut bytes)?;
                if count == 0 {
                    break;
                }
                wire::write(channel, &bytes[..count])?;
            }
            channel.write_all(b"0000")?;
            channel.flush()?;
            wire::finish(channel)?;
        }
        (Direction::Upload, "verify") => {
            let mut args = args.to_vec();
            args.push(format!("size={size}"));
            wire::command(channel, &format!("verify-object {oid}"), &args, false)?;
            wire::finish(channel)?;
        }
        (Direction::Upload, "noop") => {}
        _ => {
            return Err(failure(
                "SSH attachment service returned an unsupported action",
            ));
        }
    }
    Ok(())
}

/// Git's `upload-pack`/`receive-pack` exec channel for libgit2. The smart
/// protocol is stateful over SSH, so libgit2 reuses the advertisement's
/// stream for the transfer that follows it.
pub(super) struct GitSubtransport {
    operation: Option<Arc<smart::Operation>>,
}

impl GitSubtransport {
    pub(super) const fn new(operation: Option<Arc<smart::Operation>>) -> Self {
        Self { operation }
    }
}

impl git2::transport::SmartSubtransport for GitSubtransport {
    fn action(
        &self,
        url: &str,
        action: git2::transport::Service,
    ) -> std::result::Result<Box<dyn git2::transport::SmartSubtransportStream>, git2::Error> {
        let operation = smart::require(self.operation.as_ref(), url)?;
        let executable = match action {
            git2::transport::Service::UploadPackLs | git2::transport::Service::UploadPack => {
                "git-upload-pack"
            }
            git2::transport::Service::ReceivePackLs | git2::transport::Service::ReceivePack => {
                "git-receive-pack"
            }
        };
        let command = git_command(&operation.remote, executable)
            .map_err(|_| git2::Error::from_str("Invalid SSH repository path"))?;
        let session =
            connect(&operation.remote, &operation.auth).map_err(|error| match error.state {
                "verify_server" => smart::host_verification(&error.to_string()),
                "needs_sign_in" => smart::auth_error(),
                _ => git2::Error::from_str("SSH connection failed"),
            })?;
        let mut channel = session
            .channel(None)
            .map_err(|_| git2::Error::from_str("SSH channel failed"))?;
        if !channel
            .exec(&command)
            .map_err(|_| git2::Error::from_str("SSH channel failed"))?
        {
            return Err(git2::Error::from_str("SSH server refused the Git command"));
        }
        Ok(Box::new(GitStream {
            channel,
            _session: session,
        }))
    }

    fn close(&self) -> std::result::Result<(), git2::Error> {
        Ok(())
    }
}

/// Field order drops the channel before its session disconnects.
struct GitStream {
    channel: Channel,
    _session: Session,
}

impl Read for GitStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.channel.read(buffer)
    }
}

impl Write for GitStream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.channel.write(bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.channel.flush()
    }
}
