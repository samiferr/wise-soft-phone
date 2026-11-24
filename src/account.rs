use std::fs;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rvoip_sip::{Endpoint, EndpointProfile, EndpointSrtpMode, EndpointTransport};
use serde::{Deserialize, Serialize};
use tokio::net::lookup_host;
use tokio::time::timeout;

static STORE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Account {
    pub registrar: String,
    pub username: String,
    pub auth_username: String,
    pub outbound_proxy: String,
    pub secure: bool,
    pub remember_password: bool,
}

impl Account {
    pub fn load(path: &Path) -> Result<Self, String> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "Could not read the saved SIP account settings.".into()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(_) => Err("Could not open the saved SIP account settings.".into()),
        }
    }

    pub fn validate(&mut self) -> Result<(), String> {
        self.registrar = self.registrar.trim().to_ascii_lowercase();
        self.username = self.username.trim().into();
        self.auth_username = self.auth_username.trim().into();
        self.outbound_proxy = self.outbound_proxy.trim().to_ascii_lowercase();
        self.server()?;
        if !self.outbound_proxy.is_empty() && !valid_server(&self.outbound_proxy) {
            return Err(
                "Enter an outbound proxy hostname or IPv4 address, optionally followed by :port."
                    .into(),
            );
        }

        if self.username.is_empty()
            || !self
                .username
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.!~*'+".contains(&byte))
        {
            return Err("Enter a valid SIP username or extension.".into());
        }
        if self.auth_username.chars().any(char::is_control) {
            return Err("Enter a valid authentication username.".into());
        }
        Ok(())
    }

    fn server(&self) -> Result<(&str, u16), String> {
        let (host, port) = match self.registrar.split_once(':') {
            Some((host, port)) => (host, port.parse::<u16>().ok().filter(|port| *port != 0)),
            None => (
                self.registrar.as_str(),
                Some(if self.secure { 5061 } else { 5060 }),
            ),
        };
        let valid_host = valid_host(host);
        match (valid_host, port) {
            (true, Some(port)) => Ok((host, port)),
            _ => Err("Enter a PBX hostname or IPv4 address, optionally followed by :port.".into()),
        }
    }

    fn proxy_uri(&self) -> Option<String> {
        let (host, _) = self.server().ok()?;
        let proxy = if !self.outbound_proxy.is_empty() {
            self.outbound_proxy.as_str()
        } else if host.ends_with(".twilio.com") {
            self.registrar.as_str()
        } else {
            return None;
        };
        Some(format!(
            "{}:{proxy};lr",
            if self.secure { "sips" } else { "sip" }
        ))
    }

    fn credential_name(&self) -> String {
        format!(
            "{}:{}@{}",
            self.username, self.auth_username, self.registrar
        )
    }

    pub fn store(self, path: &Path, password: Option<String>) -> Result<String, String> {
        let _guard = STORE_LOCK.try_lock().map_err(
            |_| "A credential operation is still running. Unlock the system keyring and retry.",
        )?;
        let previous = Self::load(path)?;
        let saved =
            previous.remember_password && previous.credential_name() == self.credential_name();
        let password = match password.filter(|password| !password.is_empty()) {
            Some(password) => password,
            None if self.remember_password && saved => {
                crate::credentials::get(&self.credential_name())
                    .map_err(|_| "Could not read the saved password. Enter it again.".to_owned())?
            }
            None => return Err("Enter the SIP password.".into()),
        };

        if password.contains(['\r', '\n', '\0']) {
            return Err("The SIP password contains an invalid character.".into());
        }

        // Check and stage the settings write before touching saved credentials.
        // A full transaction spanning a filesystem and Secret Service is not available.
        let staged = StagedSettings::new(path, &self)?;
        if self.remember_password {
            crate::credentials::set(&self.credential_name(), &password).map_err(|_| {
                "Could not save the SIP password in the system credential store.".to_owned()
            })?;
        }
        if previous.remember_password && (!saved || !self.remember_password) {
            crate::credentials::remove(&previous.credential_name())
                .map_err(|_| "Could not remove the previous saved SIP password.".to_owned())?;
        }

        fs::rename(&staged.0, path).map_err(
            |_| "Could not save the account settings. Re-enter the password when retrying.",
        )?;
        Ok(password)
    }

    pub async fn endpoint(&self, password: String) -> Result<Endpoint, String> {
        let (host, port) = self.server()?;
        let candidates = timeout(Duration::from_secs(5), lookup_host((host, port)))
            .await
            .map_err(|_| "Looking up the PBX timed out.")?
            .map_err(|_| "Could not resolve the PBX hostname.")?;

        // Select the interface the OS would use to reach the PBX, including VPN routes.
        let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))
            .map_err(|_| "Could not open a local SIP socket.")?;
        let address = candidates
            .filter(SocketAddr::is_ipv4)
            .find_map(|remote| {
                socket.connect(remote).ok()?;
                socket.local_addr().ok()
            })
            .filter(|local| !local.ip().is_unspecified())
            .ok_or("Could not find an IPv4 network route to the PBX.")?;
        // RVoIP 0.4 can retain :0 in dialog routing when configured with an
        // ephemeral bind. Choose the concrete port before constructing its
        // identity so Contact and the transport agree on a reachable port.
        let address = if self.secure {
            let listener = TcpListener::bind((address.ip(), 0))
                .map_err(|_| "Could not choose a local TLS listening port.")?;
            listener
                .local_addr()
                .map_err(|_| "Could not read the TLS listening address.")?
        } else {
            address
        };
        drop(socket);
        tracing::debug!(local = %address, secure = self.secure, "Selected SIP listening address");
        self.build_endpoint(address, port, password).await
    }

    async fn build_endpoint(
        &self,
        address: SocketAddr,
        port: u16,
        password: String,
    ) -> Result<Endpoint, String> {
        let (host, _) = self.server()?;
        let (scheme, transport, profile, srtp) = if self.secure {
            (
                "sips",
                EndpointTransport::Tls,
                EndpointProfile::AsteriskTlsSrtpRegisteredFlow,
                EndpointSrtpMode::Required,
            )
        } else {
            (
                "sip",
                EndpointTransport::Udp,
                EndpointProfile::AsteriskUdp,
                EndpointSrtpMode::Off,
            )
        };
        let mut builder = Endpoint::builder()
            .name("Softphone")
            .enable_sip_trace()
            .account(&self.username)
            .password(password)
            .registrar(format!("{scheme}:{host}:{port}"))
            .profile(profile)
            .transport(transport)
            .srtp(srtp)
            .bind_addr(address)
            .auto_180_ringing(true);

        if let Some(proxy) = self.proxy_uri() {
            tracing::info!("Routing SIP through the configured provider edge");
            builder = builder.outbound_proxy(proxy);
        }
        if !self.auth_username.is_empty() {
            builder = builder.auth_username(&self.auth_username);
        }
        builder
            .build()
            .await
            .map_err(|_| "Could not start the SIP endpoint.".into())
    }
}

struct StagedSettings(PathBuf);

impl StagedSettings {
    fn new(path: &Path, account: &Account) -> Result<Self, String> {
        let parent = path
            .parent()
            .ok_or("The settings directory is unavailable.")?;
        fs::create_dir_all(parent).map_err(|_| "Could not create the settings directory.")?;
        let staged = Self(path.with_extension(format!("{}.tmp", std::process::id())));
        let bytes =
            serde_json::to_vec_pretty(account).map_err(|_| "Could not encode the SIP account.")?;
        fs::write(&staged.0, bytes).map_err(|_| "Could not write the account settings.")?;
        Ok(staged)
    }
}

impl Drop for StagedSettings {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|part| {
            !part.is_empty()
                && part.len() <= 63
                && !part.starts_with('-')
                && !part.ends_with('-')
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

pub(crate) fn valid_server(server: &str) -> bool {
    match server.split_once(':') {
        Some((host, port)) => valid_host(host) && port.parse::<u16>().is_ok_and(|port| port != 0),
        None => valid_host(server),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_and_normalizes_accounts() {
        let mut account = Account {
            registrar: " PBX.Example.com:5060 ".into(),
            username: " 1001 ".into(),
            ..Default::default()
        };
        assert!(account.validate().is_ok());
        assert_eq!(account.registrar, "pbx.example.com:5060");
        assert_eq!(account.username, "1001");
        for host in [
            "",
            "https://pbx.example.com",
            "host:0",
            "host:65536",
            "host:abc",
            "a..b",
            "-host",
            "host/path",
        ] {
            account.registrar = host.into();
            assert!(account.validate().is_err(), "{host}");
        }
    }

    #[test]
    fn secure_accounts_default_to_tls_port() {
        let account = Account {
            registrar: "pbx.example.com".into(),
            secure: true,
            ..Default::default()
        };
        assert_eq!(account.server().unwrap().1, 5061);
    }

    #[test]
    fn invalid_password_is_rejected_before_writing_settings() {
        let path = std::env::temp_dir().join(format!(
            "softphone-invalid-{}/account.json",
            std::process::id()
        ));
        let account = Account {
            registrar: "host".into(),
            username: "1001".into(),
            ..Default::default()
        };
        assert!(
            account
                .store(&path, Some("bad\r\npassword".into()))
                .is_err()
        );
        assert!(!path.exists());
    }
}

#[cfg(test)]
mod proxy_tests {
    use super::*;
    #[test]
    fn twilio_uses_its_edge_automatically_and_allows_an_explicit_proxy() {
        let mut account = Account {
            registrar: "example.sip.twilio.com".into(),
            ..Default::default()
        };
        assert_eq!(
            account.proxy_uri().as_deref(),
            Some("sip:example.sip.twilio.com;lr")
        );
        account.secure = true;
        assert_eq!(
            account.proxy_uri().as_deref(),
            Some("sips:example.sip.twilio.com;lr")
        );
        account.registrar = "pbx.example.com".into();
        assert!(account.proxy_uri().is_none());
        account.outbound_proxy = "127.0.0.1:5090".into();
        assert_eq!(
            account.proxy_uri().as_deref(),
            Some("sips:127.0.0.1:5090;lr")
        );
        account.outbound_proxy = "invalid;route=evil".into();
        assert!(account.validate().is_err());
    }
}
