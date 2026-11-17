"""Wave-2 concurrency parity tests.

These tests pin three invariants before/after the bounded-concurrency changes:

1. API task dispatch never runs more workers than ``max_concurrent`` (atomic
   slot reservation), even when many dispatches happen at once.
2. Multi-file Hugging Face downloads preserve input order and still call the
   auth-carrying downloader and SHA-256 verifier exactly once per file.
3. PyPI dependency pre-fetch preserves input order and still runs the
   SHA-256-verifying ``execute()`` path exactly once per wheel.

All fakes are local; no network, real downloads, or real subprocesses occur.
"""

from __future__ import annotations

import sys
import threading
import types
from pathlib import Path
from typing import Any

import pytest

from paradl.core.models import TargetType, TransferResult

# ---------------------------------------------------------------------------
# 1. API dispatch slot safety (deterministic, barrier-based)
# ---------------------------------------------------------------------------


class _FakeStore:
    """Minimal _TaskStore surface used by _TaskRunner._spawn_worker()."""

    def __init__(self) -> None:
        self.threads: dict[str, threading.Thread] = {}
        self.stop_events: dict[str, threading.Event] = {}
        self.updates: list[tuple[str, dict[str, Any]]] = []

    def set_thread(self, task_id: str, thread: threading.Thread) -> None:
        self.threads[task_id] = thread

    def set_stop_event(self, task_id: str, event: threading.Event) -> None:
        self.stop_events[task_id] = event

    def update(self, task_id: str, **kwargs: Any) -> None:
        self.updates.append((task_id, kwargs))


class TestApiDispatchSlotSafety:
    def _make_runner(self, max_concurrent: int = 3):
        from paradl.api.server import _TaskRunner

        store = _FakeStore()
        runner = _TaskRunner(store=store, config=object(), max_concurrent=max_concurrent)  # type: ignore[arg-type]
        return runner, store

    def test_concurrent_dispatch_never_exceeds_max(self) -> None:
        """N simultaneous dispatch attempts must reserve at most max slots."""
        runner, _store = self._make_runner(max_concurrent=3)

        release = threading.Event()
        peak = {"value": 0}
        count_lock = threading.Lock()
        spawned: list[threading.Thread | None] = []
        barrier = threading.Barrier(16)

        # Replace the real _execute with a fake that records the active count
        # while it "runs", then releases its slot in a finally block (matching
        # the real _execute contract).
        def fake_execute(task: Any) -> None:
            with count_lock:
                peak["value"] = max(peak["value"], runner._active_count())
            try:
                release.wait(timeout=5.0)
            finally:
                runner._dec_active()

        runner._execute = fake_execute  # type: ignore[method-assign]

        def dispatch(idx: int) -> None:
            task = types.SimpleNamespace(id=f"task-{idx:02d}")
            barrier.wait()  # all dispatch attempts start together
            spawned.append(runner._spawn_worker(task))

        threads = [threading.Thread(target=dispatch, args=(i,)) for i in range(16)]
        for t in threads:
            t.start()
        for t in threads:
            t.join(timeout=10.0)

        # Exactly max workers were spawned; the rest saw a full pool.
        assert sum(1 for w in spawned if w is not None) == 3
        assert peak["value"] <= 3
        assert runner._active_count() == 3

        # Let the workers finish; every reserved slot is returned.
        release.set()
        for w in spawned:
            if w is not None:
                w.join(timeout=10.0)
        assert runner._active_count() == 0

    def test_released_slot_can_be_reused(self) -> None:
        """After a worker finishes, a new dispatch must be able to reserve."""
        runner, _store = self._make_runner(max_concurrent=1)

        release = threading.Event()

        def fake_execute(task: Any) -> None:
            try:
                release.wait(timeout=5.0)
            finally:
                runner._dec_active()

        runner._execute = fake_execute  # type: ignore[method-assign]

        worker_a = runner._spawn_worker(types.SimpleNamespace(id="task-a"))
        assert worker_a is not None
        assert runner._spawn_worker(types.SimpleNamespace(id="task-b")) is None

        release.set()
        worker_a.join(timeout=5.0)
        assert runner._active_count() == 0

        worker_c = runner._spawn_worker(types.SimpleNamespace(id="task-c"))
        assert worker_c is not None
        worker_c.join(timeout=5.0)

    def test_spawn_failure_releases_reserved_slot(self, monkeypatch: pytest.MonkeyPatch) -> None:
        """If worker startup fails, the reserved slot must be returned.

        A RuntimeError from Thread.start() (e.g. thread exhaustion) means
        _execute never runs, so _spawn_worker itself must release the slot;
        otherwise the active count stays high and _wait_for_slot() can block
        forever.
        """
        from paradl.api import server as server_mod

        runner, _store = self._make_runner(max_concurrent=1)
        runner._execute = lambda task: runner._dec_active()  # type: ignore[method-assign]

        def boom(self: threading.Thread) -> None:
            raise RuntimeError("can't start new thread")

        with monkeypatch.context() as m:
            m.setattr(server_mod.threading.Thread, "start", boom)
            with pytest.raises(RuntimeError, match="can't start new thread"):
                runner._spawn_worker(types.SimpleNamespace(id="task-fail"))
            # The slot reserved before the failure was returned.
            assert runner._active_count() == 0

        # With start() restored, a later dispatch is not blocked by a leak.
        worker = runner._spawn_worker(types.SimpleNamespace(id="task-ok"))
        assert worker is not None
        worker.join(timeout=5.0)
        assert runner._active_count() == 0


# ---------------------------------------------------------------------------
# 2. Hugging Face multi-file order + auth + verification parity
# ---------------------------------------------------------------------------


class TestHfConcurrentParity:
    def test_multifile_download_preserves_order_and_verifies_each(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        from paradl.engines import hf as hf_mod

        files = ["model-a.gguf", "model-b.gguf", "model-c.gguf", "model-d.gguf"]
        infos = [
            {"rfilename": name, "lfs": {"oid": f"{i + 1:064x}"}}
            for i, name in enumerate(files)
        ]
        repo_info = {"siblings": infos}

        monkeypatch.setattr(hf_mod, "_has_hf_xet", lambda: True)
        monkeypatch.setattr(
            hf_mod.HuggingFaceEngine,
            "fetch_hf_info",
            classmethod(lambda cls, repo_id, repo_type="model": repo_info),
        )
        monkeypatch.setattr(
            hf_mod.HuggingFaceEngine, "get_token", classmethod(lambda cls: "TEST_HF_TOKEN")
        )
        monkeypatch.setattr(hf_mod.UI, "can_prompt", lambda select_requested=False: False)

        download_calls: list[tuple[str, str | None]] = []

        def fake_hf_hub_download(**kwargs: Any) -> str:
            filename = kwargs["filename"]
            download_calls.append((filename, kwargs.get("token")))
            dest = tmp_path / filename
            dest.write_bytes(b"x" * 16)
            return str(dest)

        fake_module = types.ModuleType("huggingface_hub")
        fake_module.hf_hub_download = fake_hf_hub_download  # type: ignore[attr-defined]
        monkeypatch.setitem(sys.modules, "huggingface_hub", fake_module)

        verify_calls: list[str] = []

        def fake_verify(path: Path, expected: str) -> tuple[bool, str, str]:
            verify_calls.append(path.name)
            return True, "sha256", expected.split(":", 1)[-1]

        monkeypatch.setattr(hf_mod.ChecksumEngine, "verify", staticmethod(fake_verify))

        result = hf_mod.HuggingFaceEngine.download(
            meta={"repo_id": "org/repo", "revision": "main", "repo_type": "model"},
            out_dir=tmp_path,
            connections=16,
        )

        assert result.success, result.error
        # Output order is preserved even though workers finish out of order.
        assert [p.name for p in result.files] == files
        # Auth + verification happen exactly once per file (order-independent).
        assert sorted(name for name, _ in download_calls) == sorted(files)
        assert [token for _, token in download_calls] == ["TEST_HF_TOKEN"] * len(files)
        assert sorted(verify_calls) == sorted(files)


# ---------------------------------------------------------------------------
# 3. PyPI dependency pre-fetch order + verification parity
# ---------------------------------------------------------------------------


class TestPypiConcurrentParity:
    def test_batch_results_preserve_order_and_verify_each(
        self, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
    ) -> None:
        from paradl.engines import aria2 as aria2_mod
        from paradl.engines import pypi as pypi_mod

        deps = ["dep-a==1.0", "dep-b==2.0", "dep-c", "dep-d==4.0"]

        def fake_probe(target: str, py_version: Any = None) -> Any:
            name = target.split("pip:", 1)[-1].split("==", 1)[0]
            return types.SimpleNamespace(
                target_type=TargetType.PYPI,
                clean_url=f"https://example.invalid/{name}.whl",
                pypi_meta={"package": name, "version": "1.0", "sha256": "ab" * 32},
            )

        # Real PyPIEngine.execute() runs, but its Aria2 download is faked so it
        # produces a real on-disk file. The real execute() then performs the
        # SHA-256 verification branch, which we count below.
        def fake_aria2_download(*args: Any, **kwargs: Any) -> TransferResult:
            out_dir = kwargs.get("out_dir") or (args[1] if len(args) > 1 else tmp_path)
            path = Path(out_dir) / "fake.whl"
            if not path.exists():
                path.write_bytes(b"w" * 8)
            return TransferResult(success=True, file_path=path)

        monkeypatch.setattr(aria2_mod.Aria2Engine, "download", staticmethod(fake_aria2_download))

        verify_calls: list[str] = []

        def counting_verify(path: Path, expected: str) -> tuple[bool, str, str]:
            verify_calls.append(path.name)
            return True, "sha256", expected.split(":", 1)[-1]

        monkeypatch.setattr(pypi_mod.ChecksumEngine, "verify", staticmethod(counting_verify))
        monkeypatch.setattr(
            pypi_mod.PyPIEngine,
            "extract_wheel_dependencies",
            classmethod(lambda cls, path: [f"sub-{path.parent.name}"]),
        )

        engine = pypi_mod.PyPIEngine()
        results = pypi_mod.PyPIEngine._fetch_dependency_batch(
            engine,
            current_batch=deps,
            tmp_path=tmp_path,
            connections=16,
            target_py_ver=None,
            probe=fake_probe,
        )

        assert len(results) == len(deps)
        # Order is preserved even though workers finish in arbitrary order.
        assert [record[0] for record in results] == deps  # type: ignore[index]
        # One SHA-256 verification per wheel (inside execute()).
        assert len(verify_calls) == len(deps)
