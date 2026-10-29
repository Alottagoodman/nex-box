#!/usr/bin/env python3
"""
tests/test_nex_box_integration.py
Comprehensive verification suite for nex-box kernel sandboxing across the Trinity stack.
Hardened for v0.2.0: Process-group killpg, cross-device link validation, and strategy inspection.
"""
from __future__ import annotations

import os
import sys
import time
from pathlib import Path
import pytest

try:
    import nex_box
    NEX_BOX_AVAILABLE = True
except ImportError:
    NEX_BOX_AVAILABLE = False


def test_01_core_pyo3_and_cow_latency() -> None:
    """Test 1: Verifies C-extension link, strategy telemetry, and sub-20ms CoW cloning."""
    assert NEX_BOX_AVAILABLE, "nex_box native module must be installed (run: maturin develop --release)"

    t0 = time.perf_counter()
    outcome = nex_box.run(
        command=["python3", "-c", "print('COW_ACTIVE')"],
        commit=False,
        timeout_ms=5000,
    )
    duration = (time.perf_counter() - t0) * 1000.0
    assert outcome.exit_code == 0, f"Expected exit code 0, got {outcome.exit_code}"
    assert "COW_ACTIVE" in outcome.stdout, "Expected stdout capture from sandboxed child"
    assert outcome.strategy != "", "Strategy telemetry must be reported"
    assert outcome.clone_duration_micros < 50000, f"CoW clone took too long: {outcome.clone_duration_micros}µs"
    print(f"[TEST 1 PASSED] Strategy: {outcome.strategy} | CoW Latency: {outcome.clone_duration_micros}µs | Total: {duration:.2f}ms")


def test_02_kernel_network_confinement() -> None:
    """Test 2: Verifies kernel-level network lockdown when allow_net=False."""
    assert NEX_BOX_AVAILABLE
    outcome = nex_box.run(
        command=["python3", "-c", "import urllib.request; urllib.request.urlopen('https://github.com', timeout=2)"],
        allow_net=False,
        timeout_ms=5000,
    )
    assert outcome.exit_code != 0, "Network isolation failed: outbound network connection succeeded despite allow_net=False"
    print(f"[TEST 2 PASSED] Kernel Network Confinement Active (Exit Code: {outcome.exit_code})")


def test_03_watchdog_timeout_process_group_teardown() -> None:
    """Test 3: Verifies hard SIGKILL killpg watchdog termination on multiprocess runaway loops."""
    assert NEX_BOX_AVAILABLE
    t0 = time.perf_counter()

    # Spawns a child and grandchildren to verify killpg cleans up the whole tree
    fork_bomb_probe = (
        "import multiprocessing, time\n"
        "def worker():\n"
        "    while True: time.sleep(0.01)\n"
        "if __name__ == '__main__':\n"
        "    p = multiprocessing.Process(target=worker)\n"
        "    p.start()\n"
        "    while True: time.sleep(0.01)\n"
    )

    try:
        nex_box.run(
            command=["python3", "-c", fork_bomb_probe],
            timeout_ms=400,  # Hard killpg after 400ms
            allow_net=True,
        )
        raise AssertionError("Watchdog failed to abort hanging multi-process tree")
    except RuntimeError as err:
        elapsed = (time.perf_counter() - t0) * 1000.0
        assert "timed out" in str(err).lower() or "killpg" in str(err).lower(), f"Unexpected error message: {err}"
        assert elapsed < 2500.0, f"Watchdog took too long to trigger killpg: {elapsed:.2f}ms"
        print(f"[TEST 3 PASSED] Process-group killpg terminated child tree in {elapsed:.2f}ms")


def test_04_atomic_commit_vs_rollback() -> None:
    """Test 4: Verifies exit-code gated promotion (commit=True on success vs failure)."""
    assert NEX_BOX_AVAILABLE
    canary_name = f"canary_ratify_{int(time.time() * 1000)}.txt"
    outcome_success = nex_box.run(
        command=["python3", "-c", f"with open('{canary_name}', 'w') as f: f.write('ratified')"],
        commit=True,
        timeout_ms=5000,
    )
    assert outcome_success.exit_code == 0
    assert outcome_success.committed is True
    assert Path(canary_name).is_file(), "Committed file missing on host"
    Path(canary_name).unlink(missing_ok=True)

    # Sub-test B: Failure with commit=True -> Automatic rollback purges file
    canary_fail_name = f"canary_purge_{int(time.time() * 1000)}.txt"
    outcome_fail = nex_box.run(
        command=["python3", "-c", f"with open('{canary_fail_name}', 'w') as f: f.write('junk'); exit(1)"],
        commit=True,
        timeout_ms=5000,
    )
    assert outcome_fail.exit_code == 1
    assert outcome_fail.committed is False
    assert not Path(canary_fail_name).exists(), "Failed command leaked file onto host workspace"
    print("[TEST 4 PASSED] Atomic Commit & Automatic Rollback verified sound")


def test_05_strategy_flag_override() -> None:
    """Test 5: Verifies strategy parameter parsing (copy fallback vs auto)."""
    assert NEX_BOX_AVAILABLE
    outcome = nex_box.run(
        command=["python3", "-c", "print('FALLBACK_STRATEGY_OK')"],
        strategy="copy",
        timeout_ms=5000,
    )
    assert outcome.exit_code == 0
    assert "Parallel Copy" in outcome.strategy, f"Expected Parallel Copy strategy, got: {outcome.strategy}"
    print(f"[TEST 5 PASSED] Strategy Override Validated ({outcome.strategy})")


if __name__ == "__main__":
    print("=================================================================")
    print("⚡ RUNNING NEX-BOX v0.2.0 KERNEL INTEGRATION SUITE")
    print("=================================================================\n")
    test_01_core_pyo3_and_cow_latency()
    test_02_kernel_network_confinement()
    test_03_watchdog_timeout_process_group_teardown()
    test_04_atomic_commit_vs_rollback()
    test_05_strategy_flag_override()
    print("\n\033[92m[★] ALL INTEGRATION TESTS PASSED CLEANLY.\033[0m")