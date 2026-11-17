// Mister Download Manager's remote control page (#141). Served by MDM itself; no other site is
// reached. Every text shown goes through T() (the app's translations, /api/texts): the English
// texts here are the catalog's keys (crates/app/src/remote.rs lists them for the catalog).
'use strict';

const $ = (id) => document.getElementById(id);
let TX = {};
let LANG = 'en';
let token = null;
let filter = 'all';
let data = null;
let timer = null;
let failures = 0;
let chosenFile = null;
const rows = new Map();

try { token = localStorage.getItem('mdm-token'); } catch (e) { token = null; }

function T(s) { return TX[s] || s; }
function F(s, args) { return T(s).replace(/\{(\w+)\}/g, (m, k) => (k in args ? String(args[k]) : m)); }
function unit(tpl, n) { return (TX['unit:' + tpl] || tpl).replace('{n}', n); }
function num(n, digits) {
  try {
    return new Intl.NumberFormat(LANG, { maximumFractionDigits: digits, minimumFractionDigits: digits, numberingSystem: 'latn' }).format(n);
  } catch (e) { return n.toFixed(digits); }
}
function size(n) {
  if (n == null) return '?';
  if (n < 1048576) return unit('{n} KB', num(n / 1024, n < 10240 ? 1 : 0));
  if (n < 1073741824) return unit('{n} MB', num(n / 1048576, 1));
  return unit('{n} GB', num(n / 1073741824, 2));
}
function speed(n) {
  if (n < 1048576) return unit('{n} KB/sec', num(n / 1024, 1));
  return unit('{n} MB/sec', num(n / 1048576, 1));
}

async function loadTexts() {
  const langs = (navigator.languages && navigator.languages.length ? navigator.languages : [navigator.language || 'en']).join(',');
  try {
    const r = await fetch('api/texts?lang=' + encodeURIComponent(langs), { cache: 'no-store' });
    const j = await r.json();
    TX = j.texts || {};
    LANG = j.lang || 'en';
    document.documentElement.lang = LANG;
    document.documentElement.dir = j.rtl ? 'rtl' : 'ltr';
  } catch (e) { /* English, as written */ }
  for (const el of document.querySelectorAll('[data-t]')) el.textContent = T(el.dataset.t);
  for (const el of document.querySelectorAll('[data-ta]')) { el.setAttribute('aria-label', T(el.dataset.ta)); el.title = T(el.dataset.ta); }
  for (const el of document.querySelectorAll('[data-tp]')) el.placeholder = T(el.dataset.tp);
}

class ApiError extends Error {
  constructor(code, body) { super(code); this.code = code; this.body = body; }
}

async function api(path, opts) {
  opts = opts || {};
  const headers = Object.assign({}, opts.headers || {});
  if (token) headers.Authorization = 'Bearer ' + token;
  let body = opts.body;
  if (opts.json !== undefined) { headers['Content-Type'] = 'application/json'; body = JSON.stringify(opts.json); }
  const r = await fetch('api/' + path, { method: body !== undefined || opts.post ? 'POST' : 'GET', headers, body, cache: 'no-store', credentials: 'omit' });
  let j = null;
  try { j = await r.json(); } catch (e) { j = null; }
  if (r.status === 401) { signedOut(); throw new ApiError('not-paired', j); }
  if (!r.ok || (j && j.error)) throw new ApiError((j && j.code) || String(r.status), j);
  return j;
}

let toastTimer = null;
function toast(text, bad) {
  const t = $('toast');
  t.textContent = text;
  t.classList.toggle('bad', !!bad);
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { t.hidden = true; }, bad ? 6000 : 3000);
}

// ---- pairing -----------------------------------------------------------------------------

function guessName() {
  const ua = navigator.userAgent;
  for (const [re, name] of [[/iPhone/, 'iPhone'], [/iPad/, 'iPad'], [/Android/, 'Android'], [/Windows/, 'Windows'], [/Mac OS X|Macintosh/, 'Mac'], [/CrOS/, 'ChromeOS'], [/Linux/, 'Linux']]) {
    if (re.test(ua)) return name;
  }
  return '';
}

function showPair() {
  stopPolling();
  $('main').hidden = true;
  $('menu').hidden = true;
  $('summary').textContent = '';
  $('pair').hidden = false;
  if (!$('name').value) $('name').value = guessName();
  // A code from the QR code MDM shows (#pair=12345678): filled in, never sent anywhere else.
  const m = /^#pair=(\d{8})$/.exec(location.hash);
  if (m) { $('code').value = m[1].slice(0, 4) + ' ' + m[1].slice(4); history.replaceState(null, '', location.pathname); }
  ($('code').value ? $('name') : $('code')).focus();
}

function signedOut() {
  token = null;
  try { localStorage.removeItem('mdm-token'); } catch (e) { /* nothing kept */ }
  showPair();
}

$('pair-form').addEventListener('submit', async (ev) => {
  ev.preventDefault();
  const err = $('pair-error');
  err.textContent = '';
  try {
    const j = await api('pair', { json: { code: $('code').value, name: $('name').value } });
    token = j.token;
    try { localStorage.setItem('mdm-token', token); } catch (e) { /* this visit only */ }
    $('code').value = '';
    showMain();
  } catch (e) {
    if (e.code === 'wrong-code' && e.body && e.body.left > 0) err.textContent = F('Wrong code. Tries left: {n}', { n: e.body.left });
    else if (e.code === 'wrong-code') err.textContent = T('Wrong code. Show a new code in MDM and try again.');
    else if (e.code === 'no-code') err.textContent = T('MDM shows no code now. Click Pair a device there first.');
    else if (e.code === 'too-soon') err.textContent = T('Wait a moment, then try again.');
    else err.textContent = T('MDM does not answer. Check that it is running and on the same network.');
  }
});

// ---- the list ----------------------------------------------------------------------------

function showMain() {
  $('pair').hidden = true;
  $('main').hidden = false;
  $('menu').hidden = false;
  refresh();
}

function stopPolling() { clearTimeout(timer); timer = null; }

function schedule() {
  stopPolling();
  if (document.visibilityState !== 'visible' || !token) return;
  timer = setTimeout(refresh, failures ? Math.min(10000, 1500 * (failures + 1)) : 1500);
}

async function refresh() {
  if (!token) return;
  try {
    data = await api('list');
    failures = 0;
    $('offline').hidden = true;
    render();
  } catch (e) {
    if (e.code === 'not-paired') return;
    failures++;
    $('offline').textContent = T('MDM does not answer. Trying again...');
    $('offline').hidden = false;
  }
  schedule();
}

const STATUS = { Queued: 'Queued', Downloading: 'Downloading', Stopped: 'Stopped', Complete: 'Complete', Error: 'Error' };

function fillSelect(sel, options) {
  const key = JSON.stringify(options);
  if (sel.dataset.key === key) return;
  const was = sel.value;
  sel.textContent = '';
  for (const [value, label] of options) {
    const o = document.createElement('option');
    o.value = value;
    o.textContent = label;
    sel.append(o);
  }
  sel.dataset.key = key;
  if (options.some(([v]) => v === was)) sel.value = was;
}

function render() {
  const d = data;
  const running = d.downloads.filter((x) => x.status === 'Downloading').length;
  const parts = [];
  if (running) parts.push(speed(d.speed));
  if (d.free != null) parts.push(F('{size} free', { size: size(d.free) }));
  $('summary').textContent = parts.join(' · ');

  fillSelect($('category'), [['', T('Automatic (by file type)')]].concat(d.categories.map((c) => [c.key, T(c.name)])));
  fillSelect($('when'), [['', T('Start now')]].concat(d.queues.map((q) => [q.name, F('Wait in {queue}', { queue: T(q.name) })])));

  const chips = $('queues');
  chips.textContent = '';
  for (const q of d.queues) {
    const chip = document.createElement('div');
    chip.className = 'chip' + (q.running ? ' on' : '');
    const label = document.createElement('span');
    label.textContent = T(q.name) + ' (' + q.ids.length + ')';
    const b = document.createElement('button');
    b.type = 'button';
    b.textContent = q.running ? T('Stop') : T('Start');
    b.addEventListener('click', () => queueAction(q.name, !q.running));
    chip.append(label, b);
    chips.append(chip);
  }

  const shown = d.downloads.filter((x) => filter === 'all' || (filter === 'done') === (x.status === 'Complete'));
  const list = $('list');
  const keep = new Set();
  for (const x of shown) {
    keep.add(x.id);
    let li = rows.get(x.id);
    if (!li) {
      li = $('item').content.firstElementChild.cloneNode(true);
      li.querySelector('.toggle').addEventListener('click', () => toggle(li.dataset.id));
      li.querySelector('.remove').addEventListener('click', () => askRemove(li.dataset.id));
      rows.set(x.id, li);
    }
    li.dataset.id = x.id;
    fillRow(li, x);
    list.append(li);
  }
  for (const [id, li] of rows) if (!keep.has(id)) { li.remove(); rows.delete(id); }
  $('empty').hidden = shown.length > 0;
}

function fillRow(li, x) {
  li.querySelector('.name').textContent = x.file;
  const state = li.querySelector('.state');
  state.textContent = T(STATUS[x.status] || x.status);
  state.className = 'state s-' + x.status.toLowerCase();
  const pct = x.size ? Math.min(100, (100 * x.downloaded) / x.size) : (x.status === 'Complete' ? 100 : 0);
  li.querySelector('.bar-fill').style.width = pct + '%';
  li.querySelector('.bar-track').hidden = x.status === 'Complete';
  let detail;
  if (x.status === 'Complete') detail = size(x.size != null ? x.size : x.downloaded);
  else if (x.size) detail = num(pct, 1) + '% · ' + F('{done} of {size}', { done: size(x.downloaded), size: size(x.size) });
  else detail = size(x.downloaded);
  if (x.status === 'Downloading' && x.speed) detail += ' · ' + speed(x.speed);
  li.querySelector('.detail').textContent = detail;
  const toggleBtn = li.querySelector('.toggle');
  toggleBtn.hidden = x.status === 'Complete';
  toggleBtn.textContent = x.status === 'Downloading' ? T('Pause') : T('Start');
  const err = li.querySelector('.error-text');
  err.hidden = !x.error;
  err.textContent = x.error || '';
}

function byId(id) { return data && data.downloads.find((x) => String(x.id) === String(id)); }

async function act(action, ids) {
  const j = await api('act', { json: { action, ids } });
  return j;
}

async function toggle(id) {
  const x = byId(id);
  if (!x) return;
  try {
    await act(x.status === 'Downloading' ? 'pause' : 'start', [x.id]);
  } catch (e) { toast(T('MDM could not do it.'), true); }
  refresh();
}

let removing = null;
function askRemove(id) {
  const x = byId(id);
  if (!x) return;
  removing = x;
  $('confirm-text').textContent = F('Remove {name} from the list?', { name: x.file });
  $('confirm-file').checked = false;
  $('confirm').showModal();
}

$('confirm').addEventListener('close', async () => {
  const x = removing;
  removing = null;
  if (!x || $('confirm').returnValue !== 'ok') return;
  try {
    if (x.status === 'Downloading') await act('pause', [x.id]);
    const j = await act($('confirm-file').checked ? 'delete' : 'remove', [x.id]);
    if (j.skipped && j.skipped.length) toast(T('MDM could not do it.'), true);
  } catch (e) { toast(T('MDM could not do it.'), true); }
  refresh();
});

async function queueAction(name, start) {
  try { await api('queue', { json: { name, start } }); } catch (e) { toast(T('MDM could not do it.'), true); }
  refresh();
}

for (const tab of document.querySelectorAll('[data-filter]')) {
  tab.addEventListener('click', () => {
    filter = tab.dataset.filter;
    for (const t of document.querySelectorAll('[data-filter]')) t.setAttribute('aria-selected', String(t === tab));
    if (data) render();
  });
}

// ---- adding ------------------------------------------------------------------------------

$('file').addEventListener('change', () => {
  const f = $('file').files[0];
  chosenFile = null;
  $('file-name').hidden = true;
  if (!f) return;
  if (f.size > 10 * 1024 * 1024) { toast(T('This file is too large to be a torrent.'), true); $('file').value = ''; return; }
  chosenFile = f;
  $('url').value = '';
  $('file-name').textContent = f.name;
  $('file-name').hidden = false;
});
$('url').addEventListener('input', () => {
  if ($('url').value) { chosenFile = null; $('file').value = ''; $('file-name').hidden = true; }
});
document.querySelector('label.file').addEventListener('keydown', (ev) => {
  if (ev.key === 'Enter' || ev.key === ' ') { ev.preventDefault(); $('file').click(); }
});

$('add').addEventListener('submit', async (ev) => {
  ev.preventDefault();
  const category = $('category').value;
  const queue = $('when').value;
  const start = queue === '';
  const url = $('url').value.trim();
  if (!url && !chosenFile) { $('url').focus(); return; }
  const button = $('add').querySelector('button[type=submit]');
  button.disabled = true;
  try {
    let j;
    if (chosenFile) {
      const q = new URLSearchParams({ category, queue, start: String(start) });
      j = await api('torrent?' + q, { body: await chosenFile.arrayBuffer(), headers: { 'Content-Type': 'application/x-bittorrent' } });
    } else {
      j = await api('add', { json: { url, category, queue, start } });
    }
    toast(j && j.pending ? T('Added. MDM is reading the torrent: it joins the list in a moment.') : T('Added.'));
    $('url').value = '';
    $('file').value = '';
    chosenFile = null;
    $('file-name').hidden = true;
  } catch (e) {
    if (e.code === 'unsupported') toast(T('MDM cannot download this address.'), true);
    else if (e.code === 'not-a-torrent') toast(T('This is not a torrent file.'), true);
    else if (e.code !== 'not-paired') toast(T('MDM could not add it.'), true);
  } finally {
    button.disabled = false;
  }
  refresh();
});

// ---- the menu ----------------------------------------------------------------------------

$('menu').addEventListener('click', (ev) => {
  ev.stopPropagation();
  const open = $('menu-list').hidden;
  $('menu-list').hidden = !open;
  $('menu').setAttribute('aria-expanded', String(open));
});
document.addEventListener('click', () => { $('menu-list').hidden = true; $('menu').setAttribute('aria-expanded', 'false'); });
$('forget').addEventListener('click', async () => {
  if (!confirm(T('Forget this device? It needs a new code to connect again.'))) return;
  try { await api('forget', { post: true }); } catch (e) { /* unpaired here anyway */ }
  signedOut();
});

document.addEventListener('visibilitychange', () => {
  if (document.visibilityState === 'visible' && token) refresh(); else stopPolling();
});

loadTexts().then(() => { if (token) showMain(); else showPair(); });
