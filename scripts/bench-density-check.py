#!/usr/bin/env python3
"""Check a density bench result against the published reference.

    bench-density-check.py <reference.json> <density.json> [component.sha256]

Memory is checked: at every level the reference has, process PSS and marginal PSS per agent
must fall inside the range the published runs span, widened by the reference's tolerance.
CPU and timings depend on the machine, so they are printed for comparison, not checked.
Exit status: 0 when every check passes, 1 otherwise.
"""
import json
import sys


def main(argv):
    if len(argv) < 3:
        print(__doc__.strip())
        return 2
    ref = json.load(open(argv[1]))
    got = json.load(open(argv[2]))
    sha = open(argv[3]).read().split()[0] if len(argv) > 3 else None
    tol = ref["tolerance"]
    runs = ref["runs"]
    failures = 0

    def check(name, value, published, tolerance):
        nonlocal failures
        lo, hi = min(published) * (1 - tolerance), max(published) * (1 + tolerance)
        ok = lo <= value <= hi
        failures += not ok
        span = f"{min(published):.2f}" if min(published) == max(published) else f"{min(published):.2f}-{max(published):.2f}"
        print(f"  {'PASS' if ok else 'FAIL'}  {name:<34} {value:9.2f}   published {span}, accepted {lo:.2f}-{hi:.2f}")

    print(f"reference: {len(runs)} published runs, tolerance ±{tol['pss_mb']:.0%} on PSS, "
          f"±{tol['marginal_pss_mb_per_agent']:.0%} on marginal PSS per agent")
    print()
    if got.get("stopped_early"):
        print(f"  FAIL  the bench stopped early: {got['stopped_early']}")
        failures += 1
    else:
        print("  PASS  every level ran; every agent kept its conversation between turns")
    # A load measured in seconds means the component was compiled inside the measured process,
    # whose PSS then includes the compiler's memory. The warm-up run exists to prevent that.
    if got["load_ms"] > 5000:
        print(f"  FAIL  the component was compiled in the measured process ({got['load_ms'] / 1000:.1f} s load), "
              "so process PSS includes the compiler: rerun scripts/bench-density.sh")
        failures += 1
    else:
        print(f"  PASS  the component loaded from the compiled cache ({got['load_ms']:.0f} ms)")

    mine = {row["agents"]: row for row in got["levels"]}
    levels = sorted({row["agents"] for run in runs for row in run["result"]["levels"]})
    for n in levels:
        pub = [row for run in runs for row in run["result"]["levels"] if row["agents"] == n]
        if n not in mine:
            print(f"  FAIL  level {n} agents is missing: include it in DENSITY_LEVELS")
            failures += 1
            continue
        row = mine[n]
        check(f"{n} agents: process PSS MB", row["pss_mb"], [p["pss_mb"] for p in pub], tol["pss_mb"])
        marg = [p["marginal_pss_mb_per_agent"] for p in pub if p["marginal_pss_mb_per_agent"] is not None]
        if marg and row["marginal_pss_mb_per_agent"] is not None:
            check(f"{n} agents: marginal PSS MB/agent", row["marginal_pss_mb_per_agent"], marg,
                  tol["marginal_pss_mb_per_agent"])

    print()
    print("for comparison only (machine-dependent, not checked):")
    print(f"  {'':<34} {'yours':>9}   " + "   ".join(f"{r['label']:>12}" for r in runs))
    print(f"  {'sequential CPU-ms per turn':<34} {got['sequential_cpu_ms_per_turn']:9.1f}   "
          + "   ".join(f"{r['result']['sequential_cpu_ms_per_turn']:12.1f}" for r in runs))
    for n in levels:
        if n in mine:
            pub = [next((x for x in r["result"]["levels"] if x["agents"] == n), None) for r in runs]
            print(f"  {f'{n} agents: CPU-ms per turn':<34} {mine[n]['second_turn_cpu_ms_per_agent']:9.1f}   "
                  + "   ".join(f"{p['second_turn_cpu_ms_per_agent']:12.1f}" if p else f"{'-':>12}" for p in pub))
    if sha:
        same = [r["label"] for r in runs if r.get("component_sha256") == sha]
        print(f"  {'component sha256':<34} {sha[:12]}…  "
              + (f"same as {', '.join(same)}" if same else
                 "differs from the published ones (expected: the guest build is not byte-for-byte reproducible)"))

    print()
    print("RESULT: PASS" if failures == 0 else f"RESULT: FAIL ({failures} check(s) outside the published range)")
    return 0 if failures == 0 else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv))
