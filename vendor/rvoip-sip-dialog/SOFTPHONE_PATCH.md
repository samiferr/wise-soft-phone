# Local RVoIP 0.4.0 patch

Source: https://crates.io/crates/rvoip-sip-dialog/0.4.0
Upstream: https://github.com/eisenzopf/rvoip
License: MIT; see LICENSE.

The ACK template in `src/transaction/method/ack.rs` retains the initial INVITE's preloaded Route headers and uses them when the 2xx response supplies no Record-Route. Learned Record-Route takes precedence. The private Contact remains the ACK request URI.

The application configures an outbound proxy automatically for Twilio domains. RVoIP already adds that proxy to BYE; this patch supplies the missing equivalent behavior for 2xx ACK. Dialog route sets are otherwise unchanged, avoiding duplicate proxy Route entries in BYE.

Application regressions in `src/sip_routing_tests.rs` simulate a private Contact with Record-Route, without Record-Route plus an explicit proxy, and with a 407 Digest challenge. They assert that ACK/BYE reach the edge, private request URIs are preserved, there is exactly one edge Route, and hang-up completes in under one second on loopback.

Keep this patch until an upstream release passes these tests. SOFTPHONE.patch shows the exact source change relative to crates.io 0.4.0.

A configured preloaded edge is also prefixed when the first learned Record-Route is an RFC1918 IPv4 literal and differs from that edge. The private Record-Route and Contact remain intact behind the public edge. This covers receiver-initiated hang-up after outgoing call answer, where ACK previously followed the private hop. Public learned routes and unproxied accounts keep their existing behavior.

The transaction manager (`src/transaction/manager/mod.rs`) must also resolve the ACK's selected next-hop URI. The upstream literal-only conversion returned None for hostname proxies and then chose the private Contact. The patch reuses the original INVITE transport route when the next hop is unchanged, preserving its DNS-selected edge/flow, or resolves the selected hostname route explicitly when it changes. It never falls back from an unresolved Route hostname to Contact.

Hostname-proxy and hostname Record-Route tests use localhost without external DNS, along with 407/180/200 and receiver BYE. These tests reproduced the remaining live-provider failure before this fix.
