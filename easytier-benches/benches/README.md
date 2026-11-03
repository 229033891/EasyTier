# Benchmarks

Criterion benchmarks for EasyTier hot paths.

| Bench                     | What it measures                                                                |
| ------------------------- | ------------------------------------------------------------------------------- |
| `packet_bytes_extraction` | `ZCPacket::payload_bytes` / `tunnel_payload_bytes` extraction (advance hot path) |
| `routing_lookup`          | Outbound lookup: VIP hit/miss, default proxy, exit node, and `LocalRouteTable::lookup` (policy-route LPM) |

## Feature fidelity

The crate depends on `easytier-core` with `default-features = false` plus
`test-utils` (the bench helper module `peers::bench_support` is gated behind
that feature). The workspace pins `easytier-core` with `default-features = false`
as well, so a bench build compiles `easytier-core` **without** its `default`
feature set (`aes-gcm`, `endpoint-discovery`, `extended-services`, `management`,
`tcp-hole-punch`).

That does not change the measured code:

- Every function these benches exercise — `PeerMap::get_peer_id_by_ipv4`,
  `PeerMap::get_peer_id_by_ipv4_allowing_default_proxy`,
  `PeerMap::resolve_exit_node_peer` and `LocalRouteTable::lookup` — sits
  **behind no `#[cfg(feature = ...)]` gate**, so the compiled machine code is
  the same as in a default-feature production build.
- The one feature-gated item in `peers/route/local_route_table.rs`
  (`has_domain_rules`) is gated by `proxy-packet`, which is **not** part of
  `default` either — so it is compiled out in the bench build *and* in the
  shipped binary.

In short: the bench measures the same code the released binary runs, and the
CI gate in `script/check_routing_bench.py` compares like with like.

If a benchmarked path ever *does* become feature-gated, either enable the
matching features here (`features = ["default", ...]`, at the cost of a slower
bench build) or re-validate the thresholds in `script/check_routing_bench.py`.

## Packet Bytes Extraction

Criterion benchmark for `ZCPacket` bytes extraction — the methods touched by the
`advance`-based slicing refactor. Measures `payload_bytes` and
`tunnel_payload_bytes` at two payload sizes (1280, 4096). Setup
(`ZCPacket::new_with_payload`) runs in the benchmark harness's preparation
phase and is excluded from the timed region, so the numbers reflect only the
extraction call.

### Quick start

```bash
cargo bench -p easytier-benches --bench packet_bytes_extraction
```

Smoke run:

```bash
PACKET_BYTES_MEASUREMENT_SECS=2 \
PACKET_BYTES_WARMUP_SECS=1 \
PACKET_BYTES_SAMPLE_SIZE=10 \
cargo bench -p easytier-benches --bench packet_bytes_extraction -- --quiet
```

### Environment variables

| Variable                        | Default | Notes                        |
| ------------------------------- | ------- | ---------------------------- |
| `PACKET_BYTES_MEASUREMENT_SECS` | `10`    | Criterion `measurement_time` |
| `PACKET_BYTES_WARMUP_SECS`      | `3`     | Criterion `warm_up_time`     |
| `PACKET_BYTES_SAMPLE_SIZE`      | `10`    | Criterion `sample_size` (min 10) |
