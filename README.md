# tx3

tx3 p2p communications

#### An interim Holochain networking solution

Holochain has experimented with many different networking solutions.
We mainly have our eye on webrtc, due to the benefits of direct STUN
p2p negotiation, with fallback TURN relaying for non-symmetric network
cases. We've also run on QUIC. For the time being, however, none of these
solutions have proven mature enough in the rust ecosystem to get us
the performance, reliability, and end-to-end encryption we need.

Tx3 provides a very simple interim stack: TLS over TCP. A relay server
splices raw TCP streams together providing addressability for clients
behind NATs. The clients negotiate end-to-end TLS over these spliced
streams, ensuring the relay server or any MITM has no access to the
plaintext. (If you want to learn more about the relay protocol, see
the [Tx3Relay] docs.)

```no-compile
       +-------+
       | relay | (TCP splicing)
       +-------+
        /     \
       /       \  (TLS over TCP)
      /         \
+-------+     +-------+
| node1 |     | node2 |
+-------+     +-------+
```rust

As with many p2p solutions, ensuring you are talking to who you think you
are is left to other layers of the application. But tx3 tries to make
this simple by including the TLS certificate digest in the url, so that
holochain can easily provide signature verification.

E.g. `tx3-rst://127.0.0.1:38141/EHoKZ3-8R520Unp3vr4xeP6ogYAqoZ-em8lm-rMlwhw`

Nodes that are directly addressable, or can configure port-forwarding are
welcome and encouraged to make direct connections, instead of relaying.

```no-compile
+-------+ (TLS over TCP) +-------+
| node1 |----------------| node2 |
+-------+                +-------+
```

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

#### Connect and receive connections

```rust
// create a listening node
let tx3_config = tx3::Tx3Config::new().with_bind("tx3-st://127.0.0.1:0");
let (node1, mut recv1) = tx3::Tx3Node::new(tx3_config).await.unwrap();
let addr1 = node1.local_addrs()[0].to_owned();

// listen for incoming connections
let task = tokio::task::spawn(async move {
    let acceptor = recv1.recv().await.unwrap();

    // in production code we might want to spawn this so we can
    // receive more connections, handling their handshakes in paralel
    let mut in_con = acceptor.accept().await.unwrap();

    // make sure we can read data
    let mut got = [0; 5];
    in_con.read_exact(&mut got).await.unwrap();
    assert_eq!(b"hello", &got[..]);

    // make sure we can write data
    in_con.write_all(b"world").await.unwrap();
    in_con.shutdown().await.unwrap();
});

// create an outgoing-only node
let tx3_config = tx3::Tx3Config::new();
let (node2, _) = tx3::Tx3Node::new(tx3_config).await.unwrap();

// connect from our outgoing node to our receive node
let mut out_con = node2.connect(&addr1).await.unwrap();

// make sure we can write data
out_con.write_all(b"hello").await.unwrap();
out_con.shutdown().await.unwrap();

// make sure we can read data
let mut got = [0; 5];
out_con.read_exact(&mut got).await.unwrap();
assert_eq!(b"world", &got[..]);

// make sure our receiver task shuts down cleanly
task.await.unwrap();
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
