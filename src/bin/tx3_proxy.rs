use clap::Parser;
use tx3::*;

type Result<T> = std::result::Result<T, String>;

#[derive(Debug, Parser)]
#[clap(
    name = "tx3-proxy",
    version,
    about = "TCP splicing relay for tx3 p2p communications"
)]
struct Opt {
    /// Initialize a new tx3-proxy.yml configuration file
    /// (as specified by --config).
    /// Will abort if it already exists.
    #[clap(short, long, verbatim_doc_comment)]
    init: bool,

    /// Configuration file to use for running the
    /// tx3-proxy.
    #[clap(
        short,
        long,
        verbatim_doc_comment,
        default_value = "./tx3-proxy.yml"
    )]
    config: std::path::PathBuf,
}

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
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_env_filter(
            tracing_subscriber::filter::EnvFilter::from_default_env(),
        )
        .with_file(true)
        .with_line_number(true)
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();

    if let Err(err) = main_err().await {
        eprintln!("{}", err);
        std::process::exit(1);
    }
}

async fn main_err() -> Result<()> {
    let opt = Opt::parse();

    if opt.init {
        return run_init(opt).await;
    }

    let conf = read_config(opt).await?;

    let relay = Tx3Relay::new(conf)
        .await
        .map_err(|err| format!("{:?}", err))?;

    println!("# tx3-proxy listening #");
    println!("# tx3-proxy address list start #");
    for addr in relay.local_addrs() {
        println!("{}", addr);
    }
    println!("# tx3-proxy address list end #");

    futures::future::pending().await
}

async fn read_config(opt: Opt) -> Result<Tx3RelayConfig> {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::AsyncReadExt;

    let mut file = match tokio::fs::OpenOptions::new()
        .read(true)
        .open(&opt.config)
        .await
    {
        Err(err) => {
            return Err(format!(
                "Failed to open config file {:?}: {:?}",
                opt.config, err,
            ))
        }
        Ok(file) => file,
    };

    let perms = match file.metadata().await {
        Err(err) => {
            return Err(format!(
                "Failed to load config file metadata {:?}: {:?}",
                opt.config, err
            ))
        }
        Ok(perms) => perms.permissions(),
    };

    if !perms.readonly() {
        return Err(format!(
            "Refusing to run with writable config file {:?}",
            opt.config
        ));
    }

    #[cfg(unix)]
    {
        let mode = perms.mode() & 0o777;
        if mode != 0o400 {
            return Err(format!(
                "Refusing to run with config file not set to mode 0o400 {:?} 0o{:o}",
                opt.config,
                mode,
            ));
        }
    }

    let mut conf = String::new();
    if let Err(err) = file.read_to_string(&mut conf).await {
        return Err(format!(
            "Failed to read config file {:?}: {:?}",
            opt.config, err,
        ));
    }

    let conf: Tx3ProxyConfigFile = match serde_yaml::from_str(&conf) {
        Err(err) => {
            return Err(format!(
                "Failed to parse config file {:?}: {:?}",
                opt.config, err,
            ))
        }
        Ok(res) => res,
    };

    let Tx3ProxyConfigFile {
        mut tx3_relay,
        tls_cert_der,
        tls_cert_pk_der,
    } = conf;

    let tls_cert_der = match base64::decode(&tls_cert_der) {
        Err(err) => {
            return Err(format!(
                "Failed to parse config file {:?}: {:?}",
                opt.config, err,
            ))
        }
        Ok(cert) => tls::TlsCertDer(cert.into_boxed_slice()),
    };

    let tls_cert_pk_der = match base64::decode(&tls_cert_pk_der) {
        Err(err) => {
            return Err(format!(
                "Failed to parse config file {:?}: {:?}",
                opt.config, err,
            ))
        }
        Ok(pk) => tls::TlsPkDer(pk.into_boxed_slice()),
    };

    let tls = match tls::TlsConfigBuilder::default()
        .with_cert(tls_cert_der, tls_cert_pk_der)
        .build()
    {
        Err(err) => {
            return Err(format!(
                "Failed to build TlsConfig from config file {:?}: {:?}",
                opt.config, err,
            ))
        }
        Ok(tls) => tls,
    };

    tx3_relay.tls = Some(tls);
    Ok(tx3_relay)
}

async fn run_init(opt: Opt) -> Result<()> {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::AsyncWriteExt;

    let mut file = tokio::fs::OpenOptions::new();
    file.create_new(true);
    file.write(true);
    let mut file = match file.open(&opt.config).await {
        Err(err) => {
            return Err(format!(
                "Failed to create config file {:?}: {:?}",
                opt.config, err,
            ))
        }
        Ok(file) => file,
    };

    let (cert, cert_pk) = tls::gen_tls_cert_pair().unwrap();
    let cert = base64::encode(&cert.0);
    let cert_pk = base64::encode(&cert_pk.0);

    let conf = Tx3ProxyConfigFile {
        tx3_relay: Tx3RelayConfig::default().with_bind("tx3-rst://0.0.0.0:0"),
        tls_cert_der: cert,
        tls_cert_pk_der: cert_pk,
    };
    let conf = serde_yaml::to_string(&conf).unwrap();

    if let Err(err) = file.write_all(conf.as_bytes()).await {
        return Err(format!(
            "Failed to initialize config file {:?}: {:?}",
            opt.config, err
        ));
    };

    let mut perms = match file.metadata().await {
        Err(err) => {
            return Err(format!(
                "Failed to load config file metadata {:?}: {:?}",
                opt.config, err,
            ))
        }
        Ok(perms) => perms.permissions(),
    };
    perms.set_readonly(true);

    #[cfg(unix)]
    perms.set_mode(0o400);

    if let Err(err) = file.set_permissions(perms).await {
        return Err(format!(
            "Failed to set config file permissions {:?}: {:?}",
            opt.config, err,
        ));
    }

    if let Err(err) = file.shutdown().await {
        return Err(format!("Failed to flush/close config file: {:?}", err));
    }

    Ok(())
}
