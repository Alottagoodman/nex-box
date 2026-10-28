#!/usr/bin/env python3
"""
examples/smoke_test.py
Verification harness demonstrating native Python bindings for nex-box.
"""
import sys

try:
    import nex_box
except ImportError:
    sys.exit(
        "\033[91m[ERROR] nex_box module not found in active Python environment.\n"
        "Run: maturin develop --release\033[0m"
    )

print("\033[96m=================================================================\033[0m")
print(f"\033[1;97m⚡ NEX-BOX PYTHON C-EXTENSION VERIFICATION (v{nex_box.__version__})\033[0m")
print("\033[96m=================================================================\033[0m\n")

# -----------------------------------------------------------------------------
# RUN 1: Successful command with atomic promotion (--commit)
# -----------------------------------------------------------------------------
print("\033[93m[*] Executing Run 1: Verified file generation with atomic commit...\033[0m")
outcome = nex_box.run(
    command=["python3", "-c", "with open('agent_output.py', 'w') as f: f.write('RATIFIED = True\\n')"],
    commit=True,
    timeout_ms=5000
)

print(f"  Exit Code:               {outcome.exit_code}")
print(f"  Execution Duration:      {outcome.duration_ms}ms")
print(f"  Kernel CoW Clone Time:   {outcome.clone_duration_micros}µs ({outcome.clone_duration_micros / 1000.0:.2f}ms)")
print(f"  Files Cloned in RAM:     {outcome.files_cloned}")
print(f"  Atomic Commit Applied:   {outcome.committed}")
print(f"  Files Created:           {outcome.diff.files_created}")
print(f"  Files Modified:          {outcome.diff.files_modified}\n")

assert outcome.exit_code == 0, "Run 1 expected exit code 0"
assert "agent_output.py" in outcome.diff.files_created, "agent_output.py should be in files_created"

# -----------------------------------------------------------------------------
# RUN 2: Failing command with automatic rollback (zero host pollution)
# -----------------------------------------------------------------------------
print("\033[93m[*] Executing Run 2: Failing command to verify automatic rollback...\033[0m")
fail_outcome = nex_box.run(
    command=["python3", "-c", "with open('bad_file.txt', 'w') as f: f.write('corrupted'); exit(1)"],
    commit=True,
    timeout_ms=5000
)

print(f"  Exit Code:               {fail_outcome.exit_code}")
print(f"  Atomic Commit Applied:   {fail_outcome.committed} (False = Rollback Succeeded)")
print(f"  Files Created in Box:    {fail_outcome.diff.files_created}\n")

assert fail_outcome.exit_code == 1, "Run 2 expected exit code 1"
assert fail_outcome.committed is False, "Failing command must not commit to host"

print("\033[92m[★] ALL PYO3 RUNTIME ASSERTIONS PASSED CLEANLY.\033[0m\n")