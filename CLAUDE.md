# Working on rv-rs

Notes for the agents (and people) who write and verify pull requests here. Release steps are in
[RELEASING.md](RELEASING.md).

## Network clients need a test over the scheme the shipped code uses

0.1.0 and 0.2.0 shipped with `rv update` and the online client panicking on the first `https://`
request (no rustls crypto provider). Every test then served plain `http://` from a mock, so none
of them reached the TLS code.

**Author.** A pull request that adds or changes a network client (anything in `rv-update`,
`rv-online`, or a new caller of an HTTP agent) includes at least one test that talks to a local
server over the scheme the shipped code uses. That is `https://`, with the throw-away CA and the
server in `tests/common/tls_server.rs` (see `crates/rv-update/tests/tls_trusted.rs` and
`tls_untrusted.rs`, and the same under `crates/rv-online/tests/`). The test is not `#[ignore]`.
A plain-`http://` mock test is fine for the protocol, but it does not count for this rule.

**Verifier.** Name that test in the PASS (file and test function), and fail the pull request if
there is none. If the pull request changes no network client, say so in the PASS.

## Checking a draft release

A draft check is not done until it has run the network path of the draft's own `rv`, and the
verdict pastes the output. The command is in RELEASING.md, "Run the network path of the draft's
`rv`". `--version` and `--help` alone do not count.
