# nex-box ⚡

**Sub-millisecond, zero-copy, kernel-level execution sandbox for autonomous coding agents.**

[![PyPI version](https://img.shields.io/pypi/v/nex-box.svg)](https://pypi.org/project/nex-box/)
[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey.svg)](https://pypi.org/project/nex-box/)
[![Rust](https://img.shields.io/badge/rust-2021%20edition-orange.svg)](https://www.rust-lang.org/)

`nex-box` solves the fundamental dilemma of autonomous AI engineering: **how to let coding agents run untrusted tests, builds, and destructive file mutations without risking host corruption or waiting seconds for Docker.**

Backed by macOS APFS `clonefile(2)` and Linux `ioctl(FICLONE)` copy-on-write primitives, `nex-box` instantiates ephemeral scratch workspaces in **under 3 milliseconds**, traps all file mutations and network attempts at the kernel layer, and guarantees **instant atomic rollback on failure** or **zero-copy promotion on success**.

---

## The Problem: The Agent Sandboxing Dilemma

When an AI agent (Cursor, Claude Code, Aider, custom autonomous loops) writes and tests software, developers are forced into bad compromises:

```
+-------------------+--------------------+---------------------------------------------------+
| APPROACH          | LATENCY            | FAILURE MODES & OPERATIONAL FRICTION              |
+-------------------+--------------------+---------------------------------------------------+
| Host Subprocess   | ~25ms              | Destructive file overwrites; leaks .env secrets;  |
|                   |                    | dirty cache pollution (.pytest_cache, .pyc).      |
+-------------------+--------------------+---------------------------------------------------+
| Docker Containers | 1,500ms – 3,500ms  | Heavy daemon requirement; multi-second latency    |
|                   |                    | kills agent flow state; complex volume mounts.    |
+-------------------+--------------------+---------------------------------------------------+
| Python venv / cp  | 150ms – 800ms      | Slow file copies; wears SSD endurance; no network |
|                   |                    | containment; fails to sandbox non-Python edits.   |
+-------------------+--------------------+---------------------------------------------------+
| nex-box (CoW)     | 2.1ms (2,168µs)    | Sub-millisecond APFS/FICLONE inode branching;     |
|                   |                    | atomic commit or instant purge; zero disk bloat.  |
+-------------------+--------------------+---------------------------------------------------+
```

---

## Architectural Topology

```
                  HOST WORKSPACE (Pristine Inode Root)
                                   │
              ┌────────────────────┴────────────────────┐
              │ clonefile(2) / ioctl(FICLONE)            │ < 2.5ms Latency
              ▼                                         ▼
   EPHEMERAL SCRATCH ROOT                    KERNEL CONFINEMENT ENCLAVE
   (.nexbox/sessions/box_xxx)                 ├── macOS Seatbelt: (deny network*)
              │                               └── Linux: unshare(CLONE_NEWNET)
              ├─────────────────────────────────────────┤
              │ COMMAND EXECUTION & WATCHDOG TIMER      │
              │ ├── Scratch Log I/O (Anti-Pipe Lock)    │
              │ └── Hard millisecond SIGKILL watchdog   │
              ├─────────────────────────────────────────┤
              │ BLAKE3 CRYPTOGRAPHIC DIFFERENTIAL       │
              │ ├── Hashes only mutated file inodes     │
              │ └── Extracts: Created, Modified, Deleted│
              └────────────────────┬────────────────────┘
                                   │
                    ┌──────────────┴──────────────┐
                    ▼                             ▼
           EXIT CODE 0 & --commit        EXIT CODE != 0 / TIMEOUT
                    │                             │
                    ▼                             ▼
        [ATOMIC PROMOTION]               [AUTOMATIC ROLLBACK]
   Reflink changes to host tree.    Instant purge of scratch tree.
   Host updated in microseconds.    Host remains 100% untouched.
```

---

## Installation

### Via pip (Pre-compiled wheels for all major platforms)
```bash
pip install nex-box
# or with uv (recommended):
uv pip install nex-box
```

### Via Cargo (Standalone binary)
```bash
cargo install nex-box
```

---

## Quickstart

### 1. Python API (`import nex_box`)

`nex-box` ships a native PyO3 C-extension that hooks directly into any Python agentic framework:

```python
import nex_box

# Run untrusted agent code in a 2ms CoW sandbox:
outcome = nex_box.run(
    command=["pytest", "tests/"],
    commit=True,          # Automatically commit to host ONLY if exit code == 0
    timeout_ms=5000,      # Hard watchdog kill after 5.0 seconds
    allow_net=False       # Kernel-level network socket isolation
)

if outcome.is_success():
    print(f"Verified in {outcome.duration_ms}ms!")
    print(f"Cloned in: {outcome.clone_duration_micros}µs")
    print(f"Files Created: {outcome.diff.files_created}")
    print(f"Files Modified: {outcome.diff.files_modified}")
else:
    print(f"Agent failed with exit code {outcome.exit_code}!")
    print("Host was untouched. Error trace:")
    print(outcome.stderr)
```

### 2. Standalone CLI (`nex-box`)

```bash
# 1. Ephemeral Dry Run (Default: Discards all mutations on exit)
nex-box run -- bash -c "echo 'mutation' > test.txt && pytest"

# 2. Atomic Commit (Promotes changes to host only if command succeeds)
nex-box run --commit -- python3 -m py_compile module.py

# 3. Watchdog Timeout Protection (Kills runaway infinite loops)
nex-box run --timeout-ms 2000 -- python3 -c "while True: pass"

# 4. Kernel Network Isolation (Denies all outbound TCP/UDP sockets)
nex-box run --allow-net false -- curl -I https://github.com

# 5. Machine-Readable JSON Telemetry (For agent stdout integration)
nex-box run --json -- echo "Agent complete"
```

---

## Concrete Recipes for Coding Agents

### Recipe 1: Safe Pytest Runner for Coding Agents
When agents run `pytest`, failed runs dirty your repository with `.pytest_cache/`, `__pycache__`, and half-written test artifacts. `nex-box` completely isolates them:

```python
import nex_box

outcome = nex_box.run(
    command=["pytest", "tests/unit/"],
    commit=False,         # Throw away all pytest cache files and test debris
    timeout_ms=10000
)

# Output is captured cleanly; your git tree remains 100% pristine:
print(outcome.stdout)
```

### Recipe 2: Air-Gapped Code Evaluation
Block outbound exfiltration or network dependencies at the operating system layer:

```python
import nex_box

# On macOS, uses Apple Seatbelt ((deny network*)).
# On Linux, detaches the network namespace (unshare(CLONE_NEWNET)).
outcome = nex_box.run(
    command=["python3", "untrusted_script.py"],
    allow_net=False
)
```

### Recipe 3: Atomic Multi-File Refactoring
Let an LLM refactor multiple files across your project. Promote the edits **only** if the build and lint passes:

```python
import nex_box

outcome = nex_box.run(
    command=["bash", "-c", "ruff check . && cargo test"],
    commit=True,          # Atomic promotion: all or nothing
    timeout_ms=30000
)

if outcome.is_success():
    print("Refactor applied cleanly across files:", outcome.diff.files_modified)
```

---

## Python API Reference

### `nex_box.run(...)`

```python
def run(
    command: list[str],
    cwd: str | None = None,
    commit: bool = False,
    timeout_ms: int = 60000,
    allow_net: bool = True,
    preserve: bool = False
) -> SandboxOutcome: ...
```

#### Parameters:
- `command` *(list[str])*: The command line arguments to execute inside the scratch root (e.g. `["pytest", "tests/"]`).
- `cwd` *(str | None)*: Base workspace directory to duplicate via CoW. Defaults to current working directory.
- `commit` *(bool)*: If `True`, automatically promotes all created/modified files back to `cwd` if and only if `exit_code == 0`. Default: `False`.
- `timeout_ms` *(int)*: Maximum wall-clock execution time in milliseconds before triggering hard `SIGKILL`. Default: `60000` (60s).
- `allow_net` *(bool)*: If `False`, denies all network sockets at the kernel level. Default: `True`.
- `preserve` *(bool)*: If `True`, preserves the `.nexbox/sessions/` directory for manual inspection. Default: `False`.

#### Returns `SandboxOutcome`:
- `.exit_code` *(int)*: Process termination status code (e.g. `0`, `1`).
- `.duration_ms` *(int)*: Total process execution duration in milliseconds.
- `.clone_duration_micros` *(int)*: Time taken by the kernel to duplicate the entire directory tree (microseconds).
- `.files_cloned` *(int)*: Total file count mirrored into scratch space.
- `.committed` *(bool)*: Whether mutations were promoted back to the host filesystem.
- `.stdout` *(str)*: Standard output buffer.
- `.stderr` *(str)*: Standard error buffer.
- `.diff` *(WorkspaceDiff)*:
  - `.diff.files_created` *(list[str])*: Relative paths of files created in scratch.
  - `.diff.files_modified` *(list[str])*: Relative paths of existing files mutated.
  - `.diff.files_deleted` *(list[str])*: Relative paths of files removed.
  - `.diff.elapsed_micros` *(int)*: Time taken by the in-memory BLAKE3 SIMD hash diff.

---

## Technical Specifications

1. **Kernel CoW Engine (`vfs_cow.rs`):**
   - **macOS:** Binds directly to `libc::clonefile(2)` on APFS. Creates copy-on-write pointers in the filesystem extent tree. Duplicating a 10,000-file repository takes $\le 4\text{ms}$ and consumes zero bytes of disk storage until mutated.
   - **Linux:** Invokes `ioctl(dst_fd, FICLONE, src_fd)` across Btrfs, XFS (reflink=1), and OpenZFS. Falls back to hardlinks or zero-allocation byte buffers.
2. **BLAKE3 SIMD Differential Engine (`diff.rs`):**
   - Uses multi-threaded BLAKE3 tree hashing to compare file states between the host and scratch workspace. Detects exact byte mutations while automatically filtering OS noise (`.DS_Store`, `.git`).
3. **Pipe-Deadlock Immune Watchdog (`sandbox.rs`):**
   - Child process `stdout` and `stderr` streams are redirected to scratch log files within the isolated inode space. This permanently neutralizes the 64KB POSIX pipe deadlock vulnerability where parent processes sleep while child processes block on filled pipe buffers.

---

## License

Dual-licensed under either of:
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))
```


