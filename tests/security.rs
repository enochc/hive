//! TLS, mutual TLS and shared-secret auth (todo.md phase 4). Run with
//!   cargo test --features tls,auth --test security
//! Without those features only the fail-closed tests run.

use hive::handler::Handler;
use hive::hive::Hive;
use hive::CancellationToken;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

async fn wait_for<F: Fn() -> bool>(f: F, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    f()
}

/// Long enough for a rejected handshake to have been rejected.
const REJECT_WAIT: Duration = Duration::from_millis(800);

async fn server(port: u16, extra: &str) -> (Handler, Arc<AtomicI64>) {
    let mut h = Hive::new_from_str_unknown(&format!(
        "listen = \"127.0.0.1:{port}\"\nname = \"Serv\"\n{extra}\n[Properties]\nm = 0"
    ));
    let last = Arc::new(AtomicI64::new(-1));
    let l = last.clone();
    h.get_mut_property_by_name("m").unwrap().on_next(move |v| {
        l.store(v.val.as_integer().unwrap(), Ordering::SeqCst);
    });
    (h.go(true, CancellationToken::new()).await, last)
}

async fn client(port: u16, name: &str, extra: &str) -> (Handler, Arc<AtomicI64>, CancellationToken) {
    let mut c = Hive::new_from_str_unknown(&format!(
        "connect = \"127.0.0.1:{port}\"\nname = \"{name}\"\n{extra}"
    ));
    let last = Arc::new(AtomicI64::new(-1));
    let l = last.clone();
    c.get_mut_property_by_name("m").unwrap().on_next(move |v| {
        l.store(v.val.as_integer().unwrap(), Ordering::SeqCst);
    });
    let token = CancellationToken::new();
    (c.go(false, token.clone()).await, last, token)
}

/// Properties flow both ways between `srv` and an attached client.
async fn assert_syncs(srv: &mut Handler, srv_last: &AtomicI64, cli: &mut Handler, cli_last: &AtomicI64) {
    cli.set_property("m", Some(&11.into())).await;
    assert!(wait_for(|| srv_last.load(Ordering::SeqCst) == 11, Duration::from_secs(3)).await, "client -> server");
    srv.set_property("m", Some(&22.into())).await;
    assert!(wait_for(|| cli_last.load(Ordering::SeqCst) == 22, Duration::from_secs(3)).await, "server -> client");
}

#[cfg(not(feature = "tls"))]
#[test]
#[should_panic(expected = "without the `tls` feature")]
fn tls_config_without_tls_feature_refuses_to_start() {
    Hive::new_from_str_unknown("listen = \"127.0.0.1:34990\"\n[TLS]\ncert = \"a\"\nkey = \"b\"");
}

#[cfg(not(feature = "auth"))]
#[test]
#[should_panic(expected = "without the `auth` feature")]
fn auth_config_without_auth_feature_refuses_to_start() {
    Hive::new_from_str_unknown("listen = \"127.0.0.1:34991\"\nauth_secret = \"s\"");
}

/// A key written below a [table] header belongs to that table. A secret
/// that landed in one must stop the hive, not leave it open (or, under
/// [Properties], turn the secret into a property synced to every peer).
#[test]
#[should_panic(expected = "must be at the top level")]
fn auth_secret_inside_a_table_refuses_to_start() {
    Hive::new_from_str_unknown("listen = \"127.0.0.1:34992\"\n[Properties]\nm = 0\nauth_secret = \"s\"");
}

#[cfg(feature = "tls")]
mod tls {
    use super::*;
    use rcgen::{BasicConstraints, Certificate, CertificateParams, IsCa, KeyPair};
    use std::path::PathBuf;

    struct Ca {
        cert: Certificate,
        key: KeyPair,
    }

    fn ca() -> Ca {
        let mut p = CertificateParams::new(Vec::<String>::new()).unwrap();
        p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let key = KeyPair::generate().unwrap();
        Ca { cert: p.self_signed(&key).unwrap(), key }
    }

    /// Writes CA/cert/key PEMs into a fresh temp dir and hands out paths.
    struct Pki {
        dir: PathBuf,
    }

    impl Pki {
        fn new(tag: &str) -> Pki {
            let dir = std::env::temp_dir().join(format!("hive-tls-{}-{}", std::process::id(), tag));
            std::fs::create_dir_all(&dir).unwrap();
            Pki { dir }
        }

        fn ca(&self, name: &str, ca: &Ca) -> String {
            let p = self.dir.join(format!("{name}.pem"));
            std::fs::write(&p, ca.cert.pem()).unwrap();
            p.display().to_string()
        }

        /// Leaf cert for 127.0.0.1/localhost signed by `ca`; returns (cert, key) paths.
        fn leaf(&self, name: &str, ca: &Ca) -> (String, String) {
            let key = KeyPair::generate().unwrap();
            let cert = CertificateParams::new(vec!["127.0.0.1".to_string(), "localhost".to_string()])
                .unwrap()
                .signed_by(&key, &ca.cert, &ca.key)
                .unwrap();
            let c = self.dir.join(format!("{name}.pem"));
            let k = self.dir.join(format!("{name}.key"));
            std::fs::write(&c, cert.pem()).unwrap();
            std::fs::write(&k, key.serialize_pem()).unwrap();
            (c.display().to_string(), k.display().to_string())
        }
    }

    impl Drop for Pki {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn server_tls(cert: &(String, String), extra: &str) -> String {
        format!("[TLS]\ncert = \"{}\"\nkey = \"{}\"\n{extra}", cert.0, cert.1)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn tls_syncs_properties() {
        let pki = Pki::new("sync");
        let ca1 = ca();
        let ca_path = pki.ca("ca", &ca1);
        let srv_cert = pki.leaf("server", &ca1);

        let (mut srv, srv_last) = server(34901, &server_tls(&srv_cert, "")).await;
        let (mut cli, cli_last, _t) = client(34901, "c", &format!("[TLS]\nca = \"{ca_path}\"")).await;
        assert!(wait_for(|| srv.peer_count() == 1, Duration::from_secs(3)).await, "TLS client never attached");
        assert_syncs(&mut srv, &srv_last, &mut cli, &cli_last).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn prefer_chacha20_still_connects() {
        let pki = Pki::new("chacha");
        let ca1 = ca();
        let ca_path = pki.ca("ca", &ca1);
        let srv_cert = pki.leaf("server", &ca1);

        let (mut srv, srv_last) = server(34902, &server_tls(&srv_cert, "prefer_chacha20 = true")).await;
        let (mut cli, cli_last, _t) =
            client(34902, "c", &format!("[TLS]\nca = \"{ca_path}\"\nprefer_chacha20 = true")).await;
        assert!(wait_for(|| srv.peer_count() == 1, Duration::from_secs(3)).await);
        assert_syncs(&mut srv, &srv_last, &mut cli, &cli_last).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn client_trusting_another_ca_is_rejected() {
        let pki = Pki::new("wrongca");
        let ca1 = ca();
        let other = pki.ca("other", &ca());
        let srv_cert = pki.leaf("server", &ca1);

        let (srv, _) = server(34903, &server_tls(&srv_cert, "")).await;
        let (cli, _, _t) = client(34903, "c", &format!("[TLS]\nca = \"{other}\"")).await;
        tokio::time::sleep(REJECT_WAIT).await;
        assert_eq!(srv.peer_count(), 0);
        assert_eq!(cli.peer_count(), 0, "client accepted a server its CA didn't sign");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plaintext_client_is_rejected_and_tls_clients_unaffected() {
        let pki = Pki::new("plain");
        let ca1 = ca();
        let ca_path = pki.ca("ca", &ca1);
        let srv_cert = pki.leaf("server", &ca1);

        let (mut srv, srv_last) = server(34904, &server_tls(&srv_cert, "")).await;
        let (mut good, good_last, _t1) = client(34904, "good", &format!("[TLS]\nca = \"{ca_path}\"")).await;
        assert!(wait_for(|| srv.peer_count() == 1, Duration::from_secs(3)).await);

        let (plain, _, _t2) = client(34904, "plain", "").await;
        tokio::time::sleep(REJECT_WAIT).await;
        assert_eq!(srv.peer_count(), 1, "plaintext client got in");
        assert_eq!(plain.peer_count(), 0);
        assert_syncs(&mut srv, &srv_last, &mut good, &good_last).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn mutual_tls_requires_a_client_cert_from_the_ca() {
        let pki = Pki::new("mtls");
        let ca1 = ca();
        let ca2 = ca();
        let ca_path = pki.ca("ca", &ca1);
        let srv_cert = pki.leaf("server", &ca1);
        let good_cert = pki.leaf("client-good", &ca1);
        let foreign_cert = pki.leaf("client-foreign", &ca2);

        let (mut srv, srv_last) = server(
            34905,
            &server_tls(&srv_cert, &format!("ca = \"{ca_path}\"\nrequire_client_cert = true")),
        )
        .await;

        let (no_cert, _, _t1) = client(34905, "nocert", &format!("[TLS]\nca = \"{ca_path}\"")).await;
        let (foreign, _, _t2) = client(
            34905,
            "foreign",
            &format!("[TLS]\nca = \"{ca_path}\"\ncert = \"{}\"\nkey = \"{}\"", foreign_cert.0, foreign_cert.1),
        )
        .await;
        tokio::time::sleep(REJECT_WAIT).await;
        assert_eq!(srv.peer_count(), 0, "a client without a valid cert got in");
        assert_eq!(no_cert.peer_count(), 0);
        assert_eq!(foreign.peer_count(), 0);

        let (mut good, good_last, _t3) = client(
            34905,
            "good",
            &format!("[TLS]\nca = \"{ca_path}\"\ncert = \"{}\"\nkey = \"{}\"", good_cert.0, good_cert.1),
        )
        .await;
        assert!(wait_for(|| srv.peer_count() == 1, Duration::from_secs(3)).await, "valid client cert rejected");
        assert_syncs(&mut srv, &srv_last, &mut good, &good_last).await;
    }

    #[test]
    #[should_panic(expected = "reading private key")]
    fn missing_key_file_refuses_to_start() {
        let pki = Pki::new("missing");
        let ca1 = ca();
        let (cert, _) = pki.leaf("server", &ca1);
        Hive::new_from_str_unknown(&format!(
            "listen = \"127.0.0.1:34906\"\n[TLS]\ncert = \"{cert}\"\nkey = \"/nonexistent/hive.key\""
        ));
    }

    /// One letter out, and without this check the server would start with
    /// client verification silently off.
    #[test]
    #[should_panic(expected = "unknown key `require_client_certs`")]
    fn misspelled_tls_option_refuses_to_start() {
        Hive::new_from_str_unknown(
            "listen = \"127.0.0.1:34909\"\n[TLS]\ncert = \"a\"\nkey = \"b\"\nca = \"c\"\nrequire_client_certs = true",
        );
    }

    #[test]
    #[should_panic(expected = "`require_client_cert` must be true or false")]
    fn quoted_tls_boolean_refuses_to_start() {
        Hive::new_from_str_unknown(
            "listen = \"127.0.0.1:34910\"\n[TLS]\ncert = \"a\"\nkey = \"b\"\nca = \"c\"\nrequire_client_cert = \"true\"",
        );
    }

    #[test]
    #[should_panic(expected = "unknown key `auth_secret`")]
    fn auth_secret_under_tls_refuses_to_start() {
        Hive::new_from_str_unknown(
            "listen = \"127.0.0.1:34911\"\n[TLS]\ncert = \"a\"\nkey = \"b\"\nauth_secret = \"s\"",
        );
    }

    #[test]
    #[should_panic(expected = "needs `ca`")]
    fn tls_client_without_ca_refuses_to_start() {
        Hive::new_from_str_unknown("connect = \"127.0.0.1:34907\"\n[TLS]\nserver_name = \"x\"");
    }

    #[cfg(feature = "auth")]
    #[tokio::test(flavor = "multi_thread")]
    async fn auth_over_tls() {
        let pki = Pki::new("authtls");
        let ca1 = ca();
        let ca_path = pki.ca("ca", &ca1);
        let srv_cert = pki.leaf("server", &ca1);

        let (mut srv, srv_last) =
            server(34908, &format!("auth_secret = \"s3cret\"\n{}", server_tls(&srv_cert, ""))).await;
        let (mut cli, cli_last, _t) =
            client(34908, "c", &format!("auth_secret = \"s3cret\"\n[TLS]\nca = \"{ca_path}\"")).await;
        assert!(wait_for(|| srv.peer_count() == 1, Duration::from_secs(3)).await);
        assert_syncs(&mut srv, &srv_last, &mut cli, &cli_last).await;
    }
}

#[cfg(feature = "auth")]
mod auth {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn matching_secrets_connect() {
        let (mut srv, srv_last) = server(34921, "auth_secret = \"hunter2\"").await;
        let (mut cli, cli_last, _t) = client(34921, "c", "auth_secret = \"hunter2\"").await;
        assert!(wait_for(|| srv.peer_count() == 1, Duration::from_secs(3)).await);
        assert_syncs(&mut srv, &srv_last, &mut cli, &cli_last).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn secret_from_env_var() {
        std::env::set_var("HIVE_TEST_SECRET_34922", "from-env");
        let (srv, _) = server(34922, "auth_secret_env = \"HIVE_TEST_SECRET_34922\"").await;
        let (_cli, _, _t) = client(34922, "c", "auth_secret = \"from-env\"").await;
        assert!(wait_for(|| srv.peer_count() == 1, Duration::from_secs(3)).await);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wrong_or_missing_secret_is_rejected_both_ways() {
        let (srv, _) = server(34923, "auth_secret = \"right\"").await;
        let (wrong, _, _t1) = client(34923, "wrong", "auth_secret = \"wrong\"").await;
        let (none, _, _t2) = client(34923, "none", "").await;

        // A server with no secret must also be refused by a client that has one.
        let (open_srv, _) = server(34924, "").await;
        let (strict, _, _t3) = client(34924, "strict", "auth_secret = \"right\"").await;

        tokio::time::sleep(REJECT_WAIT).await;
        assert_eq!(srv.peer_count(), 0, "a client without the right secret got in");
        assert_eq!(wrong.peer_count(), 0);
        assert_eq!(none.peer_count(), 0);
        assert_eq!(open_srv.peer_count(), 0, "server without a secret accepted an auth client");
        assert_eq!(strict.peer_count(), 0);
    }

    #[test]
    #[should_panic(expected = "auth secret is empty")]
    fn empty_secret_refuses_to_start() {
        Hive::new_from_str_unknown("listen = \"127.0.0.1:34925\"\nauth_secret = \"\"");
    }
}
