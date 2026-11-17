// labserver's adaptive-stream "player" (/page?hls=1, ?dash=1, ?drm=1; one script per stream):
// fetches the manifest given in its data-manifest, the first video variant's and the audio's
// playlists and every segment, as a real player would, so a browser extension watching the
// network sees them. It decodes nothing; its <video> (data-player, else #player) gets a
// MediaSource (a blob: address) like a real player's. When every player on the page is done,
// <body data-stream> is "done" (or "error", with the reason in data-stream-error).
"use strict";
(() => {
  const script = document.currentScript;
  const manifest = new URL(script.dataset.manifest, location.href);
  const video = document.getElementById(script.dataset.player || "player");
  const players = document.querySelectorAll("script[data-manifest]").length;

  const fetchOk = async (url) => {
    const r = await fetch(url);
    if (!r.ok) throw new Error(`${url}: ${r.status}`);
    return r;
  };
  const text = async (url) => (await fetchOk(url)).text();
  const bytes = async (url) => (await fetchOk(url)).arrayBuffer();
  const lines = (s) => s.split(/\r?\n/).map((l) => l.trim()).filter(Boolean);

  async function hls(master) {
    const l = lines(await text(master));
    const variant = l.find((_, i) => i > 0 && l[i - 1].startsWith("#EXT-X-STREAM-INF"));
    const audio = /URI="([^"]+)"/.exec(l.find((x) => x.startsWith("#EXT-X-MEDIA:TYPE=AUDIO")) || "");
    for (const p of [variant, audio && audio[1]].filter(Boolean)) {
      const playlist = new URL(p, master);
      for (const seg of lines(await text(playlist)).filter((x) => !x.startsWith("#"))) {
        await bytes(new URL(seg, playlist));
      }
    }
  }

  // Seconds of an ISO 8601 duration such as PT12S or PT1M30.5S.
  const seconds = (d) => {
    const m = /PT(?:(\d+)H)?(?:(\d+)M)?(?:([\d.]+)S)?/.exec(d || "") || [];
    return (+m[1] || 0) * 3600 + (+m[2] || 0) * 60 + (+m[3] || 0);
  };

  async function dash(mpd) {
    const doc = new DOMParser().parseFromString(await text(mpd), "application/xml");
    const total = seconds(doc.documentElement.getAttribute("mediaPresentationDuration"));
    for (const set of doc.getElementsByTagName("AdaptationSet")) {
      const tpl = set.getElementsByTagName("SegmentTemplate")[0];
      const rep = set.getElementsByTagName("Representation")[0];
      if (!tpl || !rep) continue;
      const fill = (s, n) => s.replace("$RepresentationID$", rep.getAttribute("id")).replace("$Number$", n);
      const start = +(tpl.getAttribute("startNumber") || 1);
      const each = +tpl.getAttribute("duration") / +(tpl.getAttribute("timescale") || 1);
      await bytes(new URL(fill(tpl.getAttribute("initialization")), mpd));
      for (let n = start; n < start + Math.ceil(total / each); n++) {
        await bytes(new URL(fill(tpl.getAttribute("media"), n), mpd));
      }
    }
  }

  if (video && window.MediaSource) {
    try {
      video.src = URL.createObjectURL(new MediaSource());
    } catch {}
  }
  // HLS or DASH: data-kind, else the address's ending (#171's have none).
  const kind = script.dataset.kind || (manifest.pathname.endsWith(".mpd") ? "dash" : "hls");
  const run = kind === "dash" ? dash : hls;
  // A player's other text first (data-decoy): no manifest, never to be listed.
  const decoy = script.dataset.decoy ? fetch(new URL(script.dataset.decoy, location.href)).then((r) => r.text()) : Promise.resolve();
  decoy.then(() => run(manifest)).then(
    () => {
      const done = Number(document.body.dataset.streamsDone || 0) + 1;
      document.body.dataset.streamsDone = String(done);
      if (done >= players && document.body.dataset.stream !== "error") document.body.dataset.stream = "done";
    },
    (e) => {
      document.body.dataset.stream = "error";
      document.body.dataset.streamError = String(e && e.message);
    },
  );
})();
