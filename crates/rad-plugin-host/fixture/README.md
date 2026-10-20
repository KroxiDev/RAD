# rad-plugin-fixture — standalone protocol fixture

No dependency on RAD crates: uses the public v1 JSON/u32-BE protocol directly.
Two native binaries: stable plugin ID `org.rad.fixture`, versions 1.0.0 and 2.0.0.
Same stable ID intentionally supports update/rollback; binary versions differ.

`run` action accepts required `mode`: echo / fail / slow / crash / late / flood.
Echo reports success without effects; fail reports unavailable; slow sleeps 250ms;
crash exits23; late sends a wrong prior request before its proper reply; flood sends
1000 duplicate replies. `status` source returns v1/v2 text for TTL testing.
When subscribed, slow emits `running` before sleeping, proving actual action entry
in timeout/reap tests rather than merely timing out an unsent request.
No apps, sensors, hardware, network, personal data or filesystem writes.
stdout protocol only; all stdin frames capped at64KiB. This is intentionally a
fault-injection fixture, not a production integration or security sandbox.

Standalone build after coordinator review (existing cache, no downloads):
`cargo build --offline --release --manifest-path Cargo.toml --bins`.
In RAD, bins/tests are gated behind rad-plugin-host/fixture-tests. Shared source
paths avoid copying RAD implementation into the public fixture.
