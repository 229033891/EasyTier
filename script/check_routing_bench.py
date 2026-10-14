#!/usr/bin/env python3
"""CI gate for the outbound-routing hot path (unified-routing P6 / §12.5).

Reads criterion's `estimates.json` after

    cargo bench -p easytier --bench routing_lookup -- --noplot

and enforces two **machine-independent** bounds:

  1. empty-table `lookup` median <= 1 us
     The zero-regression baseline: with no `[[policy_routes]]` the outbound path
     only pays one extra `ArcSwap` load. The bound is deliberately loose so a
     shared CI runner cannot flake it.
  2. 10K-rule median <= 1K-rule median * 1.20
     The §12.5 scale gate. The LPM index is bucketed by prefix length and binary
     searched, so growing the rule count must not degrade a lookup by more than
     20%. A full linear scan would blow this by ~10x, which is exactly the
     regression this gate exists to catch.

Both are ratios or very loose absolutes, so they hold across runners. Absolute
latency targets (P99 < 1 us) are tracked by the benchmark itself, not gated here.

Exit code 0 = pass, 1 = at least one bound violated.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

CRITERION_DIR = Path("target/criterion")

# Median lookup latency for the empty table, in nanoseconds (1 us).
EMPTY_MEDIAN_LIMIT_NS = 1_000.0

# Allowed 10K-vs-1K degradation ratio (§12.5).
SCALE_DEGRADATION_LIMIT = 1.20

# Scale points defined by `easytier/benches/routing_lookup.rs`.
SMALL_SCALE = "1000"
LARGE_SCALE = "10000"

failures: list[str] = []


def median_ns(group: str, bench: str) -> float | None:
    """Median point estimate (ns) for one criterion benchmark, or None."""
    path = CRITERION_DIR / group / bench / "new" / "estimates.json"
    if not path.exists():
        failures.append(f"missing criterion output: {path}")
        return None
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
        return float(data["median"]["point_estimate"])
    except (KeyError, ValueError, OSError) as error:
        failures.append(f"unreadable criterion output {path}: {error}")
        return None


def check_le(label: str, value: float, limit: float, unit: str = "ns") -> None:
    ok = value <= limit
    print(
        f"  [{'ok' if ok else 'FAIL'}] {label}: "
        f"{value:.2f}{unit} (limit {limit:.2f}{unit})"
    )
    if not ok:
        failures.append(f"{label}: {value:.2f}{unit} > {limit:.2f}{unit}")


def main() -> int:
    print("routing hot-path gate (unified-routing §12.5)")

    empty = median_ns("policy_route/empty", "miss")
    if empty is not None:
        check_le("policy_route/empty/miss", empty, EMPTY_MEDIAN_LIMIT_NS)

    for case in ("hit", "miss"):
        small = median_ns(f"policy_route/{SMALL_SCALE}", case)
        large = median_ns(f"policy_route/{LARGE_SCALE}", case)
        if small and large:
            check_le(
                f"policy_route/{case} {LARGE_SCALE}/{SMALL_SCALE} ratio",
                large / small,
                SCALE_DEGRADATION_LIMIT,
                unit="x",
            )

    if failures:
        print("\nrouting hot-path gate FAILED:")
        for failure in failures:
            print(f"  - {failure}")
        return 1

    print("\nrouting hot-path gate passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
