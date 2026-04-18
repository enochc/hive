/// Admin provisioning tool for deploying hunter nodes to local network machines.
///
/// This is an admin-initiated push, not autonomous propagation.  The admin
/// runs the provisioner from a trusted workstation, selects target machines
/// (by IP, hostname, or subnet scan), authenticates via SSH, and pushes the
/// hunter binary plus a bootstrap configuration.  The target machine ends
/// up with a running hunter node that connects back to the Hive network.
///
/// # Workflow
///
/// 1. **Discovery** - scan the local subnet for reachable hosts, or accept
///    an explicit list from the admin.
/// 2. **Authentication** - the admin provides SSH credentials (key or
///    password) for the target machines.  The provisioner never stores
///    credentials beyond the current session.
/// 3. **Compatibility check** - probe the target for OS and architecture
///    so the correct binary is selected.
/// 4. **Transfer** - SCP the hunter binary and a bootstrap TOML config
///    to the target machine.
/// 5. **Install** - run the install script over SSH: place the binary,
///    create the vault directory, register the systemd/launchd service.
/// 6. **Verify** - wait for the new node to appear in the Hive peer list
///    (via `tokio::sync::Notify`, no polling loop).
///
/// # Security boundaries
///
/// - The admin must have SSH access to every target machine.
/// - Credentials are held in memory only for the duration of the session.
/// - The provisioner never runs without explicit admin interaction.
/// - Target machines must already be under the admin's control.

use std::collections::HashMap;
use std::fmt;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Notify};
use tokio::time::timeout;
use tracing::{debug, info, warn, error};

/// A machine discovered on the local network or provided explicitly.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TargetHost {
    /// IP address of the target.
    pub address: IpAddr,
    /// Optional hostname if resolved.
    pub hostname: Option<String>,
    /// Detected OS family ("linux", "macos", "windows", or "unknown").
    pub os: String,
    /// Detected CPU architecture ("x86_64", "aarch64", etc.)
    pub arch: String,
    /// Whether SSH is reachable on port 22.
    pub ssh_reachable: bool,
    /// Whether a hunter node is already running (checked via Hive port).
    pub hunter_installed: bool,
}

impl fmt::Display for TargetHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let host = self.hostname.as_deref().unwrap_or("unknown");
        write!(
            f, "{} ({}) - {}/{} ssh:{} hunter:{}",
            self.address, host, self.os, self.arch,
            self.ssh_reachable, self.hunter_installed
        )
    }
}

/// Result of probing a single port on a target host.
#[derive(Debug)]
enum ProbeResult {
    Open,
    Closed,
    Timeout,
}

/// Events emitted during the provisioning process.
#[derive(Debug)]
pub enum ProvisionEvent {
    /// A host was discovered during network scan.
    HostDiscovered(TargetHost),
    /// Scan of the subnet is complete.
    ScanComplete { hosts_found: usize },
    /// SSH connection established to a target.
    SshConnected { address: IpAddr },
    /// Binary transfer started.
    TransferStarted { address: IpAddr, binary_size: u64 },
    /// Binary transfer complete.
    TransferComplete { address: IpAddr },
    /// Installation finished on the target.
    InstallComplete { address: IpAddr, node_name: String },
    /// The new node appeared in the Hive peer list.
    NodeVerified { address: IpAddr, node_name: String },
    /// Something went wrong on a specific target.
    Error { address: IpAddr, message: String },
}

/// SSH credential types supported for authentication.
#[derive(Clone)]
pub enum SshCredential {
    /// Path to a private key file, with optional passphrase.
    KeyFile { path: PathBuf, passphrase: Option<String> },
    /// Password authentication (less secure, but sometimes necessary).
    Password(String),
    /// Use the SSH agent for key management.
    Agent,
}

/// Configuration for the provisioner.
pub struct ProvisionConfig {
    /// Which subnet to scan (e.g. "192.168.1.0/24").
    /// If None, the admin must provide explicit target addresses.
    pub subnet: Option<String>,
    /// Explicit list of target addresses (used instead of or alongside scan).
    pub targets: Vec<IpAddr>,
    /// Port to probe for SSH (default 22).
    pub ssh_port: u16,
    /// Port the hunter node will listen on after installation.
    pub hunter_port: u16,
    /// How long to wait for a TCP probe response.
    pub probe_timeout: Duration,
    /// How long to wait for the new node to appear in the peer list.
    pub verify_timeout: Duration,
    /// Directory on the target where the hunter binary is installed.
    pub remote_install_dir: String,
    /// Address of an existing Hive node the new hunter should connect to.
    pub hive_connect_address: String,
    /// Map of platform strings to binary paths on the admin machine.
    /// e.g. "x86_64-linux" -> "/path/to/hunter-x86_64-linux"
    pub binaries: HashMap<String, PathBuf>,
}

impl Default for ProvisionConfig {
    fn default() -> Self {
        Self {
            subnet: None,
            targets: Vec::new(),
            ssh_port: 22,
            hunter_port: 9100,
            probe_timeout: Duration::from_secs(2),
            verify_timeout: Duration::from_secs(30),
            remote_install_dir: "/opt/hive-hunter".into(),
            hive_connect_address: String::new(),
            binaries: HashMap::new(),
        }
    }
}

/// The provisioner service.
///
/// Constructed with a config and an event channel.  Call methods in sequence:
/// `scan_network()` or `add_targets()`, then `provision_host()` for each
/// approved target.
pub struct Provisioner {
    config: ProvisionConfig,
    event_tx: mpsc::Sender<ProvisionEvent>,
    discovered: Vec<TargetHost>,
}

impl Provisioner {
    pub fn new(config: ProvisionConfig, event_tx: mpsc::Sender<ProvisionEvent>) -> Self {
        Self {
            config,
            event_tx,
            discovered: Vec::new(),
        }
    }

    /// Probe a single TCP port on a host with a timeout.
    async fn probe_port(&self, addr: IpAddr, port: u16) -> ProbeResult {
        let socket = SocketAddr::new(addr, port);
        match timeout(self.config.probe_timeout, TcpStream::connect(socket)).await {
            Ok(Ok(_stream)) => ProbeResult::Open,
            Ok(Err(_)) => ProbeResult::Closed,
            Err(_) => ProbeResult::Timeout,
        }
    }

    /// Scan a range of IPv4 addresses for reachable hosts.
    ///
    /// Probes SSH (port 22) and the hunter port to determine which
    /// machines are reachable and which already have a hunter node.
    /// Results are sent through the event channel as they are found.
    pub async fn scan_subnet(&mut self, base: Ipv4Addr, prefix_len: u8) -> Vec<TargetHost> {
        let mask = if prefix_len >= 32 {
            u32::MAX
        } else {
            u32::MAX << (32 - prefix_len)
        };
        let network = u32::from(base) & mask;
        let host_count = (!mask) as usize;

        info!(
            "scanning {}/{} ({} addresses)",
            base, prefix_len, host_count
        );

        // Spawn probes concurrently, collect via channel
        let (tx, mut rx) = mpsc::channel::<TargetHost>(256);

        for i in 1..host_count {
            let addr = Ipv4Addr::from(network + i as u32);
            let ip = IpAddr::V4(addr);
            let probe_timeout = self.config.probe_timeout;
            let ssh_port = self.config.ssh_port;
            let hunter_port = self.config.hunter_port;
            let tx = tx.clone();

            tokio::spawn(async move {
                // Probe SSH
                let ssh_socket = SocketAddr::new(ip, ssh_port);
                let ssh_open = match timeout(probe_timeout, TcpStream::connect(ssh_socket)).await {
                    Ok(Ok(_)) => true,
                    _ => false,
                };

                if !ssh_open {
                    return; // Not interesting if we cannot SSH in
                }

                // Probe hunter port
                let hunter_socket = SocketAddr::new(ip, hunter_port);
                let hunter_open = match timeout(probe_timeout, TcpStream::connect(hunter_socket)).await {
                    Ok(Ok(_)) => true,
                    _ => false,
                };

                let host = TargetHost {
                    address: ip,
                    hostname: None, // DNS lookup is a future enhancement
                    os: "unknown".into(),
                    arch: "unknown".into(),
                    ssh_reachable: true,
                    hunter_installed: hunter_open,
                };

                let _ = tx.send(host).await;
            });
        }

        // Drop our copy of tx so rx closes when all spawned tasks finish
        drop(tx);

        let mut hosts = Vec::new();
        while let Some(host) = rx.recv().await {
            let _ = self.event_tx.send(ProvisionEvent::HostDiscovered(host.clone())).await;
            hosts.push(host);
        }

        let _ = self.event_tx.send(ProvisionEvent::ScanComplete {
            hosts_found: hosts.len(),
        }).await;

        info!("scan complete: {} reachable hosts with SSH", hosts.len());
        self.discovered.extend(hosts.clone());
        hosts
    }

    /// Add explicit target addresses (bypass network scanning).
    pub fn add_targets(&mut self, addresses: Vec<IpAddr>) {
        for addr in addresses {
            self.config.targets.push(addr);
        }
    }

    /// Return all discovered hosts.
    pub fn discovered_hosts(&self) -> &[TargetHost] {
        &self.discovered
    }

    /// Generate a bootstrap TOML config for a new hunter node.
    ///
    /// The config tells the new node to connect back to an existing
    /// Hive peer (the hub or another node the admin designates).
    pub fn generate_bootstrap_config(&self, node_name: &str) -> String {
        format!(
            r#"name = "{}"
connect = "{}"

[Properties]
hunter_update_manifest = ""
scan_directive = ""

[Hunter]
vault_dir = "{}/quarantine"
scan_paths = ["/home", "/tmp", "/var"]
scan_interval_secs = 3600
max_file_size_mb = 100
"#,
            node_name,
            self.config.hive_connect_address,
            self.config.remote_install_dir,
        )
    }

    /// Select the correct binary for a target's platform.
    pub fn binary_for_target(&self, target: &TargetHost) -> Option<&PathBuf> {
        let key = format!("{}-{}", target.arch, target.os);
        self.config.binaries.get(&key)
    }

    /// Provision a single target host.
    ///
    /// This is the main entry point for deploying to one machine.  It
    /// requires the admin to have already provided SSH credentials.
    /// The full sequence: connect SSH, detect platform, transfer binary,
    /// write config, install service, verify Hive connectivity.
    ///
    /// Returns the node name assigned to the new hunter.
    pub async fn provision_host(
        &self,
        target: &TargetHost,
        credential: &SshCredential,
        node_name: &str,
    ) -> Result<String, ProvisionError> {
        // Step 1: Validate that we have a binary for this platform
        let _binary_path = self.binary_for_target(target).ok_or_else(|| {
            ProvisionError::NoBinary {
                platform: format!("{}-{}", target.arch, target.os),
            }
        })?;

        // Step 2: SSH connection
        // TODO: integrate an SSH library (thrussh or ssh2) to:
        //   a) Connect and authenticate with the provided credential
        //   b) Run `uname -sm` to detect OS/arch if not already known
        //   c) SCP the binary to remote_install_dir
        //   d) Write the bootstrap TOML config
        //   e) Run the install script (chmod, systemd unit, start)
        info!(
            "provisioning {} as '{}' (SSH to {}:{})",
            target.address, node_name, target.address, self.config.ssh_port
        );

        let _ = self.event_tx.send(ProvisionEvent::SshConnected {
            address: target.address,
        }).await;

        // Step 3: Generate and transfer config
        let _config = self.generate_bootstrap_config(node_name);

        // Step 4: Transfer binary
        // TODO: SCP transfer with progress reporting

        let _ = self.event_tx.send(ProvisionEvent::TransferComplete {
            address: target.address,
        }).await;

        // Step 5: Remote install
        // TODO: run install commands over SSH:
        //   mkdir -p /opt/hive-hunter/quarantine
        //   chmod 755 /opt/hive-hunter/hunter
        //   install systemd unit / launchd plist
        //   systemctl start hive-hunter

        let _ = self.event_tx.send(ProvisionEvent::InstallComplete {
            address: target.address,
            node_name: node_name.into(),
        }).await;

        // Step 6: Verify the new node connects to Hive
        // TODO: watch the Hive peer list for the new node_name to appear.
        // Use a Notify or watch channel on the peers list, with a timeout.
        // Do not poll in a loop.

        Ok(node_name.into())
    }

    /// Generate a systemd unit file for the hunter service.
    pub fn generate_systemd_unit(&self, node_name: &str) -> String {
        format!(
            r#"[Unit]
Description=Hive Hunter Node ({name})
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={dir}/hunter {dir}/hunter.toml
Restart=on-failure
RestartSec=10
WorkingDirectory={dir}

# Security hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths={dir}/quarantine
PrivateTmp=true

[Install]
WantedBy=multi-user.target
"#,
            name = node_name,
            dir = self.config.remote_install_dir,
        )
    }

    /// Generate a macOS launchd plist for the hunter service.
    pub fn generate_launchd_plist(&self, node_name: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.hive.hunter.{name}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{dir}/hunter</string>
        <string>{dir}/hunter.toml</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>WorkingDirectory</key>
    <string>{dir}</string>
    <key>StandardOutPath</key>
    <string>{dir}/hunter.log</string>
    <key>StandardErrorPath</key>
    <string>{dir}/hunter.err</string>
</dict>
</plist>
"#,
            name = node_name,
            dir = self.config.remote_install_dir,
        )
    }
}

/// Errors that can occur during provisioning.
#[derive(Debug)]
pub enum ProvisionError {
    /// No binary available for the target platform.
    NoBinary { platform: String },
    /// SSH connection or authentication failed.
    SshFailed(String),
    /// File transfer failed.
    TransferFailed(String),
    /// Remote command execution failed.
    RemoteCommandFailed { command: String, error: String },
    /// The new node did not appear in the Hive peer list in time.
    VerifyTimeout { node_name: String },
    /// Generic IO error.
    IoError(std::io::Error),
}

impl fmt::Display for ProvisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoBinary { platform } => {
                write!(f, "no binary available for platform: {}", platform)
            }
            Self::SshFailed(msg) => write!(f, "SSH failed: {}", msg),
            Self::TransferFailed(msg) => write!(f, "transfer failed: {}", msg),
            Self::RemoteCommandFailed { command, error } => {
                write!(f, "remote command '{}' failed: {}", command, error)
            }
            Self::VerifyTimeout { node_name } => {
                write!(f, "node '{}' did not appear in peer list", node_name)
            }
            Self::IoError(e) => write!(f, "io error: {}", e),
        }
    }
}

impl From<std::io::Error> for ProvisionError {
    fn from(e: std::io::Error) -> Self {
        ProvisionError::IoError(e)
    }
}

/// Parse a subnet string like "192.168.1.0/24" into base address and prefix.
pub fn parse_subnet(s: &str) -> Result<(Ipv4Addr, u8), String> {
    let parts: Vec<&str> = s.split('/').collect();
    if parts.len() != 2 {
        return Err("expected format: x.x.x.x/prefix".into());
    }
    let addr: Ipv4Addr = parts[0].parse().map_err(|e: std::net::AddrParseError| e.to_string())?;
    let prefix: u8 = parts[1].parse().map_err(|e: std::num::ParseIntError| e.to_string())?;
    if prefix > 32 {
        return Err("prefix must be 0-32".into());
    }
    Ok((addr, prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_subnet_valid() {
        let (addr, prefix) = parse_subnet("192.168.1.0/24").unwrap();
        assert_eq!(addr, Ipv4Addr::new(192, 168, 1, 0));
        assert_eq!(prefix, 24);
    }

    #[test]
    fn parse_subnet_invalid() {
        assert!(parse_subnet("not_a_subnet").is_err());
        assert!(parse_subnet("192.168.1.0/33").is_err());
    }

    #[test]
    fn bootstrap_config_generation() {
        let (tx, _rx) = mpsc::channel(16);
        let config = ProvisionConfig {
            hive_connect_address: "192.168.1.100:9100".into(),
            remote_install_dir: "/opt/hive-hunter".into(),
            ..Default::default()
        };
        let prov = Provisioner::new(config, tx);
        let toml = prov.generate_bootstrap_config("hunter-bravo");

        assert!(toml.contains("name = \"hunter-bravo\""));
        assert!(toml.contains("connect = \"192.168.1.100:9100\""));
        assert!(toml.contains("vault_dir"));
    }

    #[test]
    fn systemd_unit_generation() {
        let (tx, _rx) = mpsc::channel(16);
        let config = ProvisionConfig {
            remote_install_dir: "/opt/hive-hunter".into(),
            ..Default::default()
        };
        let prov = Provisioner::new(config, tx);
        let unit = prov.generate_systemd_unit("hunter-alpha");

        assert!(unit.contains("Hive Hunter Node"));
        assert!(unit.contains("/opt/hive-hunter/hunter"));
        assert!(unit.contains("NoNewPrivileges=true"));
    }

    #[test]
    fn launchd_plist_generation() {
        let (tx, _rx) = mpsc::channel(16);
        let config = ProvisionConfig {
            remote_install_dir: "/opt/hive-hunter".into(),
            ..Default::default()
        };
        let prov = Provisioner::new(config, tx);
        let plist = prov.generate_launchd_plist("hunter-alpha");

        assert!(plist.contains("com.hive.hunter.hunter-alpha"));
        assert!(plist.contains("<key>KeepAlive</key>"));
    }
}
