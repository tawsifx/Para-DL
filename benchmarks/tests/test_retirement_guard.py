def _residue(tmp_path):
    oracle = tmp_path / "src" / "paradl"
    (oracle / "__pycache__").mkdir(parents=True)
    (oracle / "__pycache__" / "stale.pyc").write_bytes(b"")
    return oracle


def test_baseline_retires_when_only_pycache_remains(tmp_path, monkeypatch):
    import bench_baseline as bb
    monkeypatch.setattr(bb, "ORACLE_DIR", _residue(tmp_path))
    monkeypatch.setattr(bb, "RESULTS_DIR", tmp_path / "results")
    out = bb.run("guard-test", None)
    assert out["status"] == "retired"
    assert out["results"] and all(r["value"] is None for r in out["results"])


def test_rust_engine_retires_when_only_pycache_remains(tmp_path, monkeypatch):
    import bench_rust_engine as br
    monkeypatch.setattr(br, "ORACLE_DIR", _residue(tmp_path))
    out = br.run_benchmark_suite(tmp_path / "bench")
    assert out["retired"] is True
    assert out["benchmarks"]["checksum_large_file"]["value"] is None
