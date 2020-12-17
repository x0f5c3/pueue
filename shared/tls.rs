use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, Context, Error, Result};
use async_tls::TlsConnector;
use rustls::internal::pemfile::{certs, rsa_private_keys};
use rustls::{Certificate, ClientConfig, PrivateKey, ServerConfig};

use crate::settings::Settings;

/// Initialize our client [TlsConnector].
/// 1. Trust our own CA. ONLY our own CA.
/// 2. Set the client certificate and key
pub async fn get_client_tls_connector(settings: &Settings) -> Result<TlsConnector> {
    let mut config = ClientConfig::new();

    // Trust server-certificates signed with our own CA.
    let mut ca = load_ca(&settings.shared.ca_cert)?;
    config
        .root_store
        .add_pem_file(&mut ca)
        .map_err(|_| anyhow!("Failed to add CA to client root store."))?;

    // Set the client-side key and certificate that should be used for any communication
    let certs = load_certs(&settings.shared.client_cert)?;
    let mut keys = load_keys(&settings.shared.client_key)?;
    config
        // set this server to use one cert together with the loaded private key
        .set_single_client_cert(certs, keys.remove(0))
        .map_err(|err| Error::new(err))
        .context("Failed to set single certificate for daemon.")?;

    config.enable_sni = false;

    Ok(TlsConnector::from(Arc::new(config)))
}

/// Configure the server using rusttls.
/// A TLS server needs a certificate and a fitting private key.
/// On top of that, we require authentication via client certificates.
/// We need to trust our own CA for that to work.
pub fn load_config(settings: &Settings) -> Result<ServerConfig> {
    // Initialize our cert store with our own CA.
    let mut root_store = rustls::RootCertStore::empty();
    let mut ca = load_ca(&settings.shared.ca_cert)?;
    root_store
        .add_pem_file(&mut ca)
        .map_err(|_| anyhow!("Failed to add CA to client root store."))?;

    // Only trust clients with a valid certificate of our own CA.
    let client_auth_only = rustls::AllowAnyAuthenticatedClient::new(root_store);
    let mut config = ServerConfig::new(client_auth_only);

    // Set the mtu to 1500, since we might have non-local communication.
    config.mtu = Some(1500);

    // Set the server-side key and certificate that should be used for any communication
    let certs = load_certs(&settings.shared.daemon_cert)?;
    let mut keys = load_keys(&settings.shared.daemon_key)?;
    config
        // set this server to use one cert together with the loaded private key
        .set_single_cert(certs, keys.remove(0))
        .map_err(|err| Error::new(err))
        .context("Failed to set single certificate for daemon.")?;

    Ok(config)
}

/// Load the passed certificates file
fn load_certs(path: &Path) -> Result<Vec<Certificate>> {
    certs(&mut BufReader::new(File::open(path)?))
        .map_err(|_| anyhow!("Failed to parse daemon certificate."))
}

/// Load the passed keys file
fn load_keys(path: &Path) -> Result<Vec<PrivateKey>> {
    rsa_private_keys(&mut BufReader::new(File::open(path)?))
        .map_err(|_| anyhow!("Failed to parse daemon key."))
}

fn load_ca(path: &Path) -> Result<Cursor<Vec<u8>>> {
    let file = std::fs::read(path).map_err(|_| anyhow!("Failed to read CA file."))?;
    Ok(Cursor::new(file))
}
