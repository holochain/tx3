use tx3::*;

#[non_exhaustive]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tx3ProxyConfigFile {
    /// tx3-proxy config file
    #[serde(flatten)]
    pub tx3_relay: Tx3RelayConfig,

    /// der-encoded certificate
    pub tls_cert_der: String,

    /// der-encoded private key
    /// despite the recommendation in Tx3Config, we *are* going to
    /// put the plaintext private key in here, for usability / systemd restart
    /// we'll just do our best to set the file permissions sanely
    pub tls_cert_pk_der: String,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let (cert, cert_pk) = tls::gen_tls_cert_pair().unwrap();
    let cert = base64::encode(&cert.0);
    let cert_pk = base64::encode(&cert_pk.0);

    let conf = Tx3ProxyConfigFile {
        tx3_relay: Tx3RelayConfig::default().with_bind("tx3-rst://0.0.0.0:0"),
        tls_cert_der: cert,
        tls_cert_pk_der: cert_pk,
    };
    let conf = serde_yaml::to_string(&conf).unwrap();
    println!("{}", conf);
}
