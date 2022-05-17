# tx3_pool

Tx3 p2p generic connection pool management.

[![Project](https://img.shields.io/badge/project-holochain-blue.svg?style=flat-square)](http://holochain.org/)
[![Forum](https://img.shields.io/badge/chat-forum%2eholochain%2enet-blue.svg?style=flat-square)](https://forum.holochain.org)
[![Chat](https://img.shields.io/badge/chat-chat%2eholochain%2enet-blue.svg?style=flat-square)](https://chat.holochain.org)

[![License: Apache-2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](https://www.apache.org/licenses/LICENSE-2.0)

This library is a bit of a swiss army blade for p2p connection management.

- Message framing - converts streaming channels into framed send/recv
- Connection pool - pool manages connection open/close
- TODO - connection rate limiting on both outgoing and incoming data
- TODO - message multiplexing to mitigate head-of-line blocking

License: Apache-2.0
