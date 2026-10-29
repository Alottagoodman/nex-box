#!/usr/bin/env python3
"""
tests/test_nex_hop_standalone.py
Standalone test harness verifying nex_hop.py internal engines, RAM VFS, database lattices,
and sandboxed host execution routing.
"""
from __future__ import annotations

import asyncio
import json
import sys
from pathlib import Path
import pytest

# Insert parent directories to allow importing nex_hop if located nearby
CURRENT_DIR = Path(__file__).resolve().parent
PARENT_DIR = CURRENT_DIR.parent
if str(PARENT_DIR) not in sys.path:
    sys.path.insert(0, str(PARENT_DIR))

try:
    import nex_hop
    NEX_HOP_AVAILABLE = True
except ImportError:
    NEX_HOP_AVAILABLE = False


def test_01_ram_vfs_engine() -> None:
    """Test 1: Verifies nex_hop RamWorkspaceEngine VFS state transitions."""
    assert NEX_HOP_AVAILABLE, "nex_hop.py must be in PYTHONPATH or parent directory"
    
    workspace = nex_hop.RamWorkspaceEngine()

    # 1. Write file
    res_write = workspace.write_file("subnet.py", "def check_overlap(): return True")
    assert res_write["status"] == "SUCCESS"
    assert res_write["path"] == "subnet.py"

    # 2. Read file
    res_read = workspace.read_file("subnet.py")
    assert res_read["status"] == "SUCCESS"
    assert "check_overlap" in res_read["content"]

    # 3. Checkpoint & Rollback
    workspace.checkpoint("v1_snapshot")
    workspace.write_file("subnet.py", "CORRUPTED")
    assert "CORRUPTED" in workspace.read_file("subnet.py")["content"]

    workspace.rollback("v1_snapshot")
    assert "check_overlap" in workspace.read_file("subnet.py")["content"]
    print("[TEST 1 PASSED] nex_hop RamWorkspaceEngine VFS validated")


def test_02_local_databases_lattice() -> None:
    """Test 2: Verifies embedded SQLite WAL and DuckDB analytics initialization."""
    assert NEX_HOP_AVAILABLE

    nex_hop.init_local_databases()

    # Query SQLite ledger
    sqlite_res = json.loads(nex_hop.execute_sqlite_query("SELECT COUNT(*) as cnt FROM audit_ledger"))
    assert sqlite_res[0]["cnt"] >= 1

    # Query DuckDB cluster metrics
    duck_res = json.loads(nex_hop.execute_duckdb_query("SELECT COUNT(*) as cnt FROM cluster_metrics"))
    assert duck_res[0]["cnt"] >= 1
    print("[TEST 2 PASSED] Embedded SQLite WAL & DuckDB analytics operational")


def test_03_ram_test_runner_prover() -> None:
    """Test 3: Verifies in-memory module injection and test execution in RAM VFS."""
    assert NEX_HOP_AVAILABLE

    workspace = nex_hop.RamWorkspaceEngine()
    workspace.write_file("math_ops.py", "def add(a, b): return a + b\n__all__ = ['add']")

    test_payload = (
        "from math_ops import add\n"
        "assert add(2, 3) == 5\n"
        "assert add(-1, 1) == 0\n"
        "print('MATH_VERIFIED')\n"
    )

    result = workspace.execute_test(test_payload)
    assert result["status"] == "PASSED", f"RAM test runner failed: {result.get('error')}"
    assert "MATH_VERIFIED" in result["stdout"]
    print("[TEST 3 PASSED] RAM Test Runner Prover executed successfully in memory")


@pytest.mark.asyncio
async def test_04_host_exec_cow_sandbox_dispatch() -> None:
    """Test 4: Verifies dispatch_local_tool_call routing host_exec through nex-box."""
    assert NEX_HOP_AVAILABLE

    workspace = nex_hop.RamWorkspaceEngine()
    
    arguments = json.dumps({
        "command": "python3 -c \"print('HOST_EXEC_SANDBOX_OK')\"",
        "cwd": str(PARENT_DIR),
        "timeout_seconds": 10
    })

    raw_response = await nex_hop.dispatch_local_tool_call(
        name="host_exec",
        arguments_str=arguments,
        workspace=workspace,
        active_tools=[]
    )

    payload = json.loads(raw_response)
    assert payload["exit_code"] == 0, f"Host exec failed: {payload}"
    assert "HOST_EXEC_SANDBOX_OK" in payload["stdout"]
    print(f"[TEST 4 PASSED] host_exec routed through nex-box kernel sandbox in {payload.get('duration_ms')}ms")


if __name__ == "__main__":
    import pytest
    sys.exit(pytest.main([__file__, "-v"]))