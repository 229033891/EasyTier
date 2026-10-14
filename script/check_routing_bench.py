#!/usr/bin/env python3
"""CI gate for the outbound-routing hot path (unified-routing P6 / §12.5).

Reads criterion's `estimates.json` after

    cargo bench -p easytier --bench routing_lookup -- --noplot

and enforces two **machine-independent** bounds:

  1. empty-table `lookup` median <= 1 us
     The zero-regression baseline: with no `[[policy_routes]]` the outbound path
     only pays one extra `ArcSwap` load. The bound is deliberately loose so a
     shared CI runner cannot flake it.
  2. 10K-rule median <= 1K-rule median * SCALE_DEGRADATION_LIMIT
     Catches a **linear-scan regression** (~10×), not a quiet-machine SLA.
     The fixture puts every rule in the same /24 bucket, so miss/hit cost is
     roughly `overhead + c·log2(N)`; 1K→10K alone is ~1.25–1.35× before CI
     noise. The product stretch in §12.5 (<20% on a quiet host) is tighter
     than this CI bound on purpose.

Criterion stores groups under `target/criterion/<group>/<function>/new/`.
Group names must not contain `/` (Criterion replaces `/` with `_` in paths);
the benches use `policy_route_empty` / `policy_route_{scale}`.

Absolute latency targets (P99 < 1 us) are tracked by the benchmark itself,
not gated here.

Exit code 0 = pass, 1 = at least one bound violated.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

CRITERION_DIR = Path("target/criterion")

# Median lookup latency for the empty table, in nanoseconds (1 us).
EMPTY_MEDIAN_LIMIT_NS = 1_000.0

# Allowed 10K-vs-1K degradation ratio on shared CI runners.
# Keep well below linear-scan (~10×); leave headroom above the ~1.25–1.35×
# expected from same-prefix-length binary search (see module docstring).
SCALE_DEGRADATION_LIMIT = 1.50

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


def list_criterion_groups() -> None:
    """Print what Criterion actually wrote (helps path-mismatch debugging)."""
    if not CRITERION_DIR.is_dir():
        print(f"  (no criterion dir at {CRITERION_DIR})")
        return
    groups = sorted(
        p.name for p in CRITERION_DIR.iterdir() if p.is_dir() and p.name != "report"
    )
    print(f"  criterion groups present: {', '.join(groups) or '(none)'}")


def main() -> int:
    print("routing hot-path gate (unified-routing §12.5)")

    empty = median_ns("policy_route_empty", "miss")
    if empty is not None:
        check_le("policy_route_empty/miss", empty, EMPTY_MEDIAN_LIMIT_NS)

    for case in ("hit", "miss"):
        small = median_ns(f"policy_route_{SMALL_SCALE}", case)
        large = median_ns(f"policy_route_{LARGE_SCALE}", case)
        # Use `is not None` — a 0.0 median is falsy but still a valid estimate.
        if small is not None and large is not None:
            check_le(
                f"policy_route_{case} {LARGE_SCALE}/{SMALL_SCALE} ratio",
                large / small,
                SCALE_DEGRADATION_LIMIT,
                unit="x",
            )

    if failures:
        print("\nrouting hot-path gate FAILED:")
        for failure in failures:
            print(f"  - {failure}")
        list_criterion_groups()
        return 1

    print("\nrouting hot-path gate passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
