// Mister Download Manager's download button over videos (a content script, in every page and
// frame). Our own design: a small pill at the video's edge that appears while the pointer is
// over the video (or it has the keyboard's focus) and fades when the pointer rests or leaves.
// Clicking it has the app look the video up (an embedded player's own address, which yt-dlp
// understands, with the video's own address when it has a plain one) and lists its qualities in a
// menu over the page; the one picked downloads without the app's window. An app that can't look
// up (an older one, another MDM chosen) opens its Download video window for this page instead,
// as does the menu's last command. It can be dragged along the video's edge, and stays where
// it was put on that site. Its menu hides it on this video, keeps it off the site (the app's
// Options → General → Download panels lists those sites), or opens those settings. The app
// decides where it may show at all (the native host: integration on, this browser captured,
// the button on, the site not excluded).
(() => {
  // One button per page: a copy cut off from the extension (after an update) gives way to the
  // new one.
  const prev = globalThis.__mdmVideoButton;
  if (prev && typeof prev === "object" && prev.alive()) return;
  if (prev && typeof prev.teardown === "function") prev.teardown();
  const ext = globalThis.browser ?? globalThis.chrome;

  // A video worth a button: big enough to be content, not a short muted looping clip (an
  // animated picture or a background). A sound player (<audio> with controls) gets one too.
  const MIN_W = 200;
  const MIN_H = 112;
  const MIN_AUDIO_H = 24;
  const CLIP_S = 15;
  // Shown this long after the pointer last moved over the video, kept while it is on the button.
  const IDLE_MS = 2600;
  const INSET = 10;
  const DRAG_PX = 4;
  // Our app icon (extension/icons/icon-48.png, as data: no extension file is exposed to pages).
  const LOGO = "data:image/png;base64,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";

  const t = (key, ...subs) => ext.i18n.getMessage(key, subs) || FALLBACK[key] || key;
  const FALLBACK = {
    video_download: "Download video",
    video_more: "More options",
    video_hide_here: "Hide on this video",
    video_settings: "Button settings...",
    video_sent: "Sent to Mister Download Manager",
    video_failed: "Mister Download Manager isn't installed or didn't start",
    video_off: "The download button is off on this site",
    video_drm: "Protected video: it can't be saved",
    video_unnamed: "Can't tell which video this is: open its own page and try there",
    video_open_post: "Open the post, then use the button there",
    video_drag: "Drag to move it along the video's edge",
    video_looking: "Looking up the qualities…",
    video_open_mdm: "Open in Mister Download Manager…",
    video_no_choices: "No video to download here",
    video_playlist: "A list of videos: choose them in Mister Download Manager",
    video_no_ytdlp: "Mister Download Manager needs yt-dlp for this video",
    video_too_long: "Looking up took too long",
    video_busy: "Mister Download Manager is busy with other videos",
    video_queued: "Added to the queue",
    video_failed_lookup: "Could not look it up",
    video_tab_video: "Video",
    video_tab_audio: "Audio",
  };
  // Sizes as the browser's language writes them.
  const lang = ext.i18n.getUILanguage?.() || undefined;
  // To the background. After the extension was updated or removed, this page's copy of the script
  // is cut off from it (calls throw at once): then it takes itself away.
  const send = (m) => {
    try {
      if (!ext.runtime?.id) throw new Error("gone");
      return ext.runtime.sendMessage(m).catch(() => ({ ok: false }));
    } catch {
      teardown();
      return Promise.resolve({ ok: false, gone: true });
    }
  };
  const host = location.hostname;
  // The address the app looks the video up by — the video's, not the page's. Most pages that play
  // a video are not the video's page: YouTube's home, a search, a channel or a playlist carries a
  // list in its address ("recommended", the results, the channel's videos), and yt-dlp reads such
  // an address as the list, so the app offered the whole feed instead of the video the button was
  // on. A watch page opened from a mix or a playlist has the same problem one level down
  // (…&list=…&index=…): its own page is watch?v=<id>, and nothing else.
  //
  // Null when the page plays a video but its DOM doesn't name it: YouTube's channel masthead player
  // has no video-id attribute, no link to the video inside the player and a blob: source. The page
  // itself never goes to the app then (a channel is a whole list to yt-dlp): the background asks
  // the player what it plays, in the page's own world (background.js playingVideo), and this
  // answer is only what it falls back on.
  //
  // Elsewhere the same, generalised (#689, #643): a video in a feed (LinkedIn's, Facebook's, X's,
  // Reddit's…) sits in the post it belongs to, and that post names its own address (its
  // permalink, its timestamp link, the post id the page gives it); `ownAddress` finds it the way
  // a person would. Null there too when nothing names it: a feed's address is never the video's.
  //
  // ---- the video's own address: from here to its end mark, run as is on saved pages by
  // tools/extension/test.mjs (only `location`, `document` and the elements' parentElement,
  // parentNode.host, localName, getAttribute, querySelector(All) with simple selectors).
  function videoPage(v) {
    const watch = (id) =>
      /^[\w-]+$/.test(id || "") ? `https://www.youtube.com/watch?v=${id}` : null;
    try {
      const url = new URL(location.href);
      const name = url.hostname.replace(/^(www|m|music)\./, "");
      if (name === "youtu.be") {
        const id = url.pathname.split("/")[1];
        if (watch(id)) return `https://youtu.be/${id}`;
      }
      if (name === "youtube.com" || name === "youtube-nocookie.com") {
        // An embed's own page is the video's (…/embed/<id>).
        if (url.pathname.startsWith("/embed/")) {
          const id = watch(url.pathname.split("/")[2]);
          if (id) return id;
        }
        // A Short's or a live stream's own page (/shorts/<id>, /live/<id>). yt-dlp reads these as
        // the same one video as watch?v=<id>.
        const [, kind, pathId] = url.pathname.split("/");
        if (kind === "shorts" || kind === "live") {
          const id = watch(pathId);
          if (id) return id;
        }
        // The page is this video's own: a watch page, with or without a list on it.
        const own = watch(url.searchParams.get("v"));
        if (own && url.pathname.startsWith("/watch")) return own;
        const shown = watch(document.querySelector("ytd-watch-flexy")?.getAttribute("video-id"));
        if (shown && url.pathname.startsWith("/watch")) return shown;
        const player = watch(document.querySelector("#movie_player")?.getAttribute("video-id"));
        if (player) return player;
        // A player inside the card it belongs to (a feed, a search, a playlist): the card links to
        // the video. Only a container that names exactly one video counts — walking on into the
        // page's column would find some other card's link and send the wrong video, which is worse
        // than sending none.
        for (let node = v, i = 0; node && i < 12; i += 1, node = node.parentElement) {
          const ids = new Set();
          for (const a of node.querySelectorAll?.('a[href*="/watch?"], a[href*="youtu.be/"]') || []) {
            const link = new URL(a.href, location.href);
            const id = link.hostname.endsWith("youtu.be")
              ? link.pathname.split("/")[1]
              : link.searchParams.get("v");
            if (watch(id)) ids.add(id);
          }
          if (ids.size === 1) return watch([...ids][0]);
          if (ids.size > 1) break;
        }
        // A page that plays a video and names none: not the video's page, so not the app's.
        return null;
      }
      return ownAddress(v, url);
    } catch {
      // Not an address this page can reason about: its own follows.
    }
    return location.href;
  }

  // ---- the qualities: the app looks the video up (yt-dlp), the user picks one here ------------
  // How long a lookup is followed, from the click.
  const LOOKUP_MS = 60000;

  // The wait before asking how a lookup is going: 100 ms before the first question. An app that
  // held the last question while it looked (a newer one, up to ~1.5 s: `took` ms of HELD_MS or
  // more) is asked again at once (50 ms), so the answer reaches the page as soon as it is ready.
  // One that answers at once (an older one) is asked as seldom as ever: the old backoff's target
  // (1 s, a little longer each time, 2 s at most) less the time the question took, 50 ms at least.
  function pollDelay(n, took) {
    const HELD_MS = 1000;
    if (n <= 0) return 100;
    const spent = Number.isFinite(took) ? Math.max(0, took) : 0;
    if (spent >= HELD_MS) return 50;
    const target = Math.min(2000, Math.round(1000 * 1.25 ** n));
    return Math.max(50, Math.round(target - spent));
  }

  // A size as the browser's language writes it ("178.8 MB"); "" when unknown.
  function sizeText(bytes, lang) {
    const n = Number(bytes);
    if (!Number.isFinite(n) || n <= 0) return "";
    const units = [["gigabyte", 2 ** 30], ["megabyte", 2 ** 20], ["kilobyte", 2 ** 10]];
    const [unit, per] = units.find(([, u]) => n >= u) || ["byte", 1];
    try {
      return new Intl.NumberFormat(lang, { style: "unit", unit, unitDisplay: "short", maximumFractionDigits: 1 }).format(n / per);
    } catch {
      return `${(n / per).toFixed(1)} ${unit}`;
    }
  }

  // What a quality's row says: the app's name for it (in the app's language) and its size. Only
  // ever set as text, never as markup.
  function rowText(choice, lang) {
    const c = choice && typeof choice === "object" ? choice : {};
    return { name: String(c.label ?? "").slice(0, 200), size: sizeText(c.size, lang) };
  }

  // Which of the app's qualities go in which tab: `choice.audio === true` is sound only, anything
  // else (an app that doesn't say, an older one) is video. Tabs only when both kinds are there;
  // otherwise one list (`all`) in the app's order. Every number is the quality's place in the
  // app's own list (what a pick sends), never its place in a tab.
  function groupChoices(choices, max) {
    const list = Array.isArray(choices) ? choices.slice(0, Math.max(0, Number.isFinite(max) ? max : 100)) : [];
    const video = [];
    const audio = [];
    list.forEach((c, i) => (c && typeof c === "object" && c.audio === true ? audio : video).push(i));
    return {
      tabbed: video.length > 0 && audio.length > 0,
      video,
      audio,
      all: list.map((_, i) => i),
      start: video.length || !audio.length ? "video" : "audio",
    };
  }

  // Follows a lookup the app is running, from its first answer: asks again only once the last
  // question was answered (never on a timer of its own), pollDelay later, until it is no longer
  // pending (that answer, with its token), the minute is up ({state: "late"}) or it is no longer
  // wanted (null: the menu closed, the video or the page changed). `ask(token, ms)`: the
  // background's answer, or {late: true} after `ms`; `sleep(ms)` may end early when dropped.
  async function followLookup(first, { ask, alive, sleep, now, end }) {
    let s = first;
    let token = "";
    let took = 0;
    for (let n = 0; ; n += 1) {
      if (!alive()) return null;
      if (!s || typeof s !== "object") return { state: "gone" };
      if (typeof s.token === "string" && s.token) token = s.token;
      if (s.state !== "pending") return { ...s, token };
      if (!token) return { state: "gone" };
      const left = end - now();
      if (left <= 0) return { state: "late" };
      await sleep(Math.min(pollDelay(n, took), left));
      if (!alive()) return null;
      if (end - now() <= 0) return { state: "late" };
      const asked = now();
      const r = await ask(token, end - asked);
      took = now() - asked;
      if (!alive() || !r || r.gone) return null;
      if (r.late) return { state: "late" };
      // Another MDM chosen meanwhile: the window, as for a click.
      s = r.fallback ? { state: "use-window" } : r.lookup;
    }
  }

  // The background's answer to a lookup or to a choice, as a lookup's state: null when there is
  // nothing to show (the extension gone); "late" past the minute, "unnamed" for a video it can't
  // tell, "feed" for a video in a feed no post names (#689), and "use-window" for no lookups here
  // (another MDM, an older one) or an answer that isn't one.
  function replyState(r) {
    if (r && r.gone) return null;
    if (r && r.late) return { state: "late" };
    if (r && (r.why === "unnamed" || r.why === "feed")) return { state: r.why };
    if (!r || r.fallback || !r.lookup || typeof r.lookup !== "object") return { state: "use-window" };
    return r.lookup;
  }

  // What a lookup's state (or replyState's) comes to, as the menu does it: "follow" it while the
  // app looks, "offer" its qualities, "say" a message in the menu (with the app's own words under
  // it), "flash" a word on the button (the menu closed), or "window": the click as it always was,
  // the app's Download video window. `t`: the texts. `picked`: the answer to a choice, which only
  // a download confirms; anything else there is the window's.
  function lookupAction(s, t, picked) {
    const state = s && typeof s === "object" && typeof s.state === "string" ? s.state : "";
    // A choice taken: downloading, in the queue, or handed to the app, which didn't say in time
    // what became of it ("sent": it will download it; the window would download it again).
    if (state === "started" || state === "sent") return { act: "flash", kind: "ok", text: t("video_sent") };
    if (state === "queued") return { act: "flash", kind: "ok", text: t("video_queued") };
    if (!picked) {
      if (state === "pending") return { act: "follow" };
      if (state === "ready" && Array.isArray(s.choices) && s.choices.length) return { act: "offer" };
      if (state === "ready" || (state === "failed" && s.reason === "no-choices")) return { act: "say", text: t("video_no_choices") };
      if (state === "failed" && s.reason === "no-ytdlp") return { act: "say", text: t("video_no_ytdlp") };
      if (state === "failed") return { act: "say", text: t("video_failed_lookup"), detail: typeof s.message === "string" ? s.message : "" };
      if (state === "late") return { act: "say", text: t("video_too_long") };
      if (state === "list") return { act: "say", text: t("video_playlist") };
      if (state === "busy") return { act: "say", text: t("video_busy") };
      if (state === "off") return { act: "flash", kind: "err", text: t("video_off") };
      if (state === "unnamed") return { act: "flash", kind: "err", text: t("video_unnamed") };
      if (state === "feed") return { act: "flash", kind: "err", text: t("video_open_post") };
    }
    // "use-window", "gone" (the app forgot it: restarted), a choice the app didn't take, and
    // anything newer: the window.
    return { act: "window" };
  }

  // The click's window at most once, whichever way the click comes to it (an answer that isn't a
  // lookup, the lookup's end, a choice the app didn't take, "Open in…"): true the first time for
  // `j`, the click's lookup.
  function firstWindow(j) {
    if (!j || j.windowed) return false;
    j.windowed = true;
    return true;
  }

  // The video the pointer takes the button to: the one under it (`found`), but not another than
  // its own (`shown`) while the qualities' menu is open (`picking`): the menu reaches over the
  // videos around its own (a feed's), and the pointer crosses them on its way to a row. The menu
  // still closes when its video plays something else or the page changes (tick), or on a click
  // elsewhere.
  function pointerTarget(found, shown, picking) {
    if (!found || (picking && found !== shown)) return null;
    return found;
  }

  // Up from a node: its parent, or the element whose shadow root holds it (Reddit's player keeps
  // its <video> in one).
  const upFrom = (n) => n.parentElement || (n.parentNode && n.parentNode.host) || null;

  // `href` as an address (relative to the page), or null.
  function linkTo(href) {
    try {
      return href ? new URL(href, location.href) : null;
    } catch {
      return null;
    }
  }

  // The nearest container around the video that names its post: `read(node)` gives what a
  // container names, { sure, maybe } (Maps of an id to the address it stands for), `sure` the
  // post's or the video's own ids, `maybe` weaker ones (a link that may be the post's). The
  // innermost container naming anything decides, and only when it names exactly one: one naming
  // two has taken in the next post or the page's column, and a wrong video is worse than none.
  // The walk ends at the page's column (a feed, role="feed") and at a container holding another
  // video: from there up, what it names may be that one's (a post naming nothing next to one
  // that names its own). A page that breaks a reader's expectations names nothing (never the
  // page's own address instead).
  function nearest(v, read) {
    try {
      for (let node = v, i = 0; node && i < 16; i += 1, node = upFrom(node)) {
        if (node.localName === "body" || node.localName === "html") break;
        if (node !== v && (node.getAttribute?.("role") === "feed" || manyVideos(node))) break;
        const { sure, maybe } = read(node);
        if (sure.size) return sure.size === 1 ? [...sure.values()][0] : null;
        if (maybe.size) return maybe.size === 1 ? [...maybe.values()][0] : null;
      }
    } catch {
      // Nothing named.
    }
    return null;
  }

  const decoded = (s) => {
    try {
      return decodeURIComponent(s);
    } catch {
      return s;
    }
  };

  // The links in a container (the container itself when it is one), as addresses, with whether
  // each holds a <time> (a timestamp link: a post's own permalink).
  function linksIn(node, sel = "a[href]") {
    const found = [];
    const own = node.localName === "a" ? [node] : [];
    for (const a of [...own, ...(node.querySelectorAll ? node.querySelectorAll(sel) : [])]) {
      const u = linkTo(a.getAttribute && a.getAttribute("href"));
      if (u && /^https?:$/.test(u.protocol)) found.push({ u, time: Boolean(a.querySelector && a.querySelector("time")) });
    }
    return found;
  }

  // The page's site without its www. or mobile host (old.reddit.com is reddit.com).
  const siteName = (host) => host.replace(/^(www|m|mobile|web|touch|old|new|sh|music|geo|player)\./, "");

  // A video's own address on this page, by site; the page's own when it is that one video's page;
  // null when the page is a feed, a list or a search and nothing names the video.
  function ownAddress(v, url) {
    const site = SITES.find((s) => s.hosts.test(siteName(url.hostname)));
    if (site) {
      // A site's own player in another page (an embed) is its video's page: its address holds
      // what the links around it may lack (a private video's key).
      if (site.embed && site.embed(url)) return location.href;
      const found = nearest(v, (node) => site.read(node, url));
      if (found) return found;
      return site.own(url) ? location.href : null;
    }
    // Any other site: a player embedded in another page (its own frame), or the one video of its
    // page, is its video's own page, as before.
    if (framed() || (!inFeed(v) && document.querySelectorAll?.("video").length <= 1)) return location.href;
    // A page of many videos: the post around this one, when one names it; else the page in
    // general (an article with two players is still theirs to yt-dlp), but never a feed's
    // (role="feed") or a page whose address is a feed's, a home's or a search's.
    const found = nearest(v, (node) => genericRead(node, url));
    if (found) return found;
    return inFeed(v) || FEED_PATH.test(url.pathname) ? null : location.href;
  }

  // A feed's, a home page's, a search's address: many videos, none of them the page's.
  const FEED_PATH = /^\/(?:$|home\/?$|feed|explore|search|discover|trending|popular|latest|timeline|for-?you|following|watch\/?$)/i;
  // The video sits in a feed (role="feed": Mastodon's, the accessible feeds).
  function inFeed(v) {
    for (let node = upFrom(v); node; node = upFrom(node)) if (node.getAttribute?.("role") === "feed") return true;
    return false;
  }

  const framed = () => {
    try {
      return window.top !== window;
    } catch {
      return false;
    }
  };
  const manyVideos = (node) => Boolean(node.querySelectorAll && node.querySelectorAll("video").length > 1);

  // Any other site: a link of this site that has a post's or a video's shape, or that holds the
  // post's timestamp (Mastodon's, a forum's), and isn't the page itself.
  function genericRead(node, url) {
    const sure = new Map();
    const maybe = new Map();
    const here = `${url.origin}${url.pathname}${url.search}`;
    for (const { u, time } of linksIn(node)) {
      if (siteName(u.hostname) !== siteName(url.hostname)) continue;
      const address = `${u.origin}${u.pathname}${u.search}`;
      if (address === here) continue;
      if (time) sure.set(address, address);
      else if (/\/(?:status|statuses|videos?|watch|posts?|p|reels?|comments|clips?|v)(?:\/|\?|$)/i.test(u.pathname + (u.search ? "?" : ""))) maybe.set(address, address);
    }
    const permalink = linkTo(node.getAttribute?.("data-permalink"));
    if (permalink && siteName(permalink.hostname) === siteName(url.hostname)) sure.set(permalink.href, permalink.href);
    return { sure, maybe };
  }

  // LinkedIn's comment tools' key: base64url of a protobuf naming the post (field 1 its
  // activity, field 2 its ugcPost), whose one field is the id as a zigzag varint. [kind, id] or
  // null.
  function commentToolsUrn(key) {
    try {
      const bytes = [...atob(key.replace(/-/g, "+").replace(/_/g, "/"))].map((c) => c.charCodeAt(0));
      const kind = { 10: "activity", 18: "ugcPost" }[bytes[0]];
      if (!kind || bytes[1] !== bytes.length - 2 || bytes[2] !== 8) return null;
      let n = 0n;
      let shift = 0n;
      for (let i = 3; i < bytes.length; i += 1) {
        n |= BigInt(bytes[i] & 0x7f) << shift;
        shift += 7n;
        if (!(bytes[i] & 0x80)) {
          if (i !== bytes.length - 1 || n & 1n) return null;
          const id = String(n >> 1n);
          return /^\d{15,20}$/.test(id) ? [kind, id] : null;
        }
      }
    } catch {
      // Not such a key.
    }
    return null;
  }

  // The big sites with feeds: what names a post in their pages, the address yt-dlp reads it by
  // (its extractors' own address shapes), and their pages that are one video's own.
  const SITES = [
    {
      // A post's container carries its URN (data-urn, data-id); its links name it too. yt-dlp's
      // LinkedIn extractor reads /feed/update/urn:li:activity:<id>/ (and /posts/…); a post known
      // only by its ugcPost URN goes as /feed/update/urn:li:ugcPost:<id>/, the same public page,
      // where yt-dlp's generic reader finds the video.
      //
      // LinkedIn's feed since 2026 (its server-driven app: role="listitem" posts in a
      // data-testid="mainFeed" list) names a post nowhere near its video, only in the keys of
      // the post's parts (componentkey): its comments' URNs (urn:li:comment:(activity:<id>,…)),
      // its text's translation key (…userGeneratedContentId=<id>…, its ugcPost), and its comment
      // tools' key, <base64>-replaceableCommentTools, a protobuf of the post's URN (field 1 its
      // activity, field 2 its ugcPost, the id zigzag-encoded). Checked against a signed-in feed:
      // each decodes to the post its comments name, and each ugcPost's page plays the video.
      hosts: /^linkedin\.com$/,
      embed: (u) => /^\/embed\//.test(u.pathname),
      read(node) {
        const sure = new Map();
        const maybe = new Map();
        const post = (kind, id) =>
          (kind === "activity" ? sure : maybe).set(`${kind}:${id}`, `https://www.linkedin.com/feed/update/urn:li:${kind}:${id}/`);
        const add = (s) => {
          s = String(s || "");
          for (const m of s.matchAll(/urn:li:(?:comment:\()?(activity|ugcPost|share):(\d{6,})/g)) post(m[1], m[2]);
          for (const m of s.matchAll(/\b(userGeneratedContent|share|activity)Id=(\d{6,})/g)) post(m[1] === "userGeneratedContent" ? "ugcPost" : m[1], m[2]);
          // A post's public address: /posts/<who>_<words>-activity-<id>-<four>.
          const p = /\/posts\/[^/?#]*-(activity|ugcPost)-(\d{6,})-\w{4}/.exec(s);
          if (p) post(p[1], p[2]);
          const k = /^([\w-]{12,40})-replaceableCommentTools/.exec(s);
          const urn = k && commentToolsUrn(k[1]);
          if (urn) post(urn[0], urn[1]);
        };
        // The container's own attributes, whatever their names (the page renames them), and its
        // insides' known ones.
        for (const a of node.attributes || []) add(a.value);
        for (const el of node.querySelectorAll?.("[data-urn], [data-id], [data-activity-urn], [data-entity-urn], [componentkey]") || []) {
          for (const a of ["data-urn", "data-id", "data-activity-urn", "data-entity-urn", "componentkey"]) add(el.getAttribute?.(a));
        }
        for (const { u } of linksIn(node)) if (/linkedin\.com$/.test(u.hostname)) add(decoded(u.pathname));
        return { sure, maybe };
      },
      own: (u) => /^\/(?:feed\/update\/urn:li:|posts\/|embed\/feed\/update\/|events\/|learning\/)/.test(u.pathname),
    },
    {
      // A video's own links (a reel, a watch page, a page's video, data-video-id) before a post's
      // (its permalink); fb.watch short links as they are (they lead to the video's page).
      hosts: /^(?:[\w-]+\.)?facebook\.com$/,
      embed: (u) => /^\/plugins\/video\.php$/.test(u.pathname),
      read(node) {
        const sure = new Map();
        const maybe = new Map();
        const video = (id) => sure.set(`v:${id}`, `https://www.facebook.com/watch/?v=${id}`);
        const id = node.getAttribute?.("data-video-id");
        if (/^\d{6,}$/.test(id || "")) video(id);
        for (const el of node.querySelectorAll?.("[data-video-id]") || []) {
          const d = el.getAttribute("data-video-id");
          if (/^\d{6,}$/.test(d || "")) video(d);
        }
        for (const { u } of linksIn(node)) {
          if (u.hostname === "fb.watch") {
            const code = u.pathname.split("/")[1];
            if (/^\w+$/.test(code || "")) sure.set(`w:${code}`, `https://fb.watch/${code}/`);
            continue;
          }
          if (!/(^|\.)facebook\.com$/.test(u.hostname)) continue;
          const p = u.pathname;
          let m;
          if ((m = /^\/reel\/(\d+)/.exec(p))) sure.set(`v:${m[1]}`, `https://www.facebook.com/reel/${m[1]}/`);
          else if (/^\/watch\/?$/.test(p) && /^\d+$/.test(u.searchParams.get("v") || "")) video(u.searchParams.get("v"));
          else if (/^\/video\.php$/.test(p) && /^\d+$/.test(u.searchParams.get("v") || "")) video(u.searchParams.get("v"));
          else if ((m = /^\/[^/]+\/videos\/(?:[^/]+\/)?(\d+)/.exec(p))) video(m[1]);
          else if ((m = /^\/([^/]+)\/posts\/(pfbid\w+|\d+)/.exec(p))) maybe.set(`p:${m[2]}`, `https://www.facebook.com/${m[1]}/posts/${m[2]}`);
          else if ((m = /^\/groups\/([^/]+)\/(?:permalink|posts)\/(\d+)/.exec(p))) maybe.set(`p:${m[2]}`, `https://www.facebook.com/groups/${m[1]}/posts/${m[2]}/`);
          else if (/^\/(?:permalink|story)\.php$/.test(p) && /^(?:pfbid\w+|\d+)$/.test(u.searchParams.get("story_fbid") || "")) {
            const story = u.searchParams.get("story_fbid");
            const owner = u.searchParams.get("id");
            maybe.set(`p:${story}`, `https://www.facebook.com/permalink.php?story_fbid=${story}${/^\d+$/.test(owner || "") ? `&id=${owner}` : ""}`);
          }
        }
        return { sure, maybe };
      },
      own: (u) =>
        /^\/(?:reel\/\d|watch\/?$|video\.php$|[^/]+\/videos\/|[^/]+\/posts\/|permalink\.php$|story\.php$|groups\/[^/]+\/(?:permalink|posts)\/|plugins\/video\.php$|share\/[vrp]\/)/.test(u.pathname) &&
        (!/^\/watch\/?$/.test(u.pathname) || /^\d+$/.test(u.searchParams.get("v") || "")),
    },
    {
      // A post's timestamp links to it (/<user>/status/<id>); other status links (a quoted post,
      // its photos and its analytics) count only when there is no timestamp.
      hosts: /^(?:x|twitter)\.com$/,
      read(node) {
        const sure = new Map();
        const maybe = new Map();
        for (const { u, time } of linksIn(node, 'a[href*="/status/"]')) {
          const m = /^\/(\w{1,15}|i\/web)\/status\/(\d+)/.exec(u.pathname);
          if (!m || !/(^|\.)(?:x|twitter)\.com$/.test(u.hostname)) continue;
          (time ? sure : maybe).set(m[2], `${location.origin}/${m[1]}/status/${m[2]}`);
        }
        return { sure, maybe };
      },
      own: (u) => /^\/(?:\w{1,15}|i\/web)\/status\/\d+|^\/i\/(?:broadcasts|spaces)\//.test(u.pathname),
    },
    {
      // A post's links (/p/<code>/, a reel's /reel/<code>/): its timestamp, its comments.
      hosts: /^instagram\.com$/,
      embed: (u) => /\/embed\/?$/.test(u.pathname),
      read(node) {
        const sure = new Map();
        for (const { u } of linksIn(node)) {
          // Not a reel's sound (/reels/audio/<id>/, the other reels using it).
          const m = /^(?:\/[\w.]+)?\/(p|tv|reels?)\/(?!audio\/)([\w-]{5,})/.exec(u.pathname);
          if (!m || !/instagram\.com$/.test(u.hostname)) continue;
          sure.set(m[2], `https://www.instagram.com/${m[1] === "tv" ? "p" : m[1].replace(/s$/, "")}/${m[2]}/`);
        }
        return { sure, maybe: new Map() };
      },
      own: (u) => /^(?:\/[\w.]+)?\/(?:p|tv|reels?)\/[\w-]{5,}|^\/stories\/[^/]+\/\d/.test(u.pathname),
    },
    {
      // A post (shreddit-post, old Reddit's .thing) names its permalink in its attributes; its
      // links (title, comments) name the same /comments/<id>/.
      hosts: /^reddit\.com$/,
      read(node) {
        const sure = new Map();
        const add = (s) => {
          const m = /^(\/(?:r|u|user)\/[^/]+)?\/comments\/(\w+)/.exec(linkTo(s)?.pathname || "");
          if (m) sure.set(m[2], `${location.origin}${m[1] || ""}/comments/${m[2]}/`);
        };
        add(node.getAttribute?.("permalink"));
        add(node.getAttribute?.("data-permalink"));
        for (const el of node.querySelectorAll?.("[permalink], [data-permalink]") || []) {
          add(el.getAttribute("permalink"));
          add(el.getAttribute("data-permalink"));
        }
        for (const { u } of linksIn(node, 'a[href*="/comments/"]')) if (/reddit\.com$/.test(u.hostname)) add(u.href);
        return { sure, maybe: new Map() };
      },
      own: (u) => /\/comments\/\w+|^\/r\/[^/]+\/s\/\w+/.test(u.pathname),
    },
    {
      // A video's link (/@<user>/video/<id>); else its player's id (xgwrapper-<n>-<id>), sent in
      // yt-dlp's own form for a video whose author isn't known (/@_/video/<id>).
      hosts: /^tiktok\.com$/,
      embed: (u) => /^\/embed\//.test(u.pathname),
      read(node) {
        const sure = new Map();
        for (const { u } of linksIn(node, 'a[href*="/video/"]')) {
          const m = /^\/@([\w.-]+)\/video\/(\d+)/.exec(u.pathname);
          if (m && /tiktok\.com$/.test(u.hostname)) sure.set(m[2], `https://www.tiktok.com/@${m[1]}/video/${m[2]}`);
        }
        if (!sure.size) {
          for (const el of [node, ...(node.querySelectorAll?.('[id^="xgwrapper-"]') || [])]) {
            const m = /^xgwrapper-\d+-(\d{15,})$/.exec(el.getAttribute?.("id") || "");
            if (m) sure.set(m[1], `https://www.tiktok.com/@_/video/${m[1]}`);
          }
        }
        return { sure, maybe: new Map() };
      },
      own: (u) => /^\/@[\w.-]+\/(?:video|photo)\/\d+|^\/embed\//.test(u.pathname),
    },
    {
      // A clip's links: vimeo.com/<id> (with an unlisted video's hash), in a channel, a group, a
      // showcase.
      hosts: /^(?:player\.)?vimeo\.com$/,
      embed: (u) => u.hostname.startsWith("player."),
      read(node) {
        const sure = new Map();
        for (const { u } of linksIn(node)) {
          if (!/(^|\.)vimeo\.com$/.test(u.hostname)) continue;
          const m = /^(?:\/video|\/channels\/[^/]+|\/groups\/[^/]+\/videos|\/showcase\/[^/]+\/video)?\/(\d{4,})(\/[\da-f]{10})?\/?$/.exec(u.pathname);
          if (m) sure.set(m[1], `https://vimeo.com/${m[1]}${m[2] || ""}`);
        }
        return { sure, maybe: new Map() };
      },
      own: (u) =>
        u.hostname.startsWith("player.") ||
        /^(?:\/channels\/[^/]+|\/groups\/[^/]+\/videos|\/showcase\/[^/]+\/video|\/[^/]+\/review)?\/\d{4,}|^\/ondemand\//.test(u.pathname),
    },
    {
      hosts: /^(?:geo\.)?dailymotion\.com$/,
      embed: (u) => u.hostname.startsWith("geo.") || /^\/(?:embed|player)\b/.test(u.pathname),
      read(node) {
        const sure = new Map();
        for (const { u } of linksIn(node, 'a[href*="/video/"]')) {
          const m = /^\/(?:embed\/)?video\/([a-z0-9]+)/i.exec(u.pathname);
          if (m && /dailymotion\.com$/.test(u.hostname)) sure.set(m[1], `https://www.dailymotion.com/video/${m[1]}`);
        }
        return { sure, maybe: new Map() };
      },
      own: (u) => u.hostname.startsWith("geo.") || /^\/(?:embed\/)?video\/[a-z0-9]+|^\/player/i.test(u.pathname),
    },
  ];
  // ---- the video's own address: end.

  // The address this page first loaded at (`download`: a page that moves in place keeps its first
  // page's og:title).
  const landedAt = location.href;
  const reduced = matchMedia("(prefers-reduced-motion: reduce)").matches;

  // "Hide on this video": the video's address (players reuse one element for the next video),
  // else the element.
  const dismissed = new WeakSet();
  const dismissedSrc = new Set();
  const isDismissed = (v) => dismissed.has(v) || (v.currentSrc && dismissedSrc.has(v.currentSrc));
  // May it show here? The app's answer (Options: the button on, the site not listed), asked
  // again when older than ASK_MS, so a change in Options reaches a page already open (#532: one
  // that never reloads, as YouTube's, kept the button for good). `never`: this page's own
  // "Never on this site".
  let allowed = null;
  let askedAt = 0;
  let asking = null;
  let never = false;
  let placed = false;
  const ASK_MS = 5000;
  const answered = () => allowed !== null && Date.now() - askedAt < ASK_MS;
  let place = { x: 1, y: 0 }; // where on the edge: fractions of the room, top right by default
  let chosen = false; // the user put it where it is (dragged it, on this site): theirs, never moved for a page's control
  let auto = { v: null, at: null, t: 0 }; // the spot found for this video, kept for a moment
  const AUTO_MS = 400;
  // Where it is on the screen, while the pointer is on it (#560): { v, left, top }.
  let pinned = null;
  let video = null; // the video it is over
  // A video gone from under its button (#503): a hover-to-play preview (YouTube's thumbnails)
  // stops as soon as the pointer leaves it for the button, since the page sees the pointer go.
  // The button stays where it was, for that video, while the pointer is on it (or its menu is
  // open), or for HOLD_MS while the pointer is still over where the video was, on its way to it;
  // a click downloads what the video was playing.
  let held = null; // { r: the video's last rectangle, at: the button's spot, d: what download sends, since }
  const HOLD_MS = 1500;
  let lastRect = null; // the video's rectangle when the button was last placed over it
  let entered = null; // what download would send, taken as the pointer reached the button: { v, d, t }
  let lastMove = 0;
  let pointer = { x: -1, y: -1 };
  let menuOpen = false;
  let dragging = null;
  let busy = false;
  // The lookup the qualities menu is for (one at a time), until the menu closes.
  let job = null;
  let flashing = false;
  let raf = 0;
  let scan = 0;
  // Kept on screen while the keyboard is on it (not after a mouse click: that leaves focus too),
  // or on the video's player (its own controls).
  const keyboardOn = () =>
    Boolean(shadow.activeElement && shadow.activeElement.matches(":focus-visible")) || playerFocused();
  let focusedPlayer = null;
  const playerFocused = () =>
    Boolean(video && focusedPlayer && document.activeElement === focusedPlayer &&
      focusedPlayer.contains(video) && focusedPlayer.matches(":focus-visible"));

  // ---- the button: a closed shadow tree the page's styles and scripts can't reach ----------
  const root = document.createElement("mdm-video-button");
  // Each with !important: a page's own rules can't hide or move it.
  for (const [k, v] of Object.entries({
    all: "initial", position: "fixed", "z-index": "2147483647", left: "0", top: "0", display: "none",
    contain: "layout style", opacity: "1", visibility: "visible", "pointer-events": "auto",
  })) root.style.setProperty(k, v, "important");
  const shadow = root.attachShadow({ mode: "closed" });
  const CSS = `
:host { all: initial; }
* { box-sizing: border-box; }
/* A light, glossy panel that stands out over a dark video, with our icon: the classic download
   button's feel, made modern (rounded, crisp type, soft shadow and motion). */
.pill {
  display: flex; align-items: stretch; height: 34px; border-radius: 9px;
  background: linear-gradient(#ffffff, #f1f4f8 48%, #e3e8ef 52%, #eef2f6);
  color: #17324d; border: 1px solid rgba(23, 50, 77, .28);
  box-shadow: 0 4px 14px rgba(0, 0, 0, .32), 0 1px 0 rgba(255, 255, 255, .9) inset;
  font: 600 13px/1 "Segoe UI", system-ui, -apple-system, Roboto, sans-serif;
  opacity: 0; transform: translateY(-3px) scale(.98);
  transition: opacity .16s ease-out, transform .16s ease-out, filter .12s;
  user-select: none; -webkit-user-select: none; touch-action: none; white-space: nowrap;
}
.pill.shown { opacity: 1; transform: none; }
.pill.dragging { cursor: grabbing; box-shadow: 0 10px 28px rgba(0, 0, 0, .45); transition: none; }
.pill.dragging .go { cursor: grabbing; }
.pill.ok { background: linear-gradient(#f2fff6, #d4f3df); border-color: #2f9a5b; color: #135c33; }
.pill.err { background: linear-gradient(#fff4f4, #f6d6d6); border-color: #c24848; color: #7d1f1f; }
button {
  /* all: unset resets box-sizing too, past the rule above: without it a menu item's 100 % width
     plus its padding ran past the menu's edge (#81). */
  all: unset; box-sizing: border-box; display: flex; align-items: center; gap: 8px;
  color: inherit; cursor: pointer; font: inherit;
}
.go { padding-block: 0; padding-inline: 8px 12px; border-start-start-radius: 8px; border-end-start-radius: 8px; cursor: grab; }
.more { padding: 0 9px; border-inline-start: 1px solid rgba(23, 50, 77, .18); border-start-end-radius: 8px; border-end-end-radius: 8px; }
.go:hover, .more:hover { background: linear-gradient(rgba(214, 232, 255, .9), rgba(182, 212, 250, .9)); }
.go:active, .more:active { background: rgba(160, 196, 242, .9); }
button:focus-visible { outline: 2px solid #2a78d6; outline-offset: -2px; }
.go img { width: 22px; height: 22px; flex: none; filter: drop-shadow(0 1px 1px rgba(0, 0, 0, .25)); }
.more svg { width: 12px; height: 12px; color: #3b5876; }
.compact .label { display: none; }
.compact .go { padding: 0 6px; border-start-start-radius: 8px; border-end-start-radius: 8px; }
.menu {
  position: absolute; min-width: 210px; padding: 4px; border-radius: 9px;
  background: #fbfcfe; color: #17324d; border: 1px solid rgba(23, 50, 77, .22);
  box-shadow: 0 12px 32px rgba(0, 0, 0, .35);
  font: 400 13px/1.2 "Segoe UI", system-ui, -apple-system, Roboto, sans-serif;
  display: none;
}
.menu.open { display: block; }
.menu button { display: block; width: 100%; padding: 8px 12px; border-radius: 6px; cursor: default; }
.menu button:hover, .menu button:focus-visible { background: #dceafb; outline: none; }
.menu hr { border: 0; border-top: 1px solid rgba(23, 50, 77, .12); margin: 4px 2px; }
/* The qualities: a row each (the app's name for it, its size at the end), a message while it
   looks or when it found none, "Open in…" last. Scrolls when there are many. */
.picks { min-width: 240px; max-width: 360px; max-height: min(300px, 45vh); overflow: auto; overscroll-behavior: contain; }
.picks:focus { outline: none; }
.menu .pick { display: flex; align-items: baseline; gap: 16px; }
.pick .name { overflow-wrap: anywhere; }
.pick .size { margin-inline-start: auto; color: #4a6681; font-variant-numeric: tabular-nums; white-space: nowrap; }
/* Video / Audio tabs over the rows, when the video has both. */
.tabs { display: flex; gap: 4px; padding: 0 0 4px; }
.tabs[hidden] { display: none; }
.menu .tab { flex: 1 1 0; width: auto; text-align: center; font-weight: 600; color: #3b5876; border-radius: 6px 6px 0 0; border-bottom: 2px solid transparent; }
.menu .tab[aria-selected="true"] { color: #17324d; border-bottom-color: #2a78d6; }
.note { padding: 8px 12px; color: #3b5876; overflow-wrap: anywhere; }
.note .why { display: block; margin-top: 4px; font-size: 12px; color: #5a7086; }
@media (prefers-reduced-motion: reduce) { .pill { transition: none; transform: none; } }
`;
  // A constructed sheet: a page whose Content-Security-Policy refuses inline styles can't strip
  // it (Firefox applies the page's policy to what a content script adds).
  try {
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(CSS);
    shadow.adoptedStyleSheets = [sheet];
  } catch {
    const style = document.createElement("style");
    style.textContent = CSS;
    shadow.appendChild(style);
  }
  shadow.innerHTML += `
<div class="pill" part="pill">
  <button class="go" type="button">
    <img alt="" draggable="false">
    <span class="label"></span>
  </button>
  <button class="more" type="button" aria-haspopup="menu" aria-expanded="false">
    <svg viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.6"
      stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M3 4.5 6 7.5l3-3"/></svg>
  </button>
</div>
<div class="menu" role="menu">
  <button role="menuitem" data-act="download"></button>
  <hr>
  <button role="menuitem" data-act="hide"></button>
  <button role="menuitem" data-act="never"></button>
  <hr>
  <button role="menuitem" data-act="settings"></button>
</div>
<div class="menu picks" role="group" tabindex="-1" aria-describedby="said">
  <div class="note" id="said" role="status"></div>
  <div class="tabs" role="tablist" hidden>
    <button type="button" class="tab" role="tab" id="tab-video" aria-controls="qualities" data-tab="video"></button>
    <button type="button" class="tab" role="tab" id="tab-audio" aria-controls="qualities" data-tab="audio"></button>
  </div>
  <div class="qualities" id="qualities" role="none">
    <div class="rows" role="menu"></div>
  </div>
  <hr>
  <button data-act="window"></button>
</div>`;
  shadow.querySelector(".go img").src = LOGO;
  const pill = shadow.querySelector(".pill");
  const go = shadow.querySelector(".go");
  const label = shadow.querySelector(".label");
  const more = shadow.querySelector(".more");
  const menu = shadow.querySelector(".menu");
  const item = (act) => menu.querySelector(`[data-act="${act}"]`);
  // The qualities' menu: its message, its rows, "Open in…".
  const picks = shadow.querySelector(".picks");
  const note = picks.querySelector(".note");
  const rows = picks.querySelector(".rows");
  const tabs = picks.querySelector(".tabs");
  const tabList = [...tabs.querySelectorAll("[data-tab]")];
  const qualities = picks.querySelector(".qualities");
  const openRow = picks.querySelector('[data-act="window"]');
  // Which of the two menus is open (`menuOpen`).
  let panel = menu;
  pill.dir = ext.i18n.getMessage("@@bidi_dir") || "ltr";
  menu.dir = pill.dir;
  picks.dir = pill.dir;
  const idleLabel = t("video_download");
  label.textContent = idleLabel;
  go.title = `${idleLabel} — ${t("video_drag")}`;
  go.setAttribute("aria-label", idleLabel);
  more.title = t("video_more");
  more.setAttribute("aria-label", t("video_more"));
  item("download").textContent = idleLabel;
  item("hide").textContent = t("video_hide_here");
  item("never").textContent = ext.i18n.getMessage("video_never_site", [host]) || `Never on ${host}`;
  item("settings").textContent = t("video_settings");
  picks.setAttribute("aria-label", idleLabel);
  rows.setAttribute("aria-label", idleLabel);
  tabs.setAttribute("aria-label", idleLabel);
  tabs.dir = pill.dir;
  for (const b of tabList) b.textContent = t(b.dataset.tab === "audio" ? "video_tab_audio" : "video_tab_video");
  openRow.textContent = t("video_open_mdm");

  // ---- which video ----------------------------------------------------------------------------
  const videos = document.getElementsByTagName("video");
  const audios = document.getElementsByTagName("audio");

  // Videos inside open shadow roots (players built as web components), looked for again every
  // few seconds at most.
  let shadowed = [];
  let shadowedAt = 0;
  function shadowVideos() {
    if (Date.now() - shadowedAt < 3000) return shadowed;
    shadowedAt = Date.now();
    const found = [];
    const walk = (root, depth) => {
      if (depth > 4) return;
      const w = document.createTreeWalker(root, NodeFilter.SHOW_ELEMENT);
      for (let n = w.nextNode(), seen = 0; n && seen < 20000; n = w.nextNode(), seen++) {
        if (n.shadowRoot) {
          found.push(...n.shadowRoot.querySelectorAll("video"));
          walk(n.shadowRoot, depth + 1);
        }
      }
    };
    walk(document.documentElement, 0);
    shadowed = found;
    return found;
  }

  function eligible(v) {
    if (!v.isConnected || isDismissed(v)) return false;
    const r = v.getBoundingClientRect();
    const audio = v instanceof HTMLAudioElement;
    if (audio ? !v.controls || r.width < MIN_W || r.height < MIN_AUDIO_H : r.width < MIN_W || r.height < MIN_H) {
      return false;
    }
    // A short muted loop without controls: a moving picture, not something to save.
    if (v.muted && v.loop && !v.controls && !(v.duration > CLIP_S)) return false;
    const s = getComputedStyle(v);
    return s.visibility !== "hidden" && s.display !== "none" && Number(s.opacity) > 0.05;
  }

  // The topmost eligible video under (x, y): players lay their controls over the video, so the
  // video itself rarely gets the pointer's events.
  function videoAt(x, y) {
    let best = null;
    for (const v of [...videos, ...audios, ...shadowVideos()]) {
      const r = v.getBoundingClientRect();
      if (x < r.left || x > r.right || y < r.top || y > r.bottom || !eligible(v)) continue;
      if (!best || r.width * r.height < best.r.width * best.r.height) best = { v, r };
    }
    return best && best.v;
  }

  // ---- placing it -------------------------------------------------------------------------
  // `place` is where it sits in the room the video leaves around it: x and y from 0 to 1, one of
  // them 0 or 1 (on an edge).
  function rects() {
    const r = held ? held.r : (lastRect = video.getBoundingClientRect());
    const w = pill.offsetWidth || 150;
    const h = pill.offsetHeight || 32;
    return { r, w, h, roomX: Math.max(0, r.width - w - 2 * INSET), roomY: Math.max(0, r.height - h - 2 * INSET) };
  }

  // Is a control of the page under this spot (a viewport rectangle)? What counts is something
  // on top of the video that the user clicks and that is much smaller than the video: a button,
  // a small link, a watch-later or menu button on a preview (#421); a link or layer over the
  // whole video isn't one. On the video itself, and below it, is nothing to cover.
  function onControl(b, video, videoRect) {
    const area = (x) => Math.max(0, x.width) * Math.max(0, x.height);
    const small = (el) => area(el.getBoundingClientRect()) < area(videoRect) / 5;
    const points = [
      [(b.left + b.right) / 2, (b.top + b.bottom) / 2],
      [b.left + 4, b.top + 4],
      [b.right - 4, b.top + 4],
      [b.left + 4, b.bottom - 4],
      [b.right - 4, b.bottom - 4],
    ];
    for (const [x, y] of points) {
      for (const el of document.elementsFromPoint(x, y)) {
        if (el === root) continue;
        if (el === video || el.contains(video)) break;
        const control = el.closest("button, [role=button], a[href], input, select, [onclick]");
        if (control && !control.contains(video) && small(control)) return true;
        if (small(el) && getComputedStyle(el).cursor === "pointer") return true;
      }
    }
    return false;
  }

  // Where it goes now: the place the user gave it, else the default corner, or the next corner
  // free of the page's own controls (a thumbnail's watch-later button at the top right, #421).
  // Over the same video it only ever goes on to the next corner, never back to one it left: a
  // hover-to-play preview (YouTube's thumbnails, #560) shows its sound and captions buttons while
  // the pointer is over the video and hides them as the pointer reaches the button, so going back
  // when they hide, then away again when they show, kept the button jumping from under the pointer.
  const CORNERS = [{ x: 0, y: 0 }, { x: 0, y: 1 }, { x: 1, y: 1 }];
  function spot(r, w, h, roomX, roomY) {
    if (chosen) return place;
    // Still while its menu is open: the player's own controls come and go as the pointer moves
    // over the video, and the menu's rows would move away from under it.
    if (menuOpen && auto.v === video && auto.at) return auto.at;
    const now = Date.now();
    const same = auto.v === video;
    if (same && now - auto.t < AUTO_MS) return auto.at;
    const order = [place, ...CORNERS];
    const from = same ? Math.max(0, order.indexOf(auto.at)) : 0;
    let at = same ? auto.at : place;
    for (const c of order.slice(from)) {
      const left = r.left + INSET + c.x * roomX;
      const top = r.top + INSET + c.y * roomY;
      if (!onControl({ left, top, right: left + w, bottom: top + h }, video, r)) {
        at = c;
        break;
      }
    }
    auto = { v: video, at, t: now };
    return at;
  }

  function position() {
    if (!video) return;
    const { r, w, roomX, roomY } = rects();
    // The pointer on it (or its menu open): it stays where it is, whatever the video does
    // meanwhile. A preview that grows while the pointer is over it and shrinks back when the
    // pointer goes (#560) would otherwise take the button from under the pointer.
    if (!dragging && pinned && pinned.v === video && (menuOpen || root.matches(":hover"))) {
      if (menuOpen) placeMenu(pinned.top, pinned.left, w);
      return;
    }
    // A narrow video gets the icon only.
    pill.classList.toggle("compact", r.width < 360);
    // Held for a video gone (#503): where it was, not a corner found anew over what the page
    // shows there now.
    const at = dragging ? dragging.at : held && !chosen ? held.at : spot(r, w, pill.offsetHeight || 32, roomX, roomY);
    const left = r.left + INSET + at.x * roomX;
    // In the part of the video on screen: one scrolled half out keeps its button in view.
    const h = pill.offsetHeight || 32;
    const visTop = Math.max(r.top, 0) + INSET;
    const visBottom = Math.min(r.bottom, innerHeight) - INSET - h;
    const top = Math.min(Math.max(r.top + INSET + at.y * roomY, visTop), Math.max(visTop, visBottom));
    root.style.setProperty("transform", `translate(${Math.round(left)}px, ${Math.round(top)}px)`, "important");
    pinned = { v: video, left, top };
    if (menuOpen) placeMenu(top, left, w);
  }

  // The open menu opens below the button, or above it near the bottom of the window; aligned
  // with whichever of its sides is nearer the video's middle.
  function placeMenu(top, left, w) {
    const h = panel.offsetHeight;
    const below = top + 36 + h < innerHeight;
    panel.style.top = below ? "36px" : `${-h - 4}px`;
    const { r } = rects();
    const rightHalf = left + w / 2 > r.left + r.width / 2;
    panel.style.left = rightHalf ? "" : "0";
    panel.style.right = rightHalf ? "0" : "";
  }

  // Snap a free spot (fractions, after a drag) to the nearest edge.
  function snap(at) {
    const d = { left: at.x, right: 1 - at.x, top: at.y, bottom: 1 - at.y };
    const edge = Object.keys(d).reduce((a, b) => (d[a] <= d[b] ? a : b));
    // Along the edge it stays where it was dropped, but close to a corner it goes into it.
    const c = (n) => (n < 0.08 ? 0 : n > 0.92 ? 1 : n);
    if (edge === "left") return { x: 0, y: c(at.y) };
    if (edge === "right") return { x: 1, y: c(at.y) };
    if (edge === "top") return { x: c(at.x), y: 0 };
    return { x: c(at.x), y: 1 };
  }

  // ---- showing and hiding ---------------------------------------------------------------------
  function attach() {
    // Over the video in fullscreen: inside the fullscreen element (a fullscreen <video> itself
    // can't hold it: then it hides).
    const fs = document.fullscreenElement;
    const parent = fs && fs !== video && fs.contains(video) ? fs : document.documentElement;
    if (root.parentNode !== parent) parent.appendChild(root);
    return !(fs && (fs === video || !fs.contains(video)));
  }

  function show(v) {
    if (video !== v) {
      video = v;
      held = null;
      closeMenu();
    }
    if (!attach()) return hide(true);
    root.style.setProperty("display", "block", "important");
    position();
    if (!pill.classList.contains("shown")) requestAnimationFrame(() => pill.classList.add("shown"));
    if (!raf) raf = requestAnimationFrame(tick);
  }

  // `now`: at once and whatever has the focus (dismissed, the video gone).
  function hide(now) {
    if (!now && (menuOpen || dragging || keyboardOn())) return;
    pinned = null;
    if (now) {
      held = null;
      closeMenu();
      shadow.activeElement?.blur();
    }
    pill.classList.remove("shown");
    cancelAnimationFrame(raf);
    raf = 0;
    const was = video;
    setTimeout(() => {
      if (!pill.classList.contains("shown") && video === was) root.style.setProperty("display", "none", "important");
    }, now || reduced ? 0 : 180);
  }

  // What the button sends for `v` (its rectangle `r`): see `download`.
  function describe(v, r) {
    const url = videoPage(v);
    // Its own plain file (not a stream the page assembles, blob:). Not beside a post a big site's
    // reader knows (#689): the app would take the file over the post, its title and qualities.
    const file = /^https?:/i.test(v.currentSrc || "") ? v.currentSrc : "";
    const post = Boolean(url) && url !== location.href && SITES.some((s) => s.hosts.test(siteName(host)));
    return {
      drm: Boolean(v.mediaKeys),
      src: post ? "" : file,
      url,
      // The middle of the video: where the background asks the page's own player what it plays.
      at: { x: r.left + r.width / 2, y: r.top + r.height / 2 },
    };
  }

  // The video just went from under the button (#503): what it was, kept. What was taken as the
  // pointer reached the button, when that was a moment ago; else the video as it is now.
  function hold() {
    const now = Date.now();
    const d = entered && entered.v === video && now - entered.t < 1000 ? entered.d : describe(video, lastRect);
    return { r: lastRect, at: chosen ? place : (auto.v === video && auto.at) || place, d, since: now };
  }

  // While shown: follow the video (scrolling, layout) and fade when the pointer rests.
  function tick() {
    raf = 0;
    if (!video) return hide(true);
    const onButton = root.matches(":hover");
    if (!video.isConnected || !eligible(video)) {
      if (!held && lastRect && !isDismissed(video)) held = hold();
      const r = held && held.r;
      const over =
        r && pointer.x >= r.left && pointer.x <= r.right && pointer.y >= r.top && pointer.y <= r.bottom;
      if (!held || !(onButton || menuOpen || dragging || keyboardOn() || (over && Date.now() - held.since < HOLD_MS))) {
        const was = held;
        held = null;
        video = null;
        // Left after it was held: it fades, as when the pointer leaves a video.
        return was ? hide() : hide(true);
      }
      position();
      raf = requestAnimationFrame(tick);
      return;
    }
    held = null;
    const r = video.getBoundingClientRect();
    const over =
      pointer.x >= r.left && pointer.x <= r.right && pointer.y >= r.top && pointer.y <= r.bottom;
    // The qualities' menu for a video this element no longer plays, or a page left: it goes.
    if (menuOpen && panel === picks && !(job && current(job))) closeMenu();
    if (!onButton && !menuOpen && !dragging && !keyboardOn() && (!over || Date.now() - lastMove > IDLE_MS)) {
      return hide();
    }
    position();
    raf = requestAnimationFrame(tick);
  }

  // At most one look per frame, however fast the pointer moves.
  function onMove(e) {
    pointer = { x: e.clientX, y: e.clientY };
    lastMove = Date.now();
    if (!scan) scan = requestAnimationFrame(look);
  }

  function look() {
    scan = 0;
    if (dragging || never || (allowed === false && answered())) return;
    // Kept on its video while the qualities' menu is open (pointerTarget).
    const v = pointerTarget(videoAt(pointer.x, pointer.y), video, menuOpen && panel === picks);
    if (!v) return;
    if (!answered()) return void ask().then(() => allowed && look());
    show(v);
  }

  // One question at a time; while it is out, the button waits for its answer.
  function ask() {
    if (never) {
      allowed = false;
      return Promise.resolve();
    }
    if (answered()) return Promise.resolve();
    asking = asking || send({ type: "panel" }).then(async (r) => {
      allowed = !never && Boolean(r && r.on);
      askedAt = Date.now();
      // Switched off meanwhile: gone now, unless the menu or a drag has it in hand.
      if (!allowed && video && !menuOpen && !dragging) {
        video = null;
        hide(true);
      }
      if (allowed && !placed) {
        placed = true;
        try {
          const { positions } = await ext.storage.local.get({ positions: {} });
          if (positions[host]) {
            place = snap(positions[host]);
            chosen = true;
          }
        } catch {
          // Default spot.
        }
      }
    }).finally(() => (asking = null));
    return asking;
  }

  // ---- acting ---------------------------------------------------------------------------------
  // The middle of the video: where the background asks the page's own player what it plays.
  function middle(v) {
    const box = v.getBoundingClientRect();
    return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
  }

  // A promise's answer, or {late: true} once `ms` is up.
  const within = (promise, ms) => {
    let timer = 0;
    const late = new Promise((done) => (timer = setTimeout(() => done({ late: true }), Math.max(0, ms))));
    return Promise.race([promise, late]).finally(() => clearTimeout(timer));
  };

  // The click: the app looks the video up and its qualities open in a menu here. An app that can't
  // (an older one, another MDM chosen) gets the click as it always was: its Download video window.
  async function download() {
    // The button again while its qualities are open: they close.
    if (menuOpen && panel === picks) return closeMenu();
    if (busy || !video) return;
    busy = true;
    try {
      closeMenu();
      // Asked now, not kept: one <video> plays many videos at the same address (a feed's
      // previews). A video gone from under the button: what it played then (#503).
      const { drm, src, url, at } = held ? held.d : describe(video, video.getBoundingClientRect());
      // Encrypted (DRM): no tool can save it; say so rather than send it.
      if (drm) return flash("err", t("video_drm"));
      // A page that moved in place keeps its first page's og:title; its document title follows.
      const og = location.href === landedAt ? document.querySelector('meta[property="og:title"]')?.content : "";
      const title = (og || document.title || "").trim();
      const j = { video, src: video.currentSrc || "", href: location.href, asked: { src, title, url, at }, started: Date.now(), token: "", count: 0 };
      job = j;
      // The menu opens at once, saying it is looking; an answer that needs none and comes first
      // (no lookups here: the window) shows none.
      const opening = setTimeout(() => openPicks(j), 100);
      const s = replyState(await within(send({ type: "video-lookup", ...j.asked }), LOOKUP_MS));
      clearTimeout(opening);
      if (!s || !current(j)) return;
      const a = lookupAction(s, t);
      // No lookups here, or an answer that isn't one: the click as it was, once.
      if (a.act === "window") return await useWindow(j);
      // A video it can't tell, or the button off here: a word on the button, no menu.
      if (a.act === "flash") {
        closeMenu();
        dropLookup();
        return flash(a.kind, a.text);
      }
      openPicks(j);
      follow(j, s);
    } finally {
      busy = false;
    }
  }

  // Still the lookup the menu is for: the menu open on the same video (the same element playing
  // the same thing) on the same page. Whatever answers a lookup after that is dropped.
  const current = (j) =>
    job === j && video === j.video && (!j.src || video.currentSrc === j.src) && location.href === j.href;

  function openPicks(j) {
    if (!current(j) || (menuOpen && panel === picks)) return;
    closeMenu();
    panel = picks;
    menuOpen = true;
    clearRows();
    said(t("video_looking"));
    picks.setAttribute("aria-busy", "true");
    picks.classList.add("open");
    go.setAttribute("aria-expanded", "true");
    lastMove = Date.now();
    position();
    picks.focus({ preventScroll: true });
  }

  // No rows, no tabs.
  function clearRows() {
    rows.replaceChildren();
    tabs.hidden = true;
    qualities.setAttribute("role", "none");
    qualities.removeAttribute("aria-labelledby");
  }

  // The menu's message (plain text), with the app's own words under it.
  function said(text, detail) {
    note.textContent = text;
    if (detail) {
      const why = document.createElement("span");
      why.className = "why";
      why.textContent = String(detail).slice(0, 400);
      note.append(why);
    }
    note.hidden = !text;
  }

  // The keyboard's focus goes to the menu's new rows while it is in the menu or on the button
  // (not when it was taken elsewhere meanwhile).
  const focusHere = () => {
    const a = shadow.activeElement;
    return Boolean(a) && (a === go || picks.contains(a));
  };

  // A message instead of qualities, "Open in…" under it.
  function say(j, text, detail) {
    if (!current(j)) return;
    picks.setAttribute("aria-busy", "false");
    clearRows();
    said(text, detail);
    position();
    if (focusHere()) openRow.focus();
  }

  // The lookup's end, in the menu (lookupAction).
  function settle(j, s) {
    if (!current(j)) return;
    const a = lookupAction(s, t);
    if (a.act === "offer") return offer(j, s);
    if (a.act === "say") return say(j, a.text, a.detail);
    if (a.act === "flash") {
      closeMenu();
      dropLookup();
      return flash(a.kind, a.text);
    }
    // "use-window", "gone" (the app forgot it: restarted) and anything newer: the window.
    return useWindow(j);
  }

  // The rows for these qualities of the app's list (their numbers), a row each: the app's name
  // for it and its size, as text. The row keeps the quality's number in the app's list.
  function showRows(s, numbers) {
    rows.replaceChildren(
      ...numbers.map((i) => {
        const { name, size } = rowText(s.choices[i], lang);
        const b = document.createElement("button");
        b.type = "button";
        b.className = "pick";
        b.setAttribute("role", "menuitem");
        b.dataset.choice = String(i);
        const n = document.createElement("span");
        n.className = "name";
        n.textContent = name;
        b.append(n);
        if (size) {
          const z = document.createElement("span");
          z.className = "size";
          z.textContent = size;
          b.append(z);
        }
        return b;
      }),
    );
  }

  // One of the two tabs shown: it is the selected one (the only one in the tab order), its
  // qualities are the rows.
  function showTab(j, s, kind) {
    for (const b of tabList) {
      const on = b.dataset.tab === kind;
      b.setAttribute("aria-selected", String(on));
      b.tabIndex = on ? 0 : -1;
    }
    qualities.setAttribute("aria-labelledby", `tab-${kind}`);
    j.tab = kind;
    showRows(s, j.groups[kind]);
    position();
  }

  // The qualities: in one list, or in the Video and Audio tabs when there are both.
  function offer(j, s) {
    j.count = Math.min(s.choices.length, 100);
    j.token = s.token;
    j.groups = groupChoices(s.choices, j.count);
    j.s = s;
    if (j.groups.tabbed) {
      tabs.hidden = false;
      qualities.setAttribute("role", "tabpanel");
      showTab(j, s, j.groups.start);
    } else {
      clearRows();
      showRows(s, j.groups.all);
    }
    said("");
    picks.setAttribute("aria-busy", "false");
    // Rows that just pushed "Open in…" down under the pointer aren't picked by a click meant for
    // it.
    j.armed = Date.now() + 300;
    position();
    if (focusHere()) (rows.firstElementChild || openRow).focus();
  }

  // Asks how the lookup is going until it ends (followLookup), then shows what it came to.
  async function follow(j, first) {
    const s = await followLookup(first, {
      ask: (token, ms) => within(send({ type: "video-poll", token }), ms),
      alive: () => current(j),
      sleep: (ms) =>
        new Promise((wake) => {
          j.wake = wake;
          j.timer = setTimeout(wake, ms);
        }),
      now: Date.now,
      end: j.started + LOOKUP_MS,
    });
    if (!s || !current(j)) return;
    settle(j, s);
  }

  // The lookup is no longer wanted (its menu closed): no more questions about it.
  function dropLookup() {
    const j = job;
    job = null;
    if (!j) return;
    clearTimeout(j.timer);
    if (j.wake) j.wake();
  }

  // A quality picked: the app downloads it (its own choice of it, with what it looked the video
  // up with) without its window. Only the token and the row's number go.
  async function pick(j, i) {
    if (!current(j) || !j.count || j.picked || !(i >= 0 && i < j.count) || Date.now() < j.armed) return;
    j.picked = true;
    closeMenu();
    busy = true;
    try {
      const s = replyState(await send({ type: "video-pick", token: j.token, choice: i }));
      if (!s) return;
      // Downloading, queued, or handed to the app ("sent"): a word on the button. Anything else
      // (the lookup forgotten or another window's, an older app): the window.
      const a = lookupAction(s, t, true);
      if (a.act === "flash") return flash(a.kind, a.text);
      return await useWindow(j);
    } finally {
      busy = false;
    }
  }

  // The click as it always was: the app's Download video window for this video. Once per click.
  async function useWindow(j) {
    if (job === j) {
      closeMenu();
      dropLookup();
    }
    if (!firstWindow(j)) return;
    // Where the video is now, for the background to ask the page's player there.
    const r = await send({ type: "video", ...j.asked, ...(j.video.isConnected ? { at: middle(j.video) } : {}) });
    if (r.gone) return;
    const why = { options: "video_off", unnamed: "video_unnamed", feed: "video_open_post" }[r.why];
    flash(r.ok ? "ok" : "err", t(r.ok ? "video_sent" : why || "video_failed"));
  }

  // A row of the qualities' menu, clicked or by key.
  function choosePick(b) {
    const j = job;
    if (!j || !b) return;
    if (b.dataset.act === "window") return useWindow(j);
    // A tab: its qualities are the rows now (the keyboard stays on the tab).
    if (b.dataset.tab !== undefined) {
      if (j.groups && j.groups.tabbed && j.groups[b.dataset.tab] && j.s) showTab(j, j.s, b.dataset.tab);
      return;
    }
    if (b.dataset.choice !== undefined) return pick(j, Number(b.dataset.choice));
  }

  // A short confirmation on the button itself, then back to normal.
  function flash(kind, text) {
    flashing = true;
    pill.classList.remove("ok", "err");
    pill.classList.add(kind);
    label.textContent = text;
    lastMove = Date.now();
    position();
    setTimeout(() => {
      flashing = false;
      pill.classList.remove(kind);
      label.textContent = idleLabel;
      if (video) position();
    }, 2200);
  }

  function openMenu() {
    closeMenu();
    panel = menu;
    menuOpen = true;
    menu.classList.add("open");
    more.setAttribute("aria-expanded", "true");
    position();
    menu.querySelector("button").focus();
  }

  // Closes whichever menu is open; the qualities' takes its lookup with it.
  function closeMenu() {
    if (!menuOpen) return;
    const back = panel === picks && picks.contains(shadow.activeElement);
    menuOpen = false;
    panel.classList.remove("open");
    more.setAttribute("aria-expanded", "false");
    if (panel !== picks) return;
    go.setAttribute("aria-expanded", "false");
    picks.setAttribute("aria-busy", "false");
    dropLookup();
    // The keyboard's focus back on the button, not lost with the menu.
    if (back) go.focus();
  }

  menu.addEventListener("click", (e) => {
    if (!e.isTrusted) return;
    const act = e.target.closest("[data-act]")?.dataset.act;
    if (act) run(act);
  });

  async function run(act) {
    if (act === "download") return download();
    closeMenu();
    if (act === "hide") {
      held = null;
      dismissed.add(video);
      if (video.currentSrc) dismissedSrc.add(video.currentSrc);
      video = null;
      return hide(true);
    }
    if (act === "never") {
      never = true;
      allowed = false;
      video = null;
      hide(true);
      return send({ type: "hide-site" });
    }
    if (act === "settings") send({ type: "options" });
  }

  more.addEventListener("click", () => (menuOpen && panel === menu ? closeMenu() : openMenu()));

  // The qualities' rows: only the user's own clicks.
  picks.addEventListener("click", (e) => {
    if (!e.isTrusted) return;
    choosePick(e.target.closest("button"));
  });

  // The pointer reached the button: what the video plays now, kept in case the page stops it for
  // that (a hover-to-play preview, #503).
  root.addEventListener("pointerenter", () => {
    if (video && !held && video.isConnected && eligible(video)) {
      entered = { v: video, d: describe(video, video.getBoundingClientRect()), t: Date.now() };
    }
  });

  // Keys (only the user's own, never ones a page script makes up): Escape closes the open menu
  // (the keyboard's focus back on its button), arrows move through it, Enter and Space run the
  // focused command. Each stops there: a key the open menu used is not the page's too (a player's
  // Space and arrows would pause it and change its volume behind the menu).
  shadow.addEventListener("keydown", (e) => {
    if (!e.isTrusted) return;
    const focused = shadow.activeElement;
    if (e.key === "Escape" && menuOpen) {
      const opener = panel === picks ? go : more;
      closeMenu();
      opener.focus();
      e.stopPropagation();
    } else if (menuOpen && (e.key === "Enter" || e.key === " ") && focused && focused !== panel && panel.contains(focused)) {
      // The focused command, whatever the browser does with a key on a button.
      if (panel === picks) choosePick(focused);
      else run(focused.dataset.act);
      e.preventDefault();
      e.stopPropagation();
    } else if (menuOpen && panel === picks && !tabs.hidden && focused && focused.dataset.tab !== undefined && ["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) {
      // Between the tabs (not on to the rows); the arrows follow the reading direction.
      const i = tabList.indexOf(focused);
      const step = (e.key === "ArrowRight") === (pill.dir !== "rtl") ? 1 : -1;
      const next = e.key === "Home" ? 0 : e.key === "End" ? tabList.length - 1 : (i + step + tabList.length) % tabList.length;
      tabList[next].focus();
      e.preventDefault();
      e.stopPropagation();
    } else if (menuOpen && panel === picks && !tabs.hidden && focused && (e.key === "ArrowDown" || e.key === "ArrowUp") && (focused.dataset.tab !== undefined || (e.key === "ArrowUp" && focused === rows.firstElementChild))) {
      // From a tab down to the rows; up from the first row to the selected tab.
      if (focused.dataset.tab !== undefined) {
        if (e.key === "ArrowDown") (rows.firstElementChild || openRow).focus();
      } else {
        (tabList.find((b) => b.tabIndex === 0) || tabList[0]).focus();
      }
      e.preventDefault();
      e.stopPropagation();
    } else if (menuOpen && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      // The rows (and "Open in…"), never the tabs.
      const items = [...panel.querySelectorAll("button:not([data-tab])")];
      const i = items.indexOf(focused);
      const down = e.key === "ArrowDown";
      // From the menu itself (the qualities' while it looks): the first, or the last.
      const next = i < 0 ? (down ? 0 : items.length - 1) : (i + (down ? 1 : items.length - 1)) % items.length;
      items[next]?.focus();
      e.preventDefault();
      e.stopPropagation();
    }
  });

  // The main part: a click downloads, a drag moves it (and is not a click).
  // Only the user's own clicks and keys (never events a page script makes up).
  go.addEventListener("pointerdown", (e) => {
    if (e.button !== 0 || !video || !e.isTrusted) return;
    const { r, roomX, roomY } = rects();
    dragging = {
      id: e.pointerId,
      startX: e.clientX,
      startY: e.clientY,
      // Where in the button it was taken.
      dx: e.clientX - (r.left + INSET + (chosen ? place : auto.at || place).x * roomX),
      dy: e.clientY - (r.top + INSET + (chosen ? place : auto.at || place).y * roomY),
      at: chosen ? place : auto.at || place,
      moved: false,
    };
    go.setPointerCapture(e.pointerId);
    e.preventDefault();
  });
  go.addEventListener("pointermove", (e) => {
    if (!dragging || e.pointerId !== dragging.id) return;
    if (!dragging.moved && Math.hypot(e.clientX - dragging.startX, e.clientY - dragging.startY) < DRAG_PX) return;
    if (!dragging.moved) {
      dragging.moved = true;
      closeMenu();
      pill.classList.add("dragging");
    }
    const { r, roomX, roomY } = rects();
    const c = (n) => Math.min(1, Math.max(0, n));
    dragging.at = {
      x: roomX ? c((e.clientX - dragging.dx - r.left - INSET) / roomX) : 0,
      y: roomY ? c((e.clientY - dragging.dy - r.top - INSET) / roomY) : 0,
    };
    position();
  });
  const endDrag = (e) => {
    if (!dragging || e.pointerId !== dragging.id) return;
    const d = dragging;
    dragging = null;
    pill.classList.remove("dragging");
    if (!d.moved) {
      if (e.type === "pointerup") download();
      return;
    }
    place = snap(d.at);
    chosen = true;
    // It snaps to its corner although the pointer is still on it (#560's pin is for a video
    // that changes under a still pointer, not for the user's own move).
    pinned = null;
    position();
    lastMove = Date.now();
    // Remembered per site, not in a private window (it would keep the site's name), and the
    // 200 sites used last only.
    if (ext.extension?.inIncognitoContext) return;
    try {
      ext.storage.local
        .get({ positions: {} })
        .then(({ positions }) => {
          delete positions[host];
          positions[host] = place;
          const sites = Object.keys(positions);
          for (const s of sites.slice(0, Math.max(0, sites.length - 200))) delete positions[s];
          return ext.storage.local.set({ positions });
        })
        .catch(() => {});
    } catch {
      teardown();
    }
  };
  go.addEventListener("pointerup", endDrag);
  go.addEventListener("pointercancel", endDrag);
  // Keyboard: Enter / Space (a real button's click without a pointer).
  go.addEventListener("click", (e) => {
    if (e.detail === 0 && e.isTrusted) download();
  });

  // ---- wiring -----------------------------------------------------------------------------------
  // The extension updated or removed under this page: the button and its listeners go (a fresh
  // copy of this script comes with the new extension).
  function teardown() {
    dropLookup();
    cancelAnimationFrame(raf);
    cancelAnimationFrame(scan);
    removeEventListener("pointermove", onMove, true);
    removeEventListener("keydown", onTab, true);
    root.remove();
    never = true;
    allowed = false;
    video = null;
  }
  globalThis.__mdmVideoButton = { alive: () => Boolean(ext.runtime?.id), teardown };
  addEventListener("pointermove", onMove, { capture: true, passive: true });
  addEventListener("pointerdown", (e) => {
    if (menuOpen && !e.composedPath().includes(root)) closeMenu();
  }, { capture: true, passive: true });
  // A video with the keyboard's focus, or a player around one (its controls), gets the button
  // too, kept while the focus stays there; Tab goes from there to the button.
  addEventListener("focusin", (e) => {
    if (!(e.target instanceof Element) || e.composedPath().includes(root)) return;
    const t = e.target;
    const v =
      t instanceof HTMLMediaElement ? t : [...t.querySelectorAll("video, audio")].find(eligible) || null;
    if (!v || !eligible(v)) return;
    focusedPlayer = t;
    ask().then(() => {
      if (!allowed) return;
      lastMove = Date.now();
      show(v);
    });
  }, true);
  // Tab on the focused player moves into the button first; Tab from the button goes back to
  // the player and on from there (its own controls next), as if the button sat at its start.
  let tabOn = 0;
  function onTab(e) {
    if (e.key !== "Tab" || e.shiftKey) return;
    // The Tab that just left the button: on from the player, as the browser goes.
    if (Date.now() - tabOn < 100) {
      tabOn = 0;
      return;
    }
    if (video && playerFocused() && root.style.getPropertyValue("display") === "block") {
      go.focus();
      e.preventDefault();
    }
  }
  addEventListener("keydown", onTab, true);
  go.addEventListener("keydown", (e) => {
    if (e.key === "Tab" && !e.shiftKey && focusedPlayer && focusedPlayer.isConnected) {
      tabOn = Date.now();
      focusedPlayer.focus();
    }
  });
  document.addEventListener("fullscreenchange", () => video && show(video));
  addEventListener("blur", () => hide());
})();
