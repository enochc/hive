//! Optional connection security, all applied once per connection during the
//! handshake. Nothing here runs per message.
//!
//! - `tls` feature, `[TLS]` table: TLS 1.2/1.3 via rustls on the ring
//!   backend (no CMake / C++ toolchain needed for cross builds). With
//!   `require_client_cert = true` the server also verifies the client's
//!   certificate (mutual TLS).
//! - `auth` feature, `auth_secret` / `auth_secret_env`: mutual
//!   challenge-response with HMAC-SHA256 over fresh nonces from both sides.
//!   Proves both ends hold the secret; on its own it does not encrypt
//!   anything, so pair it with TLS on untrusted networks.
//!
//! Config that asks for a feature Hive wasn't built with is an error, never
//! a silent fall back to plaintext.

use std::io;
#[allow(unused_imports)]
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

pub(crate) type BoxRead = Box<dyn AsyncRead + Send + Unpin>;
pub(crate) type BoxWrite = Box<dyn AsyncWrite + Send + Unpin>;

#[derive(Default)]
pub(crate) struct Security {
    #[cfg(feature = "tls")]
    tls: Option<tls::TlsSettings>,
    #[cfg(feature = "auth")]
    auth: Option<auth::Secret>,
}

impl Security {
    /// Read `[TLS]` and `auth_secret` / `auth_secret_env` from a Hive config.
    /// `is_server` / `is_client` say which roles this hive plays, so missing
    /// pieces (a server with no key, a client with no CA) fail here rather
    /// than on the first connection.
    pub(crate) fn from_config(config: &toml::Value, is_server: bool, is_client: bool) -> Result<Security, String> {
        #[allow(unused_mut)]
        let mut sec = Security::default();

        if let Some(t) = config.get("TLS") {
            #[cfg(feature = "tls")]
            {
                sec.tls = Some(tls::TlsSettings::from_toml(t, is_server, is_client)?);
            }
            #[cfg(not(feature = "tls"))]
            {
                let _ = (t, is_server, is_client);
                return Err("config has a [TLS] table but Hive was built without the `tls` feature".into());
            }
        }

        if let Some(secret) = secret_from_config(config)? {
            #[cfg(feature = "auth")]
            {
                sec.auth = Some(auth::Secret::new(&secret));
            }
            #[cfg(not(feature = "auth"))]
            {
                let _ = secret;
                return Err("config sets auth_secret but Hive was built without the `auth` feature".into());
            }
        }
        Ok(sec)
    }

    pub(crate) fn auth_enabled(&self) -> bool {
        #[cfg(feature = "auth")]
        {
            self.auth.is_some()
        }
        #[cfg(not(feature = "auth"))]
        {
            false
        }
    }

    /// Wrap a freshly connected/accepted socket in TLS when configured,
    /// otherwise just split it (lock-free halves for plain TCP).
    pub(crate) async fn wrap(
        &self,
        stream: TcpStream,
        accepting: bool,
        dial_addr: &str,
    ) -> io::Result<(BoxRead, BoxWrite)> {
        #[cfg(feature = "tls")]
        if let Some(t) = &self.tls {
            return t.wrap(stream, accepting, dial_addr).await;
        }
        let _ = (accepting, dial_addr);
        let (r, w) = stream.into_split();
        Ok((Box::new(r), Box::new(w)))
    }

    /// Accepting side of the shared-secret exchange. No-op without a secret.
    pub(crate) async fn auth_accept<R, W>(&self, r: &mut R, w: &mut W) -> io::Result<()>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        #[cfg(feature = "auth")]
        if let Some(s) = &self.auth {
            return auth::accept(s, r, w).await;
        }
        let _ = (r, w);
        Ok(())
    }

    /// Dialing side of the shared-secret exchange. No-op without a secret.
    pub(crate) async fn auth_dial<R, W>(&self, r: &mut R, w: &mut W) -> io::Result<()>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        #[cfg(feature = "auth")]
        if let Some(s) = &self.auth {
            return auth::dial(s, r, w).await;
        }
        let _ = (r, w);
        Ok(())
    }
}

/// The keys that turn shared-secret auth on.
const SECRET_KEYS: [&str; 2] = ["auth_secret", "auth_secret_env"];

fn secret_from_config(config: &toml::Value) -> Result<Option<Vec<u8>>, String> {
    // In TOML a key written after a `[table]` header belongs to that table.
    // An `auth_secret` that ended up in one would be ignored (or, under
    // [Properties], synced to every peer) and the hive would run without
    // authentication, so that is an error rather than a silent no-op.
    if let Some(top) = config.as_table() {
        for (name, value) in top {
            if let Some(key) = SECRET_KEYS.iter().find(|k| value.get(**k).is_some()) {
                return Err(format!(
                    "`{}` is inside [{}]; it must be at the top level, above every [table] header",
                    key, name
                ));
            }
        }
    }
    for key in SECRET_KEYS {
        if config.get(key).is_some_and(|v| !v.is_str()) {
            return Err(format!("`{}` must be a string", key));
        }
    }
    let secret = if let Some(s) = config.get("auth_secret").and_then(|v| v.as_str()) {
        s.to_string()
    } else if let Some(var) = config.get("auth_secret_env").and_then(|v| v.as_str()) {
        std::env::var(var).map_err(|_| format!("auth_secret_env: environment variable {} is not set", var))?
    } else {
        return Ok(None);
    };
    if secret.is_empty() {
        return Err("auth secret is empty".into());
    }
    Ok(Some(secret.into_bytes()))
}

#[allow(dead_code)]
pub(crate) fn denied(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::PermissionDenied, msg.to_string())
}

#[cfg(feature = "auth")]
mod auth {
    use super::*;
    use ring::hmac;
    use ring::rand::{SecureRandom, SystemRandom};

    const NONCE_LEN: usize = 32;
    const TAG_LEN: usize = 32;

    pub(crate) struct Secret(hmac::Key);

    impl Secret {
        pub(crate) fn new(secret: &[u8]) -> Self {
            Secret(hmac::Key::new(hmac::HMAC_SHA256, secret))
        }

        /// HMAC over a fixed label, the role, then both nonces. The role
        /// keeps a client's tag from being reflected back as a server's.
        fn data(role: &[u8], a: &[u8], b: &[u8]) -> Vec<u8> {
            let mut d = Vec::with_capacity(13 + role.len() + a.len() + b.len());
            d.extend_from_slice(b"hive-auth-v1 ");
            d.extend_from_slice(role);
            d.extend_from_slice(a);
            d.extend_from_slice(b);
            d
        }

        fn tag(&self, role: &[u8], a: &[u8], b: &[u8]) -> hmac::Tag {
            hmac::sign(&self.0, &Self::data(role, a, b))
        }

        /// Constant-time comparison (ring::hmac::verify).
        fn verify(&self, role: &[u8], a: &[u8], b: &[u8], tag: &[u8]) -> bool {
            hmac::verify(&self.0, &Self::data(role, a, b), tag).is_ok()
        }
    }

    fn nonce() -> io::Result<[u8; NONCE_LEN]> {
        let mut n = [0u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut n)
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "no system randomness"))?;
        Ok(n)
    }

    /// Server: send nonce_s, check the client's tag over (nonce_s, nonce_c),
    /// then prove itself with a tag over (nonce_c, nonce_s).
    pub(crate) async fn accept<R, W>(s: &Secret, r: &mut R, w: &mut W) -> io::Result<()>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let ns = nonce()?;
        w.write_all(&ns).await?;
        w.flush().await?;
        let mut buf = [0u8; NONCE_LEN + TAG_LEN];
        r.read_exact(&mut buf).await?;
        let (nc, client_tag) = buf.split_at(NONCE_LEN);
        if !s.verify(b"client", &ns, nc, client_tag) {
            return Err(denied("peer failed shared-secret authentication"));
        }
        w.write_all(s.tag(b"server", nc, &ns).as_ref()).await?;
        w.flush().await?;
        Ok(())
    }

    /// Client: read nonce_s, answer with nonce_c and its tag, then check
    /// the server's tag so a server without the secret is rejected too.
    pub(crate) async fn dial<R, W>(s: &Secret, r: &mut R, w: &mut W) -> io::Result<()>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut ns = [0u8; NONCE_LEN];
        r.read_exact(&mut ns).await?;
        let nc = nonce()?;
        let mut out = [0u8; NONCE_LEN + TAG_LEN];
        out[..NONCE_LEN].copy_from_slice(&nc);
        out[NONCE_LEN..].copy_from_slice(s.tag(b"client", &ns, &nc).as_ref());
        w.write_all(&out).await?;
        w.flush().await?;
        let mut server_tag = [0u8; TAG_LEN];
        r.read_exact(&mut server_tag).await?;
        if !s.verify(b"server", &nc, &ns, &server_tag) {
            return Err(denied("server failed shared-secret authentication"));
        }
        Ok(())
    }
}

#[cfg(feature = "tls")]
pub(crate) mod tls {
    use super::*;
    use std::sync::Arc;
    use tokio_rustls::rustls::pki_types::pem::PemObject;
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName};
    use tokio_rustls::rustls::server::WebPkiClientVerifier;
    use tokio_rustls::rustls::{self, CipherSuite, ClientConfig, RootCertStore, ServerConfig};
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    pub(crate) struct TlsSettings {
        acceptor: Option<TlsAcceptor>,
        connector: Option<TlsConnector>,
        server_name: Option<String>,
    }

    /// Everything `[TLS]` understands. Anything else is refused: a typo
    /// such as `require_client_certs` would otherwise be ignored and leave
    /// the option it was meant to set switched off.
    const KEYS: [&str; 6] = ["cert", "key", "ca", "server_name", "prefer_chacha20", "require_client_cert"];

    fn check_keys(t: &toml::Value) -> Result<(), String> {
        crate::config::check_keys(t, "[TLS]", &KEYS)
    }

    /// A string option; present with any other type is an error, not "unset".
    fn get_str<'a>(t: &'a toml::Value, k: &str) -> Result<Option<&'a str>, String> {
        crate::config::get_str(t, "[TLS]", k)
    }

    /// A true/false option, off when absent.
    fn get_bool(t: &toml::Value, k: &str) -> Result<bool, String> {
        Ok(crate::config::get_bool(t, "[TLS]", k)?.unwrap_or(false))
    }

    impl TlsSettings {
        pub(crate) fn from_toml(t: &toml::Value, is_server: bool, is_client: bool) -> Result<Self, String> {
            check_keys(t)?;
            let cert = get_str(t, "cert")?;
            let key = get_str(t, "key")?;
            let ca = get_str(t, "ca")?;
            let server_name = get_str(t, "server_name")?;
            let prefer_chacha = get_bool(t, "prefer_chacha20")?;
            let require_client_cert = get_bool(t, "require_client_cert")?;

            let acceptor = if is_server {
                let (cert, key) = cert
                    .zip(key)
                    .ok_or("[TLS] on a listening hive needs `cert` and `key`")?;
                let client_ca = if require_client_cert {
                    Some(ca.ok_or("[TLS] require_client_cert needs `ca`")?)
                } else {
                    None
                };
                Some(TlsAcceptor::from(Arc::new(server_config(cert, key, client_ca, prefer_chacha)?)))
            } else {
                None
            };
            let connector = if is_client {
                let ca = ca.ok_or("[TLS] on a connecting hive needs `ca` to verify the server")?;
                let client_cfg = client_config(ca, cert, key, prefer_chacha)?;
                Some(TlsConnector::from(Arc::new(client_cfg)))
            } else {
                None
            };
            Ok(TlsSettings {
                acceptor,
                connector,
                server_name: server_name.map(String::from),
            })
        }

        pub(crate) async fn wrap(
            &self,
            stream: TcpStream,
            accepting: bool,
            dial_addr: &str,
        ) -> io::Result<(BoxRead, BoxWrite)> {
            if accepting {
                let acceptor = self
                    .acceptor
                    .as_ref()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "TLS server not configured"))?;
                let s = acceptor.accept(stream).await?;
                let (r, w) = tokio::io::split(s);
                Ok((Box::new(r), Box::new(w)))
            } else {
                let connector = self
                    .connector
                    .as_ref()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "TLS client not configured"))?;
                let name = self.server_name.clone().unwrap_or_else(|| host_of(dial_addr).to_string());
                let name = ServerName::try_from(name)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, format!("bad TLS server name: {}", e)))?;
                let s = connector.connect(name, stream).await?;
                let (r, w) = tokio::io::split(s);
                Ok((Box::new(r), Box::new(w)))
            }
        }
    }

    /// "host:port" / "[v6]:port" -> host. Used as the name to verify when
    /// `server_name` isn't set; IP addresses verify against IP SANs.
    fn host_of(addr: &str) -> &str {
        let host = addr.rsplit_once(':').map(|(h, _)| h).unwrap_or(addr);
        host.trim_start_matches('[').trim_end_matches(']')
    }

    /// ring's default provider. Its default order puts AES-GCM first,
    /// which is right on CPUs with AES instructions; `prefer_chacha20`
    /// moves ChaCha20-Poly1305 to the front for boards without them
    /// (Raspberry Pi and other ARMv7/ARMv8 parts lacking the crypto
    /// extensions), where it is several times faster.
    pub(crate) fn provider(prefer_chacha: bool) -> Arc<rustls::crypto::CryptoProvider> {
        let mut p = rustls::crypto::ring::default_provider();
        if prefer_chacha {
            p.cipher_suites.sort_by_key(|s| {
                !matches!(
                    s.suite(),
                    CipherSuite::TLS13_CHACHA20_POLY1305_SHA256
                        | CipherSuite::TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256
                        | CipherSuite::TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256
                )
            });
        }
        Arc::new(p)
    }

    fn load_certs(path: &str) -> Result<Vec<CertificateDer<'static>>, String> {
        let certs = CertificateDer::pem_file_iter(path)
            .map_err(|e| format!("reading certificates from {}: {:?}", path, e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("parsing certificates in {}: {:?}", path, e))?;
        if certs.is_empty() {
            return Err(format!("no certificates found in {}", path));
        }
        Ok(certs)
    }

    fn load_key(path: &str) -> Result<PrivateKeyDer<'static>, String> {
        PrivateKeyDer::from_pem_file(path).map_err(|e| format!("reading private key from {}: {:?}", path, e))
    }

    fn load_roots(path: &str) -> Result<RootCertStore, String> {
        let mut roots = RootCertStore::empty();
        for c in load_certs(path)? {
            roots.add(c).map_err(|e| format!("bad CA certificate in {}: {}", path, e))?;
        }
        Ok(roots)
    }

    fn server_config(cert: &str, key: &str, client_ca: Option<&str>, prefer_chacha: bool) -> Result<ServerConfig, String> {
        let provider = provider(prefer_chacha);
        let builder = ServerConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?;
        let builder = match client_ca {
            Some(ca) => {
                let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(load_roots(ca)?), provider)
                    .build()
                    .map_err(|e| e.to_string())?;
                builder.with_client_cert_verifier(verifier)
            }
            None => builder.with_no_client_auth(),
        };
        let mut cfg = builder
            .with_single_cert(load_certs(cert)?, load_key(key)?)
            .map_err(|e| format!("TLS cert/key: {}", e))?;
        // Honour our own suite order rather than the client's.
        cfg.ignore_client_order = prefer_chacha;
        Ok(cfg)
    }

    /// Client config on the ring provider, trusting only `ca`. Presents
    /// `cert`/`key` when both are given (for mutual TLS). Shared with the
    /// MQTT peer.
    pub(crate) fn client_config(
        ca: &str,
        cert: Option<&str>,
        key: Option<&str>,
        prefer_chacha: bool,
    ) -> Result<ClientConfig, String> {
        let builder = ClientConfig::builder_with_provider(provider(prefer_chacha))
            .with_safe_default_protocol_versions()
            .map_err(|e| e.to_string())?
            .with_root_certificates(load_roots(ca)?);
        match (cert, key) {
            (Some(c), Some(k)) => builder
                .with_client_auth_cert(load_certs(c)?, load_key(k)?)
                .map_err(|e| format!("TLS client cert/key: {}", e)),
            (None, None) => Ok(builder.with_no_client_auth()),
            _ => Err("[TLS] needs both `cert` and `key`, or neither".into()),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::host_of;

        #[test]
        fn host_of_strips_port_and_brackets() {
            assert_eq!(host_of("127.0.0.1:3000"), "127.0.0.1");
            assert_eq!(host_of("hive.local:3000"), "hive.local");
            assert_eq!(host_of("[::1]:3000"), "::1");
            assert_eq!(host_of("hive.local"), "hive.local");
        }
    }
}
