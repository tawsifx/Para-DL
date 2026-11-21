CREATE TABLE schema_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
INSERT INTO schema_meta VALUES ('schema_version','3');
INSERT INTO schema_meta VALUES ('json_migrated','1');
CREATE TABLE downloads (id INTEGER PRIMARY KEY AUTOINCREMENT, timestamp TEXT NOT NULL,
    target TEXT NOT NULL, type TEXT NOT NULL, filename TEXT NOT NULL, path TEXT NOT NULL,
    size INTEGER NOT NULL DEFAULT 0, duration REAL NOT NULL DEFAULT 0.0,
    success INTEGER NOT NULL DEFAULT 1, adb INTEGER NOT NULL DEFAULT 0, sha256 TEXT);
INSERT INTO downloads (timestamp,target,type,filename,path,size,duration,success,adb,sha256)
    VALUES ('2020-01-01 00:00:00','https://old.example/x','direct','x.bin','/x.bin',7,1.5,1,0,NULL);
CREATE TABLE host_strategies (host TEXT PRIMARY KEY, best_action TEXT NOT NULL DEFAULT 'tier1_native',
    connections INTEGER NOT NULL DEFAULT 16, ip_family TEXT NOT NULL DEFAULT 'auto',
    speed_before REAL NOT NULL DEFAULT 0.0, speed_after REAL NOT NULL DEFAULT 0.0,
    supports_range INTEGER, throttle_class TEXT, updated_at TEXT NOT NULL);
INSERT INTO host_strategies (host,updated_at) VALUES ('old.example','2020-01-01 00:00:00');
CREATE TABLE learned_profiles (host TEXT NOT NULL, hour_bucket INTEGER NOT NULL,
    network_id TEXT NOT NULL, connections INTEGER NOT NULL DEFAULT 16,
    ip_family TEXT NOT NULL DEFAULT 'auto', recovery_action TEXT NOT NULL DEFAULT 'tier2_pause_unpause',
    speed_before REAL NOT NULL DEFAULT 0.0, speed_after REAL NOT NULL DEFAULT 0.0,
    supports_range INTEGER, throttle_class TEXT, updated_at TEXT NOT NULL,
    updated_epoch REAL NOT NULL DEFAULT 0.0, source TEXT NOT NULL DEFAULT 'watchdog',
    PRIMARY KEY (host, hour_bucket, network_id));
INSERT INTO learned_profiles (host,hour_bucket,network_id,updated_at)
    VALUES ('old.example',3,'net','2020-01-01 00:00:00');
CREATE TABLE queue_jobs (id INTEGER PRIMARY KEY AUTOINCREMENT, target TEXT NOT NULL,
    target_type TEXT NOT NULL DEFAULT 'direct', out_dir TEXT NOT NULL, filename TEXT,
    mirrors_json TEXT NOT NULL DEFAULT '[]', priority INTEGER NOT NULL DEFAULT 0,
    connections INTEGER NOT NULL DEFAULT 16, status TEXT NOT NULL DEFAULT 'queued',
    require_ac INTEGER NOT NULL DEFAULT 1, pause_on_low_power INTEGER NOT NULL DEFAULT 0,
    when_fast_bps REAL NOT NULL DEFAULT 0.0, start_at TEXT, expected_length INTEGER,
    expected_digest TEXT, bytes_downloaded INTEGER NOT NULL DEFAULT 0, error TEXT,
    options_json TEXT NOT NULL DEFAULT '{}', created_at TEXT NOT NULL, updated_at TEXT NOT NULL);
INSERT INTO queue_jobs (target,out_dir,created_at,updated_at)
    VALUES ('https://old.example/j','/out','2020-01-01 00:00:00','2020-01-01 00:00:00');
CREATE TABLE http_cache (url TEXT PRIMARY KEY, etag TEXT NOT NULL, body_json TEXT NOT NULL, updated_at REAL NOT NULL);
CREATE TABLE host_caps (host TEXT PRIMARY KEY, cap INTEGER NOT NULL, seen INTEGER NOT NULL DEFAULT 0, updated_epoch REAL NOT NULL DEFAULT 0.0);
CREATE TABLE throttle_samples (id INTEGER PRIMARY KEY AUTOINCREMENT, timestamp TEXT NOT NULL, epoch REAL NOT NULL,
    hour INTEGER NOT NULL, is_weekend INTEGER NOT NULL DEFAULT 0, interface TEXT NOT NULL DEFAULT 'en0',
    ssid TEXT NOT NULL DEFAULT '', network_id TEXT NOT NULL DEFAULT 'default', source TEXT NOT NULL DEFAULT 'active',
    target_group TEXT NOT NULL DEFAULT 'international', host TEXT NOT NULL DEFAULT '', download_bps REAL NOT NULL DEFAULT 0.0,
    upload_bps REAL NOT NULL DEFAULT 0.0, rtt_ms REAL NOT NULL DEFAULT 0.0, bytes_used INTEGER NOT NULL DEFAULT 0,
    series_json TEXT NOT NULL DEFAULT '[]');
CREATE TABLE throttle_runs (id INTEGER PRIMARY KEY AUTOINCREMENT, run_id TEXT NOT NULL UNIQUE, mode TEXT NOT NULL,
    isp TEXT NOT NULL, created_at TEXT NOT NULL, created_epoch REAL NOT NULL, budget_bytes INTEGER NOT NULL DEFAULT 0,
    bytes_used INTEGER NOT NULL DEFAULT 0, report_json TEXT NOT NULL);
CREATE TABLE asn_cache (ip TEXT PRIMARY KEY, asn TEXT NOT NULL, country TEXT NOT NULL, org TEXT NOT NULL, updated_epoch REAL NOT NULL);
