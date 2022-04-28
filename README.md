# tx3

tx3 p2p communications

#### Run a locally addressable tx3 node

```rust
let tx3_config = tx3::Tx3Config::new().with_bind("tx3-st://127.0.0.1:0");

let (node, _inbound_con) = tx3::Tx3Node::new(tx3_config).await.unwrap();

println!("listening on addresses: {:#?}", node.local_addrs());
```

#### Run an in-process relay node

Note: Unless you're writing test code, you probably want the executable.
See below for `tx3-relay` commandline flags and options.

```rust
let tx3_relay_config = tx3::Tx3RelayConfig::new()
    .with_bind("tx3-rst://127.0.0.1:0");
let relay = tx3::Tx3Relay::new(tx3_relay_config).await.unwrap();

println!("relay listening on addresses: {:#?}", relay.local_addrs());
```

#### Run a tx3 node relayed over the given relay address

```rust
// set relay_addr to your relay address, something like:
// let relay_addr = "tx3-rst://127.0.0.1:38141/EHoKZ3-8R520Unp3vr4xeP6ogYAqoZ-em8lm-rMlwhw";

let tx3_config = tx3::Tx3Config::new().with_bind(relay_addr);

let (node, _inbound_con) = tx3::Tx3Node::new(tx3_config).await.unwrap();

println!("listening on addresses: {:#?}", node.local_addrs());
```

### The `tx3-relay` executable
`tx3-relay --help`
```no-compile
tx3-relay 0.0.1
TCP splicing relay for tx3 p2p communications

USAGE:
    tx3-relay [OPTIONS]

OPTIONS:
    -c, --config <CONFIG>    Configuration file to use for running the
                             tx3-relay. [default: ./tx3-relay.yml]
    -h, --help               Print help information
    -i, --init               Initialize a new tx3-relay.yml configuration file
                             (as specified by --config).
                             Will abort if it already exists.
    -V, --version            Print version information

```
