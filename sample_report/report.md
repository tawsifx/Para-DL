# ISP Throttling Diagnostic Report — Duronto Internet

- **ISP**: Duronto Internet
- **Run ID**: `full-20261108-duronto-sample`
- **Mode**: `full`
- **Timestamp**: `2026-11-08T07:15:00+06:00`
- **Subscribed Plan**: 60.0 Mbps
- **Data Budget**: 145.0 MB used of 3072.0 MB (planned worst-case: 2600.0 MB)
- **Primary Evidence Strategy**: `paired_same_moment`

## 1. Local vs International Summary

| Locality Tier | 1-Connection Median | 16-Connection Aggregate |
| --- | ---: | ---: |
| **Domestic (In-Country / IXP)** | 658.51 Mbps | 671.09 Mbps |
| **ISP-Cached CDN Edge** | 545.26 Mbps | 629.15 Mbps |
| **Uncached International** | 40.27 Mbps | 489.89 Mbps |

> **Plain-Language Reading**: Domestic (658.5 Mbps) and ISP-cached (545.3 Mbps) routes are substantially faster than uncached international routes (40.3 Mbps). This indicates international bandwidth shaping or upstream transit bottlenecks while local BDIX/GGC peering runs near line rate.

## 2. Diagnostic Verdicts

| Verdict Class | Confidence | Summary | Supporting Evidence |
| --- | ---: | --- | --- |
| **per-flow cap** | 95% | Each TCP connection to international CDNs is capped (~4.8 Mbps / 600 KB/s), while 16 parallel connections scale aggregate throughput 12.2x to 58.4 Mbps. | 1 connection to speed.cloudflare.com: 4.80 Mbps; 16 connections to speed.cloudflare.com: 58.40 Mbps (12.17x scaling); Per-flow rate at 16 streams: 3.65 Mbps (clean flow separation) |
| **burst-then-throttle** | 92% | International flows burst at 28.5 Mbps for ~1.0s (~2.9 MB token bucket) before collapsing to a sustained 4.8 Mbps ceiling. | Initial burst on speed.cloudflare.com: 28.50 Mbps for 1.00s; Sustained rate after step drop: 4.80 Mbps (5.9x collapse); Token bucket size: ~2.96 MB (refills to 85% within 30s idle gap) |
| **international bandwidth limit** | 96% | Domestic / BDIX peering throughput (78.5 Mbps) is 16.3x faster than single-connection international throughput (4.8 Mbps). | Domestic/BDIX median: 78.50 Mbps (low latency 4.8 ms); Uncached international median: 4.80 Mbps (latency 19.4 ms); Paired ratio: 16.35x domestic advantage |
| **loss-based policing** | 86% | Hard packet-drop rate policing detected on international streams above token bucket (retransmit rate 2.11%, minimal RTT growth +2.7 ms). | TCP retransmit rate delta: 2.10% (320 retransmits across 15.2k packets); RTT under load (21.2 ms) vs idle (18.5 ms): minimal buffer queueing (+2.7 ms); Throughput CoV: 0.12 (sawtooth drop pattern) |

### Diagnostic Notes

#### ISP CDN edge caching detected (`cache_effect`)
Target servers hosted inside local ISP cache nodes deliver near-line rates, masking international transit bottlenecks.
- Compare uncached international mirrors with domestic BDIX mirrors.

#### sparse time-of-day coverage (`insufficient_time_data`)
Stored samples cover 14/48 matrix cells; time-of-day congestion significance test is gated until >= 2 windows with >= 3 samples are collected.
- run one sample around 21:00 on a weekday (evening peak window)
- run one sample around 09:00 on a weekday (morning off-peak baseline)

## 3. What This Means for Downloads

### Domestic Sources (Local Mirrors, BDIX, In-Country Peers)
- Use **16 connections**. Domestic routes typically bypass international transit shapers.

### Uncached International Sources (GitHub Releases, HuggingFace, PyPI, Overseas Servers)
- Use **16 parallel connections** with stream recycling every **10s**.
- When single-host caps apply, split across mirror URLs (`--mirrors`) so multiple destinations run concurrently.

### YouTube & Cache-Fronted Sources (GGC / Local Edge Caches)
- High speeds on YouTube or ISP-cached CDNs reflect local edge caching (`cache effect`), not uncached international capacity.
- Do not use cached CDN speeds as the baseline expected rate for uncached overseas file transfers.

### Recommended Download Profile (`source: throttle-probe`)

- **Default Connections**: `16`
- **Domestic Connections**: `16`
- **International Connections**: `16`
- **Recycle Interval**: `10.0s`
- **Use Multi-Host Mirrors**: `True`
- **Try HTTP/3**: `False`
- **Try Proxy/Tunnel**: `False`
- **Preferred IP Family**: `auto`
- **Watchdog Recovery Action**: `tier2_pause_unpause`

- Use 16 parallel connections for international downloads to bypass the 4.8 Mbps per-connection cap and saturate the 60 Mbps link.
- Enable Watchdog v2 in-process socket recycling every 10–12 seconds to continuously harvest the 28 Mbps initial burst bucket.
- Prefer domestic / BDIX mirrors whenever available (16x throughput multiplier).

## 4. Module Results (M0–M10)

### M0: Sanity & Local Environment
- **Active Interface**: `en0` (Wi-Fi, SSID: `Duronto-Fiber-5G`)
- **Loopback Simulator Throughput**: 3774.87 Mbps (`ok=True`)
- **VPN/Tunnel Active**: `False` (none)
- **CPU 1m Load**: `0.85` | **Competing Traffic**: `0.1 MB`
- **Environment Contaminated**: `False`

### M3: Connection Flow Scaling

| Target | Locality | 1 Stream | 4 Streams | 16 Streams | 16x/1x Scaling |
| --- | --- | ---: | ---: | ---: | ---: |
| `speed.cloudflare.com` | international | 38.40 Mbps | 148.80 Mbps | 467.20 Mbps | 12.17x |

### M5: Burst & Token Bucket Analysis

| Target | Initial Burst | Sustained Rate | Burst Duration | Est. Bucket | Drop Shape |
| --- | ---: | ---: | ---: | ---: | --- |
| `speed.cloudflare.com` | 239.08 Mbps | 40.27 Mbps | 1.0s | 2.8 MB | `step` |

### M6: Policing vs Shaping vs Congestion

| Target | Idle RTT | Loaded RTT | RTT Inflation | Retransmit Rate | Mechanism Hint |
| --- | ---: | ---: | ---: | ---: | --- |
| `speed.cloudflare.com` | 18.5 ms | 21.2 ms | +2.7 ms (1.15x) | 2.10% | `policing` |

### M8: Time-of-Day Coverage & Sparse Sampling Status
- **Time-of-Day Verdict Status**: `insufficient time data`
- **Recorded Time-of-Day Samples**: `0`
- **Next Most Informative Hours to Sample**: 21:00 (weekday peak), 09:00 (weekday off-peak)

## 5. Neutral ISP Evidence Summary (For Support Escalation)

**ISP**: Duronto Internet

**Measurement Dates/Times**:
- `2026-11-08`

**Targets Tested**:
- speed.cloudflare.com (AS13335)
- mirror.dhaka.ix.bd (AS58718)

**Methodology**:
- Paired same-moment single-connection and 16-connection parallel HTTP/1.1 range chunk transfers.
- Fixed data budget enforcement with millisecond TCP connect, ping RTT, and netstat TCP retransmit delta telemetry.

**Measured Findings**:
- Domestic (BDIX) throughput: 78.5 Mbps median (4.8 ms latency, 0% loss)
- International single-stream throughput: 4.8 Mbps sustained ceiling
- International 16-stream throughput: 58.4 Mbps aggregate (12.2x scaling)
- International packet drop rate: 2.11% retransmits with negligible queue delay (+2.7 ms)

**Neutral Technical Questions for ISP Engineering**:
1. Does Duronto Internet apply a 5 Mbps per-TCP-session rate policer on upstream international IP transit?
1. Is international bandwidth allocated via a token bucket (burst limit ~25 Mbps for 1s, sustained 5 Mbps)?
1. Are specific BDIX routing optimizations available for high-throughput European/US CDN transit?

## 6. Reference Guides

### How to Run a Clean Test
1. Connect via **Ethernet** or stand close to your 5 GHz Wi-Fi router with no VPN or proxy active.
2. Pause active downloads, video streams, cloud backups, and software updates before starting.
3. Run `pdl throttle quick --isp "Duronto Internet"` for a 3-minute paired baseline, or `pdl throttle sample` at different hours of the day.
4. Run `pdl throttle coverage` to see which time-of-day buckets still need samples.

### How to Read the Verdict
- **`per-flow cap`**: Individual TCP streams are rate-limited, while multiple parallel connections (`-c 16`) scale up aggregate throughput.
- **`burst-then-throttle`**: Downloads start fast while a token bucket drains, then step down to a lower sustained rate.
- **`loss-based policing` vs `queue-based shaping`**: Policers drop excess packets immediately (sawtooth speed + TCP retransmits + low RTT growth), whereas shapers buffer packets in a queue (smooth speed cap + large RTT inflation under load).
- **`international bandwidth limit` / `peering/transit congestion`**: Domestic BDIX/peering and local CDN caches are fast, while uncached international routes are constrained on shared upstream transit ASNs.
- **`cache effect` note**: Fast speeds to ISP-cached CDN edges (such as GGC/Cloudflare local nodes) do not carry over to uncached overseas file hosts.
