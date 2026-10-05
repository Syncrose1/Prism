/* PRISM OS: the shell. A view of the PC's real resources (files, services,
 * vitals, the record of who came in); nothing here is a source of truth.
 * Design: POLARIS/design/prism-os-concept.html (concept 4). Layout lives on
 * the host (/api/workspace, under its own `os` key beside the classic
 * shell's), so the same desktop follows the person between devices. */
'use strict';

const $ = s => document.querySelector(s), $$ = s => [...document.querySelectorAll(s)];
const reduce = matchMedia('(prefers-reduced-motion: reduce)').matches;
const el = html => { const t = document.createElement('template'); t.innerHTML = html.trim(); return t.content.firstElementChild; };
const esc = s => String(s ?? '').replace(/[&<>"']/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));

/* ── Pictograms: 2.6-unit strokes, white on a sticker ─────────────────── */
const ICONS = {
  folder: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>',
  photo: '<rect x="3" y="5" width="18" height="14" rx="3"/><circle cx="9" cy="10" r="1.8"/><path d="M4 17l5-5 4 4 3-3 4 4"/>',
  play: '<path d="M8 5l11 7-11 7z"/>',
  stack: '<rect x="4" y="4" width="16" height="5" rx="2"/><rect x="4" y="11" width="16" height="5" rx="2"/><path d="M8 20h8"/>',
  pulse: '<path d="M3 13h4l2-5 4 10 2-5h6"/>',
  shield: '<path d="M12 3l7 3v5c0 5-3 8-7 10-4-2-7-5-7-10V6z"/>',
  prompt: '<path d="M5 8l4 4-4 4M12 17h7"/>',
  globe: '<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3c3 3 3 15 0 18M12 3c-3 3-3 15 0 18"/>',
  home: '<path d="M4 11l8-7 8 7v9H4z"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  x: '<path d="M6 6l12 12M18 6 6 18"/>',
  solo: '<rect x="4" y="4" width="16" height="16" rx="3"/>',
  doc: '<path d="M7 3h7l4 4v14H7z"/><path d="M14 3v4h4M10 12h5M10 16h5"/>',
  music: '<path d="M9 18V6l10-2v12"/><circle cx="6.5" cy="18" r="2.5"/><circle cx="16.5" cy="16" r="2.5"/>',
  box: '<path d="M4 8l8-4 8 4v8l-8 4-8-4z"/><path d="M4 8l8 4 8-4M12 12v8"/>',
  up: '<path d="M12 16V5M7 10l5-5 5 5M5 19h14"/>',
  newdir: '<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M12 10v6M9 13h6"/>',
  left: '<path d="M15 5 8 12l7 7"/>',
  right: '<path d="M9 5l7 7-7 7"/>',
  star: '<path d="M12 2.5Q14 10 21.5 12Q14 14 12 21.5Q10 14 2.5 12Q10 10 12 2.5Z" fill="#33BFE2" stroke="none"/>',
  tiles: '<rect x="3" y="4" width="8" height="16" rx="2"/><rect x="13" y="4" width="8" height="7" rx="2"/><rect x="13" y="13" width="8" height="7" rx="2"/>',
  windows: '<rect x="3" y="7" width="12" height="11" rx="2"/><rect x="9" y="3" width="12" height="11" rx="2"/>',
  link: '<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1"/><path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>',
};
const svg = (n, c = '#fff', w = 2.6) => `<svg viewBox="0 0 24 24" fill="none" stroke="${c}" stroke-width="${w}" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${ICONS[n]}</svg>`;

/* ── The daemon ──────────────────────────────────────────────────────── */
class ApiError extends Error { constructor(status, body) { super(body?.detail || body?.error || `HTTP ${status}`); this.status = status; this.body = body; } }
async function call(method, path, body, raw) {
  const opt = { method, credentials: 'same-origin', headers: {} };
  if (body !== undefined) {
    if (raw) opt.body = body;
    else { opt.body = JSON.stringify(body); opt.headers['Content-Type'] = 'application/json'; }
  }
  const r = await fetch(path, opt);
  if (r.status === 401 && signedIn) { signedIn = false; toast('Signed out on this device. Sign in again to carry on.', true); playSignin(); }
  if (!r.ok) { let b = null; try { b = await r.json(); } catch (e) {} throw new ApiError(r.status, b); }
  if (r.status === 204) return null;
  const type = r.headers.get('content-type') || '';
  return type.includes('json') ? r.json() : r.text();
}
const api = { get: p => call('GET', p), post: (p, b) => call('POST', p, b ?? {}), put: (p, b) => call('PUT', p, b) };
const q = o => Object.entries(o).map(([k, v]) => `${k}=${encodeURIComponent(v)}`).join('&');

function toast(text, bad) {
  const t = el(`<div class="toast${bad ? ' bad' : ''}">${esc(text)}</div>`);
  $('#toasts').appendChild(t);
  setTimeout(() => { t.classList.add('gone'); setTimeout(() => t.remove(), 450); }, bad ? 6500 : 3800);
}
const mib = m => m >= 1024 ? `${(m / 1024).toFixed(1)} GiB` : `${Math.round(m)} MiB`;
const bytes = b => b == null ? '' : b >= 1 << 30 ? `${(b / (1 << 30)).toFixed(1)} GB` : b >= 1 << 20 ? `${(b / (1 << 20)).toFixed(1)} MB` : b >= 1024 ? `${Math.round(b / 1024)} KB` : `${b} B`;
const ago = unix => {
  const s = Date.now() / 1000 - unix;
  if (s < 60) return 'just now'; if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return new Date(unix * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
};
const clock = unix => new Date(unix * 1000).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false });

/* ── The sky, and the chevron veil ───────────────────────────────────── */
const sky = $('#sky'), sg = sky.getContext('2d');
const dpr = () => Math.min(2, devicePixelRatio || 1);
const stars = Array.from({ length: 34 }, () => ({ x: Math.random(), y: Math.random(), r: 2 + Math.random() * 5, s: .000004 + Math.random() * .00001, tw: Math.random() * 6.28 }));
let gather = 0, skyOn = true;
function size(c) { c.width = innerWidth * dpr(); c.height = innerHeight * dpr(); c.getContext('2d').setTransform(dpr(), 0, 0, dpr(), 0, 0); }
addEventListener('resize', () => { size(sky); size($('#veil')); });
size(sky); size($('#veil'));
function starPath(g, x, y, r) { const b = r * .34; g.beginPath(); g.moveTo(x, y - r); g.quadraticCurveTo(x + b, y - b, x + r, y); g.quadraticCurveTo(x + b, y + b, x, y + r); g.quadraticCurveTo(x - b, y + b, x - r, y); g.quadraticCurveTo(x - b, y - b, x, y - r); g.closePath(); }
(function drawSky(t) {
  sg.clearRect(0, 0, innerWidth, innerHeight);
  if (skyOn) {
    sg.fillStyle = getComputedStyle(document.documentElement).getPropertyValue('--sky').trim() || '#33BFE2';
    for (const s of stars) { sg.globalAlpha = .45 * (.6 + .4 * Math.sin(t / 1400 + s.tw)) * Math.min(1, gather); starPath(sg, ((s.x + t * s.s) % 1) * innerWidth, s.y * innerHeight, s.r); sg.fill(); }
    sg.globalAlpha = 1;
  }
  if (!reduce) requestAnimationFrame(drawSky);
})(0);
function tween(ms, f, done) { const t0 = performance.now(); (function s(n) { const k = Math.min(1, (n - t0) / (reduce ? 1 : ms)); f(k); k < 1 ? requestAnimationFrame(s) : done && done(); })(t0); }
function chevronSweep(mid, done) {
  const veil = $('#veil'), vg = veil.getContext('2d'), W = innerWidth, H = innerHeight;
  const css = getComputedStyle(document.documentElement); const c1 = css.getPropertyValue('--veil-1').trim(), c2 = css.getPropertyValue('--veil-2').trim();
  const e = x => 1 - Math.pow(1 - Math.max(0, Math.min(1, x)), 3);
  const shape = (y, col) => { vg.fillStyle = col; vg.beginPath(); vg.moveTo(-W * .1, y + H * .45); vg.lineTo(W / 2, y); vg.lineTo(W * 1.1, y + H * .45); vg.lineTo(W * 1.1, y + H * 3); vg.lineTo(-W * .1, y + H * 3); vg.closePath(); vg.fill(); };
  let midDone = false;
  tween(1500, k => {
    vg.clearRect(0, 0, W, H);
    if (k < .62) { shape(H * (1.05 - 1.9 * e(k / .55)), c1); shape(H * (1.25 - 2.1 * e((k - .08) / .54)), c2); }
    else { if (!midDone) { midDone = true; mid(); } vg.globalAlpha = 1 - e((k - .62) / .38); vg.fillStyle = c2; vg.fillRect(0, 0, W, H); vg.globalAlpha = 1; }
  }, () => { vg.clearRect(0, 0, W, H); if (!midDone) mid(); done && done(); });
}

/* ── State, kept on the host ─────────────────────────────────────────── */
const state = { scene: 'signin', wm: 'float', panes: [], focus: null, pos: {}, host: 'this PC', role: 'owner', me: null, people: [] };
const GUEST_APPS = ['files', 'photos', 'videos'];
const allowed = id => state.role !== 'guest' || GUEST_APPS.includes(id);
let signedIn = false, saveTimer = null, workspace = {};
function persist() {
  // A guest's place isn't the PC's layout: it stays on their own device.
  if (state.role === 'guest') return;
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    const os = { v: 1, wm: state.wm, panes: state.panes.map(p => ({ id: p.id, args: p.args })), focus: state.focus, pos: state.pos };
    workspace = { ...(workspace || {}), os };
    api.put('/api/workspace', workspace).catch(() => {});
  }, 600);
}
async function restore() {
  if (state.role === 'guest') return null;
  try { workspace = (await api.get('/api/workspace')) || {}; } catch (e) { workspace = {}; }
  const os = workspace.os;
  if (os) { state.wm = os.wm === 'tile' ? 'tile' : 'float'; state.pos = os.pos || {}; return os; }
  return null;
}

/* ── Sign-in ─────────────────────────────────────────────────────────── */
const lockup = $('#lockup');
let seq = 0, prompt = 'code';
function dash(e, k) { const L = e.getTotalLength(); e.style.strokeDasharray = L; e.style.strokeDashoffset = L * (1 - k); }
function showScene(id) { $$('.scene').forEach(s => s.classList.toggle('on', s.id === 's-' + id)); state.scene = id; skyOn = id !== 'work'; renderStrip(); }
function topOn(on) { $('#top').classList.toggle('on', on); if (!on) { closeSheet('#open'); closeSheet('#menu'); } }

async function playSignin() {
  const my = ++seq;
  showScene('signin'); topOn(false); $('#welcome').classList.remove('in');
  lockup.classList.remove('up', 'breathe'); $$('#word span').forEach(s => s.classList.remove('in'));
  ['#ask', '#cred', '#credhint', '#swap'].forEach(s => $(s).classList.remove('in'));
  ['#mkBack', '#mkFront', '#ray1', '#ray2', '#ray3'].forEach(s => dash($(s), 0));
  try { const p = await (await fetch('/api/auth/prompt', { credentials: 'same-origin' })).json(); prompt = p.prompt; $('#swap').dataset.canPassword = p.has_password ? '1' : ''; } catch (e) {}
  $('#people').innerHTML = '';
  try { state.people = (await (await fetch('/api/auth/people', { credentials: 'same-origin' })).json()).people || []; } catch (e) { state.people = []; }
  // Everyone another program on this PC vouches for (POLARIS's people), and
  // the owner with an authenticator code.
  for (const p of state.people) {
    const [c1, c2] = p.colours || ['#8A8F99', '#4A4E56'];
    const b = el(`<button class="person" type="button"><span class="pic" style="background:linear-gradient(135deg,${esc(c1)},${esc(c2)})">${esc((p.name || '?')[0].toUpperCase())}<svg class="ring" viewBox="0 0 100 100"><circle cx="50" cy="50" r="46"/></svg></span>${esc(p.name)}<small>${p.owner ? 'POLARIS' : 'POLARIS · guest'}</small></button>`);
    b.addEventListener('click', () => choose(b, p)); $('#people').appendChild(b);
  }
  const b = el(`<button class="person" type="button"><span class="pic">O<svg class="ring" viewBox="0 0 100 100"><circle cx="50" cy="50" r="46"/></svg></span>Owner<small>${state.people.length ? 'Authenticator code' : (state.host === 'this PC' ? "This PC's owner" : esc(state.host))}</small></button>`);
  b.addEventListener('click', () => choose(b, null)); $('#people').appendChild(b);
  gather = 0;
  tween(900, k => gather = k, () => {
    if (my !== seq) return;
    tween(420, k => dash($('#mkBack'), k));
    setTimeout(() => tween(420, k => dash($('#mkFront'), k), () => {
      ['#ray1', '#ray2', '#ray3'].forEach((s, i) => setTimeout(() => tween(320, k => dash($(s), k)), i * 110));
      $$('#word span').forEach((s, i) => setTimeout(() => s.classList.add('in'), 300 + i * 90));
      setTimeout(() => {
        if (my !== seq) return;
        lockup.classList.add('up'); setTimeout(() => lockup.classList.add('breathe'), 900);
        setTimeout(() => { $('#ask').textContent = "Who's here?"; $('#ask').classList.add('in'); $$('.person').forEach((p, i) => setTimeout(() => p.classList.add('in'), 200 + i * 120)); }, 600);
      }, 1300);
    }), 260);
  });
}
let chosen = null;
function setPrompt(kind) {
  prompt = kind; const pw = $('#pw');
  pw.value = '';
  pw.type = kind === 'password' ? 'password' : 'text';
  pw.inputMode = kind === 'password' ? 'text' : 'numeric';
  pw.autocomplete = kind === 'password' ? 'current-password' : 'one-time-code';
  pw.setAttribute('aria-label', kind === 'password' ? 'Password' : 'Authenticator code');
  pw.placeholder = kind === 'password' || kind === 'account' ? 'Password' : '6-digit code';
  if (kind === 'account') { pw.type = 'password'; pw.inputMode = 'text'; pw.autocomplete = 'current-password'; pw.setAttribute('aria-label', 'Password'); }
  const hint = $('#credhint'); hint.classList.remove('bad');
  hint.textContent = kind === 'account' ? `${chosen.name}'s POLARIS password. ${chosen.owner ? '' : 'A guest can look at files, and nothing more.'}` : kind === 'password' ? 'This browser has signed in before, so your password is enough.' : 'The code from your authenticator app. It also remembers this browser, so next time a password will do.';
  const sw = $('#swap');
  sw.hidden = kind === 'account' || !(kind === 'password' || sw.dataset.canPassword);
  sw.textContent = kind === 'password' ? 'Use a code instead' : 'Use my password';
}
let ownerPrompt = 'code';
function choose(btn, person) {
  chosen = person;
  $$('.person').forEach(p => p.classList.toggle('dim', btn && p !== btn));
  $('#ask').textContent = person ? `Hello, ${person.name}` : 'Hello';
  if (!person && prompt === 'account') prompt = ownerPrompt;
  if (person && prompt !== 'account') ownerPrompt = prompt;
  setPrompt(person ? 'account' : prompt);
  ['#cred', '#credhint', '#swap'].forEach(s => $(s).classList.add('in'));
  setTimeout(() => $('#pw').focus({ preventScroll: true }), 250);
}
$('#swap').addEventListener('click', () => { setPrompt(prompt === 'password' ? 'code' : 'password'); $('#pw').focus(); });
$('#cred').addEventListener('submit', async e => {
  e.preventDefault();
  const v = $('#pw').value.trim(); if (!v) return;
  const pic = ($$('.person').find(p => !p.classList.contains('dim')) || $('.person')).querySelector('.pic'); pic.classList.remove('no'); pic.classList.add('verify');
  $('#go').disabled = true;
  try {
    if (prompt === 'account') await call('POST', '/api/auth/account', { who: chosen.id, password: v });
    else await call('POST', '/api/auth/login', prompt === 'password' ? { password: v } : { code: v.replace(/\s/g, '') });
    signedIn = true;
    ['#cred', '#credhint', '#swap'].forEach(s => $(s).classList.remove('in'));
    setTimeout(() => { pic.classList.add('pulse'); chevronSweep(() => enter(), null); }, reduce ? 0 : 900);
  } catch (err) {
    pic.classList.remove('verify'); void pic.offsetWidth; pic.classList.add('no');
    const h = $('#credhint'); h.classList.add('bad');
    h.textContent = err.status === 429 ? `Too many tries. ${err.message}.` : err.status === 503 ? 'POLARIS didn\u2019t answer. Try the owner\u2019s code instead.' : err.body?.error === 'code_already_used' ? 'That code was just used. Wait for the next one.' : prompt === 'account' ? `That password doesn't open ${chosen.name}'s account.` : prompt === 'password' ? "That password doesn't open this PC." : "That code doesn't open this PC. Check the time on your phone, then try the next one.";
    $('#pw').select();
  } finally { $('#go').disabled = false; }
});

async function boot() {
  try { const s = await (await fetch('/api/system', { credentials: 'same-origin' })).json(); state.host = s.hostname || state.host; } catch (e) {}
  $('#host').textContent = state.host;
  document.title = `${state.host} · PRISM`;
  const r = await fetch('/api/vitals', { credentials: 'same-origin' });
  if (r.ok) {
    signedIn = true;
    // Kept signed in: the welcome alone on the sky, then the veil, as POLARIS does.
    showScene('signin'); lockup.classList.add('up'); ['#mkBack', '#mkFront', '#ray1', '#ray2', '#ray3'].forEach(s => dash($(s), 1)); $$('#word span').forEach(s => s.classList.add('in'));
    tween(700, k => gather = k);
    setTimeout(() => $('#welcome').classList.add('in'), 300);
    setTimeout(() => chevronSweep(() => enter(), null), reduce ? 0 : 1300);
  } else playSignin();
}

/* ── After sign-in ───────────────────────────────────────────────────── */
async function enter() {
  seq++; gather = 1; $('#welcome').classList.remove('in');
  try { const me = await api.get('/api/auth/me'); state.role = me.role || 'owner'; state.me = me.name; } catch (e) { state.role = 'owner'; }
  const person = state.people.find(p => p.name === state.me);
  if (person) { const [c1, c2] = person.colours || []; $('#me').style.background = `linear-gradient(135deg,${c1},${c2})`; }
  // The PC's name is behind the session, so it's known only now.
  try { const s = await api.get('/api/system'); state.host = s.hostname || state.host; $('#host').textContent = state.host; document.title = `${state.host} · PRISM`; $('#me').textContent = (state.me || state.host)[0].toUpperCase(); } catch (e) {}
  document.querySelector('a.mlink[href="/classic"]').hidden = state.role === 'guest';
  const os = await restore();
  syncMenu();
  topOn(true);
  pollVitals(); if (state.role !== 'guest') loadServices();
  if (os && os.panes && os.panes.length) {
    state.panes = os.panes.filter(p => p && p.id && (APPS[p.id] || p.id.startsWith('web:')) && allowed(p.id)).slice(0, 6);
    state.focus = os.focus;
    if (state.panes.length) { showScene('work'); renderPanes(); return; }
  }
  goHome();
}
$('#signout').addEventListener('click', async () => {
  try { await api.post('/api/auth/logout'); } catch (e) {}
  signedIn = false; closeSheet('#menu'); playSignin();
});

/* ── Top bar ─────────────────────────────────────────────────────────── */
setInterval(() => { $('#clock').textContent = new Date().toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', hour12: false }); }, 1000);
$('#clock').textContent = new Date().toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', hour12: false });
$('#device').addEventListener('click', () => { if (allowed('vitals')) openApp('vitals'); });
$('#me').addEventListener('click', () => toggleSheet('#menu'));
function toggleSheet(s) { const on = !$(s).classList.contains('on'); ['#open', '#menu'].forEach(x => closeSheet(x)); if (on) $(s).classList.add('on'); }
function closeSheet(s) { $(s).classList.remove('on'); }
document.addEventListener('pointerdown', e => { if (!e.target.closest('.sheet') && !e.target.closest('.strip .add') && !e.target.closest('#me')) { closeSheet('#open'); closeSheet('#menu'); } });
function syncMenu() {
  let t = 'auto'; try { t = localStorage.getItem('prism.theme') || 'auto'; } catch (e) {}
  $$('#themeSeg button').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.v === t)));
  $$('#wmSeg button').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.v === state.wm)));
}
$$('#themeSeg button').forEach(b => b.addEventListener('click', () => {
  const t = b.dataset.v; try { localStorage.setItem('prism.theme', t); } catch (e) {}
  const dark = matchMedia('(prefers-color-scheme: dark)').matches;
  document.documentElement.dataset.theme = t === 'auto' ? (dark ? 'ash' : 'pearl') : t;
  syncMenu();
}));
$$('#wmSeg button').forEach(b => b.addEventListener('click', () => { setWM(b.dataset.v); syncMenu(); }));

/* ── Apps ────────────────────────────────────────────────────────────── */
const APPS = {
  files: { name: 'Files', c: '#E5A23A', icon: 'folder', build: filesApp },
  photos: { name: 'Photos', c: '#2E71C8', icon: 'photo', build: photosApp },
  videos: { name: 'Videos', c: '#C0485C', icon: 'play', build: videosApp },
  services: { name: 'Services', c: '#2E9E62', icon: 'stack', build: servicesApp },
  vitals: { name: 'Vitals', c: '#33BFE2', icon: 'pulse', build: vitalsApp },
  activity: { name: 'Activity', c: '#5A5F68', icon: 'shield', build: activityApp },
  terminal: { name: 'Terminal', c: '#2F3138', icon: 'prompt', build: terminalApp, flush: true },
};
let facets = [];
function appOf(id) {
  if (id.startsWith('web:')) {
    const f = facets.find(x => x.id === id.slice(4));
    const pol = f && /face-stream/.test(f.command || '');
    return { name: f ? (f.expose?.title || f.name) : id.slice(4), c: pol ? '#7C5CE0' : '#2E71C8', icon: 'globe', build: webApp, flush: true, pol };
  }
  return APPS[id];
}

/* ── Home ────────────────────────────────────────────────────────────── */
function goHome() { seq++; gather = 1; showScene('home'); topOn(true); renderHome(); }
async function renderHome() {
  const h = $('#s-home');
  const tiles = Object.entries(APPS).filter(([id]) => allowed(id)).map(([id, a], i) => `<button class="tile" data-open="${id}"><span class="m" style="--c:${a.c};--d:${(-i * 0.7) % 4.4}s">${svg(a.icon)}</span>${a.name}</button>`).join('');
  h.innerHTML = `
    <div class="jump"><h3>Jump back in</h3><div class="posters" id="posters"><div class="poster" aria-hidden="true"><span class="art">${svg('photo', 'var(--shade)')}</span></div></div></div>
    <div class="tiles">${tiles}</div>
    <div class="shelf" id="hostedShelf" hidden><div class="head2"><h3>Hosted on this PC</h3><span>opened through PRISM, no ports to remember</span></div><div class="tiles" id="hosted"></div></div>
    <div class="widgets">
      <div class="head2"><h3>Widgets</h3></div>
      <div class="card"><div class="well"><span class="lamp" id="lamp2" data-t="green"></span><b id="wellWord">All's well</b></div><p id="wellLine">Reading this PC…</p><svg class="spark" viewBox="0 0 280 40" preserveAspectRatio="none" aria-hidden="true"><path class="a" id="sparkA"/><path class="l" id="sparkL"/></svg></div>
      ${state.role === 'guest' ? `<div class="card"><span class="k">Signed in as a guest</span><b>${esc(state.me || 'Guest')}</b><p>You can look at the files shared on this PC, and play their pictures and films.</p></div>` : `<div class="card" id="lastIn"><span class="k">Last signed in</span><b>…</b></div>
      <div class="card" id="svcCard"><span class="k">Services</span><b>…</b></div>`}
    </div>`;
  h.querySelectorAll('[data-open]').forEach(b => b.addEventListener('click', () => launch(b, b.dataset.open)));
  renderVitalsWidgets();
  posters();
  if (state.role === 'guest') return;
  loadServices().then(renderHosted);
  try {
    const a = await api.get('/api/access?limit=20');
    const last = (a.entries || []).find(e => e.kind === 'signedin' && !e.who.startsWith('link:'));
    $('#lastIn').innerHTML = last ? `<span class="k">Last signed in</span><b>${esc(last.who === 'admin' ? 'You' : last.who)}${last.how === 'console' ? ', on this PC' : `, from ${esc(last.from)}`}</b><p>${ago(last.unix)} · ${esc({ code: 'with a code', password: 'with your password', console: 'from this PC', bridge: 'through POLARIS' }[last.how] || last.how)}</p>` : `<span class="k">Last signed in</span><b>Nobody yet</b><p>Every sign-in is recorded here, sealed.</p>`;
  } catch (e) {}
}
// Only what the person opened before on this device: the last picture and
// the last folder. Never a file surfaced on its own: a desktop that puts
// someone's newest picture on screen at sign-in is a desktop nobody can
// open in front of anyone.
function posters() {
  const box = $('#posters'); if (!box) return;
  const read = k => { try { return JSON.parse(localStorage.getItem(k) || 'null'); } catch (e) { return null; } };
  const photo = read('prism.lastPhoto'), folder = read('prism.lastFolder');
  box.innerHTML = '';
  if (photo) {
    const b = el(`<button class="poster"><img alt="" src="/api/files/thumb?${q({ root: photo.root, path: photo.dir ? photo.dir + '/' + photo.name : photo.name })}"><span class="cap"><span style="min-width:0"><b>${esc(photo.name)}</b><span>The picture you were looking at</span></span></span></button>`);
    b.addEventListener('click', () => openApp('photos', photo)); box.appendChild(b);
  }
  if (folder) {
    const b = el(`<button class="poster"><span class="art">${svg('folder', '#E5A23A')}</span><span class="cap"><span style="min-width:0"><b>${esc(folder.path ? folder.path.split('/').pop() : folder.root)}</b><span>${esc(folder.root)}${folder.path ? ' › ' + esc(folder.path) : ''}</span></span></span></button>`);
    b.addEventListener('click', () => openApp('files', folder)); box.appendChild(b);
  }
  if (!box.children.length) box.appendChild(el(`<div class="card"><b>Nothing to carry on yet</b><p>The picture and the folder you were last in will wait here.</p></div>`));
}
async function loadServices() { try { facets = await api.get('/api/facets'); } catch (e) {} return facets; }
function renderHosted() {
  const shelf = $('#hostedShelf'), box = $('#hosted'); if (!box) return;
  const exposed = facets.filter(f => f.expose);
  shelf.hidden = !exposed.length;
  box.innerHTML = '';
  exposed.forEach((f, i) => {
    const pol = /face-stream/.test(f.command || ''), running = f.state === 'running' || f.state === 'foreign';
    const b = el(`<button class="tile"><span class="m" style="--c:${pol ? '#7C5CE0' : running ? '#2E9E62' : '#9A978F'};--d:${-i * .9}s">${svg(pol ? 'play' : 'globe')}${pol ? `<span class="pstar">${svg('star')}</span>` : ''}</span>${esc(f.expose.title || f.name)}<small>${running ? (pol ? 'POLARIS, streamed' : 'running') : 'stopped'}</small></button>`);
    b.addEventListener('click', async () => {
      if (!running) {
        if (f.state === 'stopped' && f.available) { toast(`Starting ${f.name}…`); try { await api.post(`/api/facets/${f.id}/start`); } catch (e) { toast(`${f.name} didn't start: ${e.message}`, true); return; } await new Promise(r => setTimeout(r, pol ? 15000 : 2500)); }
        else { toast(`${f.name} isn't running.`, true); return; }
      }
      openApp('web:' + f.id);
    });
    box.appendChild(b);
  });
  const run = facets.filter(f => f.state === 'running').length;
  const card = $('#svcCard'); if (card) card.innerHTML = `<span class="k">Services</span><b>${run} running</b><p>${esc(facets.filter(f => f.state === 'running').map(f => f.name).join(', ') || 'Nothing PRISM started is running.')}</p>`;
}

/* App launch: the tile glides, its name appears, the App fades in. */
function launch(tile, id) {
  const m = tile.querySelector('.m'), a = appOf(id); if (!m || reduce) { openApp(id); return; }
  const r = m.getBoundingClientRect();
  const L = el(`<div class="launch" style="--c:${a.c}">${svg(a.icon)}</div>`);
  Object.assign(L.style, { left: r.left + 'px', top: r.top + 'px', width: r.width + 'px', height: r.height + 'px' }); document.body.appendChild(L);
  const T = el(`<div class="launchTitle">${esc(a.name)}</div>`); T.style.left = '-9999px'; document.body.appendChild(T);
  $('#s-home').style.opacity = 0;
  const sz = 112, tw = T.getBoundingClientRect().width, left = (innerWidth - (sz + 22 + tw)) / 2, top = innerHeight / 2;
  requestAnimationFrame(() => {
    Object.assign(L.style, { left: left + 'px', top: top - sz / 2 + 'px', width: sz + 'px', height: sz + 'px' });
    Object.assign(T.style, { left: left + sz + 22 + 'px', top: top - 16 + 'px' });
    setTimeout(() => T.style.opacity = 1, 300);
    setTimeout(() => { openApp(id, undefined, true); $('#s-home').style.opacity = ''; L.style.opacity = 0; T.style.opacity = 0; setTimeout(() => { L.remove(); T.remove(); }, 450); }, 1000);
  });
}

/* ── The window manager: floating or tiled, the person's choice ──────── */
const MAXN = () => state.wm === 'float' ? 6 : 4;
let zTop = 10;
function openApp(id, args, alone) {
  const a = appOf(id); if (!a || !allowed(id)) return;
  const existing = state.panes.find(p => p.id === id);
  if (existing) { if (args) { existing.args = args; const pe = paneEl(id); if (pe) rebuild(pe, existing); } state.focus = id; }
  else if (alone || state.scene !== 'work') { state.panes = [{ id, args }]; state.focus = id; }
  else {
    if (state.panes.length >= MAXN()) state.panes[state.panes.findIndex(p => p.id === state.focus)] = { id, args };
    else state.panes.push({ id, args });
    state.focus = id;
  }
  showScene('work'); renderPanes(); persist();
}
const paneEl = id => $(`.pane[data-app="${CSS.escape(id)}"]`);
function floatSpot(i, n, W, H) {
  const cells = { 1: [[0, 0, 1, 1]], 2: [[0, 0, .5, 1], [.5, 0, .5, 1]], 3: [[0, 0, .57, 1], [.57, 0, .43, .5], [.57, .5, .43, .5]], 4: [[0, 0, .5, .5], [.5, 0, .5, .5], [0, .5, .5, .5], [.5, .5, .5, .5]] };
  const c = (cells[n] || [])[i];
  if (c) { const inset = n === 1 ? .05 : .025; return { l: (c[0] + inset) * W, t: (c[1] + inset) * H, w: (c[2] - inset * 1.4) * W - 14, h: (c[3] - inset * 1.4) * H - 14 }; }
  return { l: W * (.1 + .05 * i), t: H * (.06 + .05 * i), w: W * .5, h: H * .62 };
}
function placeFloat() {
  const box = $('#panes'), W = box.clientWidth, H = box.clientHeight;
  state.panes.forEach((p, i) => {
    const e = paneEl(p.id); if (!e) return;
    let s = state.pos[p.id];
    if (!s || s.l > W - 80 || s.t > H - 40) s = state.pos[p.id] = floatSpot(i, state.panes.length, W, H);
    Object.assign(e.style, { left: s.l + 'px', top: s.t + 'px', width: Math.min(s.w, W) + 'px', height: Math.min(s.h, H) + 'px' });
    if (!e.style.zIndex) e.style.zIndex = ++zTop;
  });
}
function glide(change) {
  const before = new Map(state.panes.map(p => [p.id, paneEl(p.id)?.getBoundingClientRect()]));
  change();
  if (reduce) return;
  state.panes.forEach(p => {
    const e = paneEl(p.id), a = before.get(p.id); if (!e || !a) return;
    const b = e.getBoundingClientRect(); if (!b.width) return;
    e.animate([{ transformOrigin: 'top left', transform: `translate(${a.left - b.left}px,${a.top - b.top}px) scale(${a.width / b.width},${a.height / b.height})` }, { transformOrigin: 'top left', transform: 'none' }], { duration: 480, easing: 'cubic-bezier(.2,.8,.2,1)' });
  });
}
function setWM(mode) {
  if (state.wm === mode) return;
  glide(() => {
    state.wm = mode; const box = $('#panes');
    box.classList.toggle('float', mode === 'float');
    if (mode === 'float') placeFloat();
    else { $$('.pane').forEach(e => { e.style.left = e.style.top = e.style.width = e.style.height = e.style.zIndex = ''; e.classList.remove('max'); }); if (state.panes.length > 4) { state.panes = state.panes.slice(0, 4); renderPanes(); } }
  });
  renderStrip(); persist();
}
function renderPanes() {
  const box = $('#panes');
  box.dataset.n = Math.min(state.panes.length, 4); box.classList.toggle('float', state.wm === 'float');
  const have = new Map([...box.children].map(c => [c.dataset.app, c]));
  for (const [id, e] of have) if (!state.panes.some(p => p.id === id)) { e.__close && e.__close(); e.remove(); }
  const order = [...box.children].map(c => c.dataset.app).join();
  state.panes.forEach(p => { const e = have.get(p.id) || makePane(p); if (!e.isConnected || order !== state.panes.map(x => x.id).join()) box.appendChild(e); e.classList.toggle('focus', p.id === state.focus); });
  if (!state.focus || !state.panes.some(p => p.id === state.focus)) state.focus = state.panes[state.panes.length - 1]?.id;
  if (state.wm === 'float') { placeFloat(); const f = paneEl(state.focus); if (f && +f.style.zIndex !== zTop) f.style.zIndex = ++zTop; }
  renderStrip();
}
function rebuild(e, p) { const body = e.querySelector('.pbody'); body.__close && body.__close(); body.innerHTML = ''; body.appendChild(appOf(p.id).build(p.args, body, p)); }
function makePane(p) {
  const a = appOf(p.id);
  const e = el(`<section class="pane" data-app="${esc(p.id)}"><div class="phead"><span class="tk" style="background:${a.c}">${svg(a.icon)}</span><h2>${esc(a.name)}</h2><button class="b solo" title="Only this one" aria-label="Only ${esc(a.name)}">${svg('solo', 'currentColor', 2.4)}</button><button class="b x" aria-label="Close ${esc(a.name)}">${svg('x', 'currentColor', 2.6)}</button></div><div class="pbody${a.flush ? ' flush' : ''}"></div><span class="grip" aria-hidden="true"></span></section>`);
  const body = e.querySelector('.pbody');
  body.appendChild(a.build(p.args, body, p));
  e.__close = () => body.__close && body.__close();
  e.addEventListener('pointerdown', () => { if (state.focus !== p.id) { state.focus = p.id; renderPanes(); persist(); } });
  e.querySelector('.x').addEventListener('click', ev => { ev.stopPropagation(); closePane(p.id); });
  e.querySelector('.solo').addEventListener('click', ev => { ev.stopPropagation(); state.panes = [state.panes.find(x => x.id === p.id)]; state.focus = p.id; renderPanes(); persist(); });
  const head = e.querySelector('.phead');
  head.addEventListener('dblclick', ev => { if (state.wm === 'float' && !ev.target.closest('button')) glide(() => e.classList.toggle('max')); });
  head.addEventListener('pointerdown', ev => {
    if (ev.target.closest('button') || matchMedia('(max-width:640px)').matches) return;
    if (state.wm === 'float') {
      if (e.classList.contains('max')) return;
      head.setPointerCapture(ev.pointerId); const box = $('#panes'), sx = ev.clientX, sy = ev.clientY, s = state.pos[p.id], l0 = s.l, t0 = s.t;
      const mv = m => { s.l = Math.min(box.clientWidth - 120, Math.max(-s.w + 120, l0 + m.clientX - sx)); s.t = Math.min(box.clientHeight - 50, Math.max(0, t0 + m.clientY - sy)); e.style.left = s.l + 'px'; e.style.top = s.t + 'px'; };
      const up = () => { head.removeEventListener('pointermove', mv); head.removeEventListener('pointerup', up); persist(); };
      head.addEventListener('pointermove', mv); head.addEventListener('pointerup', up);
    } else if (state.panes.length > 1) {
      head.setPointerCapture(ev.pointerId); let over = null;
      const mv = m => { const t = document.elementFromPoint(m.clientX, m.clientY)?.closest('.pane'); if (over && over !== t) over.classList.remove('drop'); over = t && t !== e ? t : null; over && over.classList.add('drop'); };
      const up = () => { head.removeEventListener('pointermove', mv); head.removeEventListener('pointerup', up);
        if (over) { over.classList.remove('drop'); const ia = state.panes.findIndex(x => x.id === p.id), ib = state.panes.findIndex(x => x.id === over.dataset.app); [state.panes[ia], state.panes[ib]] = [state.panes[ib], state.panes[ia]]; renderPanes(); persist(); } };
      head.addEventListener('pointermove', mv); head.addEventListener('pointerup', up);
    }
  });
  const grip = e.querySelector('.grip');
  grip.addEventListener('pointerdown', ev => {
    ev.stopPropagation(); grip.setPointerCapture(ev.pointerId); const sx = ev.clientX, sy = ev.clientY, s = state.pos[p.id], w0 = s.w, h0 = s.h;
    const mv = m => { s.w = Math.max(320, w0 + m.clientX - sx); s.h = Math.max(220, h0 + m.clientY - sy); e.style.width = s.w + 'px'; e.style.height = s.h + 'px'; };
    const up = () => { grip.removeEventListener('pointermove', mv); grip.removeEventListener('pointerup', up); persist(); };
    grip.addEventListener('pointermove', mv); grip.addEventListener('pointerup', up);
  });
  return e;
}
function closePane(id) {
  const e = paneEl(id); if (!e) return;
  e.classList.add('closing');
  setTimeout(() => {
    e.__close && e.__close(); e.remove();
    state.panes = state.panes.filter(p => p.id !== id); delete state.pos[id];
    if (state.focus === id) state.focus = state.panes[state.panes.length - 1]?.id;
    persist();
    if (!state.panes.length) goHome(); else renderPanes();
  }, 280);
}
function renderStrip() {
  const s = $('#strip'); s.innerHTML = '';
  const h = el(`<button class="${state.scene === 'home' ? 'on' : ''}"><span class="tk" style="background:#2F3138">${svg('home')}</span>Home</button>`);
  h.addEventListener('click', goHome); s.appendChild(h);
  if (state.panes.length) s.appendChild(el('<span class="sep"></span>'));
  state.panes.forEach(p => {
    const a = appOf(p.id); if (!a) return;
    const b = el(`<button class="chip${state.scene === 'work' && p.id === state.focus ? ' on' : ''}"><span class="tk" style="background:${a.c}">${svg(a.icon)}</span>${esc(a.name)}<span class="x" role="button" aria-label="Close ${esc(a.name)}">${svg('x', 'currentColor', 3)}</span></button>`);
    b.addEventListener('click', ev => { if (ev.target.closest('.x')) { closePane(p.id); return; } state.focus = p.id; if (state.scene !== 'work') showScene('work'); renderPanes(); persist(); });
    s.appendChild(b);
  });
  const add = el(`<button class="add" aria-label="Open an App beside">${svg('plus', 'currentColor', 2.8)}</button>`);
  add.addEventListener('click', openSheet); s.appendChild(add);
  const wm = el(`<button class="wm" title="${state.wm === 'float' ? 'Switch to tiling' : 'Switch to floating windows'}" aria-label="${state.wm === 'float' ? 'Switch to tiling' : 'Switch to floating windows'}">${svg(state.wm === 'float' ? 'tiles' : 'windows', 'currentColor', 2.4)}</button>`);
  wm.addEventListener('click', () => { setWM(state.wm === 'float' ? 'tile' : 'float'); syncMenu(); }); s.appendChild(wm);
}
function openSheet() {
  const row = $('#openRow'); row.innerHTML = '';
  $('#openTitle').textContent = state.scene === 'work' ? 'Open beside' : 'Open';
  $('#openHint').textContent = state.wm === 'float' ? 'Up to six windows. One more replaces the one you are in.' : 'Up to four Apps share the screen. One more replaces the one you are in.';
  const all = [...Object.keys(APPS).filter(allowed), ...(state.role === 'guest' ? [] : facets).filter(f => f.expose && (f.state === 'running' || f.state === 'foreign')).map(f => 'web:' + f.id)];
  all.forEach(id => {
    const a = appOf(id);
    const b = el(`<button><span class="m" style="--c:${a.c}">${svg(a.icon)}</span>${esc(a.name)}</button>`);
    if (state.scene === 'work' && state.panes.some(p => p.id === id)) b.disabled = true;
    b.addEventListener('click', () => { closeSheet('#open'); openApp(id); }); row.appendChild(b);
  });
  toggleSheet('#open');
}
addEventListener('resize', () => { if (state.wm === 'float' && state.scene === 'work') placeFloat(); });
document.addEventListener('keydown', e => {
  if (e.key !== 'Escape') return;
  if ($('#open').classList.contains('on') || $('#menu').classList.contains('on')) { closeSheet('#open'); closeSheet('#menu'); return; }
  if (e.target.closest && e.target.closest('.xt')) return;
  if (state.scene === 'work' && state.focus) closePane(state.focus);
});

/* ── Files ───────────────────────────────────────────────────────────── */
const KIND_ICON = { dir: ['folder', '#E5A23A'], image: ['photo', '#2E71C8'], video: ['play', '#C0485C'], audio: ['music', '#7C5CE0'], text: ['doc', '#868B94'], pdf: ['doc', '#C0485C'], archive: ['box', '#9A978F'], other: ['doc', '#9A978F'] };
function filesApp(args, body, pane) {
  const root = el(`<div class="files"><div class="places"></div><div class="browse"><div class="crumbs"></div><div class="grid"></div></div></div>`);
  let roots = [], cur = { root: args?.root, path: args?.path || '' };
  const places = root.querySelector('.places'), crumbs = root.querySelector('.crumbs'), grid = root.querySelector('.grid');
  async function load() {
    pane.args = { ...cur }; persist();
    try { localStorage.setItem('prism.lastFolder', JSON.stringify(cur)); } catch (e) {}
    places.querySelectorAll('.place').forEach(b => b.setAttribute('aria-pressed', String(b.dataset.root === cur.root)));
    const parts = cur.path ? cur.path.split('/') : [];
    const writable = state.role !== 'guest' && roots.find(r => r.name === cur.root)?.writable;
    crumbs.innerHTML = '';
    const rb = el(`<button>${esc(cur.root)}</button>`); rb.addEventListener('click', () => { cur.path = ''; load(); }); crumbs.appendChild(rb);
    parts.forEach((p, i) => { crumbs.appendChild(el('<span>›</span>')); const b = el(`<button>${esc(p)}</button>`); b.addEventListener('click', () => { cur.path = parts.slice(0, i + 1).join('/'); load(); }); crumbs.appendChild(b); });
    const sb = el(`<button class="btn">${svg('link', '#fff', 2.4)}Share</button>`);
    sb.addEventListener('click', () => openShare(cur.root, cur.path, !!writable));
    const shareWrap = el(`<span class="acts"></span>`); shareWrap.appendChild(sb);
    if (!writable && state.role !== 'guest') crumbs.appendChild(shareWrap);
    if (writable) {
      const acts = el(`<span class="acts"><button class="btn q" data-a="dir">${svg('newdir', 'currentColor', 2.2)}Folder</button><label class="btn">${svg('up', '#fff', 2.4)}Upload<input type="file" multiple hidden></label></span>`);
      acts.querySelector('[data-a="dir"]').addEventListener('click', async () => {
        const name = await ask('A name for the new folder'); if (!name) return;
        try { await api.post('/api/files/mkdir', { root: cur.root, path: cur.path, name }); load(); } catch (e) { toast(e.message, true); }
      });
      acts.querySelector('input').addEventListener('change', ev => upload([...ev.target.files]));
      acts.appendChild(sb);
      crumbs.appendChild(acts);
    }
    grid.innerHTML = `<div class="empty">Opening…</div>`;
    let l;
    try { l = await api.get(`/api/files/list?${q({ root: cur.root, path: cur.path, limit: 2000 })}`); }
    catch (e) { grid.innerHTML = `<div class="empty">${esc(e.message)}</div>`; return; }
    const entries = (l.entries || []).filter(x => !x.name.startsWith('.')).sort((a, b) => (b.is_dir - a.is_dir) || a.name.localeCompare(b.name, undefined, { numeric: true }));
    grid.innerHTML = entries.length ? '' : `<div class="empty">${writable ? 'Empty. Drop files here to upload them.' : 'Empty.'}</div>`;
    const images = entries.filter(x => x.kind === 'image').map(x => x.name);
    for (const x of entries.slice(0, 600)) {
      const [ic, c] = KIND_ICON[x.kind] || KIND_ICON.other;
      const full = cur.path ? `${cur.path}/${x.name}` : x.name;
      const thumb = x.kind === 'image' || x.kind === 'video';
      const b = el(`<button class="thing"><span class="th">${svg(ic, c, 2.2)}${thumb ? `<img loading="lazy" alt="" src="/api/files/thumb?${q({ root: cur.root, path: full })}" onerror="this.remove()">` : ''}${x.is_dir && state.role !== 'guest' ? `<span class="fshare" role="button" aria-label="Share ${esc(x.name)}" title="Share this folder">${svg('link', '#fff', 2.4)}</span>` : ''}</span><b title="${esc(x.name)}">${esc(x.name)}</b><small>${x.is_dir ? 'Folder' : bytes(x.size)}${x.modified ? ' · ' + ago(x.modified) : ''}</small></button>`);
      const fs = b.querySelector('.fshare'); if (fs) fs.addEventListener('click', ev => { ev.stopPropagation(); openShare(cur.root, full, !!writable); });
      b.addEventListener('click', () => {
        if (x.is_dir) { cur.path = full; load(); }
        else if (x.kind === 'image') openApp('photos', { root: cur.root, dir: cur.path, name: x.name, list: images });
        else if (x.kind === 'video' || x.kind === 'audio') openApp('videos', { root: cur.root, path: full, name: x.name });
        else window.open(`/api/files/raw?${q({ root: cur.root, path: full })}`, '_blank', 'noopener');
      });
      grid.appendChild(b);
    }
    if (entries.length > 600) grid.appendChild(el(`<div class="empty">And ${entries.length - 600} more.</div>`));
  }
  async function upload(files) {
    for (const f of files) {
      toast(`Uploading ${f.name}…`);
      try { await call('POST', `/api/files/upload?${q({ root: cur.root, path: cur.path, name: f.name })}`, f, true); }
      catch (e) { toast(`${f.name}: ${e.message}`, true); }
    }
    load();
  }
  root.addEventListener('dragover', e => { if (roots.find(r => r.name === cur.root)?.writable) { e.preventDefault(); grid.classList.add('over'); } });
  root.addEventListener('dragleave', () => grid.classList.remove('over'));
  root.addEventListener('drop', e => { e.preventDefault(); grid.classList.remove('over'); upload([...e.dataTransfer.files]); });
  api.get('/api/files/roots').then(rs => {
    roots = rs;
    if (!rs.length) { grid.innerHTML = '<div class="empty">No folders are shared in prism.toml yet.</div>'; return; }
    places.innerHTML = '';
    rs.forEach(r => { const b = el(`<button class="place" data-root="${esc(r.name)}"><span class="tk" style="background:${r.writable ? '#2E9E62' : '#E5A23A'}">${svg('folder')}</span>${esc(r.name)}<small>${r.writable ? 'can change' : 'look only'}</small></button>`); b.addEventListener('click', () => { cur = { root: r.name, path: '' }; load(); }); places.appendChild(b); });
    if (!cur.root || !rs.some(r => r.name === cur.root)) cur.root = rs[0].name;
    load();
  }).catch(e => { grid.innerHTML = `<div class="empty">${esc(e.message)}</div>`; });
  return root;
}
// A small in-page question: the browser's prompt() isn't allowed everywhere.
function ask(question) {
  return new Promise(res => {
    const s = el(`<div class="sheet on" style="left:50%;top:30%;transform:translateX(-50%);width:min(420px,calc(100% - 32px))"><h3>${esc(question)}</h3><input style="border:3px solid var(--line);border-radius:12px;padding:10px 12px;background:var(--mount);color:var(--on-mount)"><div style="display:flex;gap:8px;justify-content:flex-end"><button class="btn q">Cancel</button><button class="btn">OK</button></div></div>`);
    document.body.appendChild(s); const i = s.querySelector('input'); i.focus();
    const done = v => { s.remove(); res(v); };
    s.querySelector('.btn.q').addEventListener('click', () => done(null));
    s.querySelector('.btn:not(.q)').addEventListener('click', () => done(i.value.trim() || null));
    i.addEventListener('keydown', e => { if (e.key === 'Enter') done(i.value.trim() || null); if (e.key === 'Escape') { e.stopPropagation(); done(null); } });
  });
}

/* ── Share a link: one folder, for a while, doing only what was chosen ─ */
const PROFILES = [
  { id: 'look', n: 'Look', c: '#33BFE2', d: 'See and play what is there. Nothing leaves, nothing changes.', p: { see: true, stream: true } },
  { id: 'take', n: 'Take', c: '#2E9E62', d: 'See, play and download.', p: { see: true, stream: true, down: true } },
  { id: 'drop', n: 'Drop off', c: '#7C5CE0', d: 'Send files in. They see only what they sent.', p: { up: true, own: true }, writes: true },
  { id: 'full', n: 'Work together', c: '#E5A23A', d: 'See, download, send, rename and remove, in this folder.', p: { see: true, stream: true, down: true, up: true, edit: true }, writes: true },
  { id: 'custom', n: 'Customise', c: '#868B94', d: 'Choose exactly what they can do.', p: null },
];
const PERMS = [['see', 'See what is in the folder'], ['own', 'See only what they send'], ['stream', 'Open pictures, music and films'], ['down', 'Download'], ['up', 'Send files in', true], ['edit', 'Rename and remove', true]];
const LASTS = [['1 hour', 3600], ['Until tonight', 'tonight'], ['3 days', 3 * 86400], ['A week', 7 * 86400]];
let sh = null;
function openShare(root, path, writable) {
  sh = { root, path, writable, prof: writable ? 'drop' : 'look', perms: writable ? { up: true, own: true } : { see: true, stream: true }, lasts: 'tonight', once: false, history: [] };
  let host = $('#shareSheet');
  if (!host) {
    document.body.appendChild(el('<div class="scrim" id="shareScrim"></div>'));
    host = el(`<div class="share" id="shareSheet" role="dialog" aria-label="Share a link"></div>`); document.body.appendChild(host);
    $('#shareScrim').addEventListener('click', closeShare);
  }
  api.get('/api/links').then(r => { sh.history = r.history || []; drawShare(); }).catch(() => {});
  drawShare();
  requestAnimationFrame(() => { $('#shareScrim').classList.add('on'); host.classList.add('on'); });
}
function closeShare() { $('#shareScrim')?.classList.remove('on'); $('#shareSheet')?.classList.remove('on'); }
document.addEventListener('keydown', e => { if (e.key === 'Escape' && $('#shareSheet')?.classList.contains('on')) { e.stopImmediatePropagation(); closeShare(); } }, true);
function sayPerms(p) {
  const v = [p.see ? 'see the folder' : p.own ? 'see only what they send' : null, p.stream && 'play', p.down && 'download', p.up && 'send files in', p.edit && 'rename and remove'].filter(Boolean);
  return v.length ? v.join(', ') : 'nothing yet';
}
function drawShare() {
  const h = $('#shareSheet'); if (!h || !sh) return;
  const name = sh.path ? sh.path.split('/').pop() : sh.root;
  h.innerHTML = `<button class="sx" aria-label="Close">${svg('x', 'currentColor', 2.6)}</button>
    <div class="col"><div><h2><span class="tk" style="background:#E5A23A;width:32px;height:32px;border-radius:10px">${svg('folder')}</span>Share ${esc(name)}</h2><p class="sub">A link to this folder alone. Nothing above it or beside it exists for them.</p></div>
      <div class="sgrp"><span>What they can do</span><div class="profiles"></div><div class="custom${sh.prof === 'custom' ? ' on' : ''}"></div></div>
      <div class="sgrp hist-g"${sh.history.length ? '' : ' hidden'}><span>Used before</span><div class="hist"></div></div>
      <div class="sgrp"><span>For how long</span><div class="seg lasts"></div>
        <button class="toggle once" aria-pressed="${sh.once}"><span>Works once<small>The first device to open it keeps it; any other is refused.</small></span><span class="sw"></span></button></div></div>
    <div class="col"><div class="sgrp"><span>What they'll see</span><div class="theirs"></div></div><button class="make">Make the link</button><div class="made"></div></div>`;
  h.querySelector('.sx').addEventListener('click', closeShare);
  const pf = h.querySelector('.profiles');
  PROFILES.forEach(pr => {
    const b = el(`<button class="prof" aria-pressed="${sh.prof === pr.id}"><b><i style="--c:${pr.c}"></i>${pr.n}</b><span>${pr.writes && !sh.writable ? 'Not for this folder: PRISM can only look at it.' : pr.d}</span></button>`);
    if (pr.writes && !sh.writable) b.disabled = true;
    b.addEventListener('click', () => { sh.prof = pr.id; if (pr.p) sh.perms = { ...pr.p }; drawShare(); });
    pf.appendChild(b);
  });
  const cu = h.querySelector('.custom');
  PERMS.forEach(([k, n, w]) => {
    const b = el(`<button class="toggle" aria-pressed="${!!sh.perms[k]}"><span>${n}</span><span class="sw"></span></button>`);
    if (w && !sh.writable) b.disabled = true;
    b.addEventListener('click', () => { sh.perms[k] = !sh.perms[k]; if (k === 'see' && sh.perms.see) sh.perms.own = false; if (k === 'own' && sh.perms.own) sh.perms.see = false; drawShare(); });
    cu.appendChild(b);
  });
  const hi = h.querySelector('.hist');
  sh.history.filter(p => sh.writable || !(p.up || p.edit)).slice(0, 6).forEach(p => {
    const b = el(`<button>${esc(sayPerms(p).replace(/^./, c => c.toUpperCase()))}</button>`);
    b.addEventListener('click', () => { sh.prof = 'custom'; sh.perms = { ...p }; drawShare(); });
    hi.appendChild(b);
  });
  const ls = h.querySelector('.lasts');
  LASTS.forEach(([n, v]) => { const b = el(`<button aria-pressed="${sh.lasts === v}">${n}</button>`); b.addEventListener('click', () => { sh.lasts = v; drawShare(); }); ls.appendChild(b); });
  h.querySelector('.once').addEventListener('click', () => { sh.once = !sh.once; drawShare(); });
  const p = sh.perms;
  h.querySelector('.theirs').innerHTML = `<div class="bar2"><span class="lamp" style="background:var(--live)"></span><b>${esc(name)}</b><span>· shared through PRISM</span></div>` +
    (p.up ? `<div class="only">${svg('up', '#7C5CE0', 2.4)}Send files here<br><span>any kind, any size</span></div>` : '') +
    (p.see || p.own ? `<div class="f">${svg('doc', 'var(--ink-soft)', 2.2).replace('<svg', '<svg style="width:20px;height:20px"')}${p.see ? 'everything in the folder' : 'only what they sent'}<span>${p.down ? 'downloadable' : p.stream ? 'to open' : 'listed'}</span></div>` : '') +
    `<p class="gone">They can ${esc(sayPerms(p))}. The rest of this PC doesn't exist for them: no other folders, no Apps, no PC.</p>`;
  h.querySelector('.make').addEventListener('click', async ev => {
    const btn = ev.currentTarget; btn.disabled = true;
    let secs = sh.lasts;
    if (secs === 'tonight') { const t = new Date(); t.setHours(23, 59, 0, 0); secs = Math.max(3600, Math.round((t - Date.now()) / 1000)); }
    try {
      const r = await api.post('/api/links', { root: sh.root, path: sh.path, perms: sh.perms, lasts_secs: secs, once: sh.once, custom: sh.prof === 'custom' });
      const url = location.origin + r.url_path;
      btn.hidden = true;
      const m = h.querySelector('.made');
      m.innerHTML = `<div class="linkrow"><code>${esc(url)}</code><button class="btn">Copy</button></div><p class="sub">${esc(r.says.replace(/^./, c => c.toUpperCase()))}. It works wherever this PRISM is reachable; take it back any time in Activity, where every visit is sealed in the record.</p>`;
      m.querySelector('.btn').addEventListener('click', e => { navigator.clipboard?.writeText(url).then(() => { e.target.textContent = 'Copied'; }, () => { const c = m.querySelector('code'); const rg = document.createRange(); rg.selectNodeContents(c); getSelection().removeAllRanges(); getSelection().addRange(rg); }); });
    } catch (e) { btn.disabled = false; toast(e.message, true); }
  });
}

/* ── Photos ──────────────────────────────────────────────────────────── */
function photosApp(args) {
  const root = el(`<div class="viewer"><div class="photo"><img alt=""><button class="nav prev" aria-label="Previous">${svg('left')}</button><button class="nav next" aria-label="Next">${svg('right')}</button><div class="info"></div></div><div class="film"></div></div>`);
  if (!args?.root) { root.innerHTML = `<div class="empty">Open a picture from Files, or from Home.</div>`; return root; }
  let list = args.list && args.list.length ? args.list : [args.name], at = Math.max(0, list.indexOf(args.name));
  const img = root.querySelector('.photo img'), film = root.querySelector('.film');
  const pathOf = n => args.dir ? `${args.dir}/${n}` : n;
  function show(i) {
    at = (i + list.length) % list.length; const n = list[at];
    try { localStorage.setItem('prism.lastPhoto', JSON.stringify({ root: args.root, dir: args.dir, name: n, list: list.length < 400 ? list : [n] })); } catch (e) {}
    img.style.opacity = 0;
    const next = new Image(); next.onload = () => { img.src = next.src; img.style.opacity = 1; }; next.src = `/api/files/raw?${q({ root: args.root, path: pathOf(n) })}`;
    root.querySelector('.info').innerHTML = `<b>${esc(n)}</b><span>${esc(args.root)}${args.dir ? ' › ' + esc(args.dir) : ''} · ${at + 1} of ${list.length}</span>`;
    film.querySelectorAll('button').forEach((b, j) => b.setAttribute('aria-pressed', String(j === at)));
    film.children[at]?.scrollIntoView({ block: 'nearest', inline: 'center', behavior: reduce ? 'auto' : 'smooth' });
  }
  list.forEach((n, j) => { const b = el(`<button aria-label="${esc(n)}"><img loading="lazy" alt="" src="/api/files/thumb?${q({ root: args.root, path: pathOf(n) })}"></button>`); b.addEventListener('click', () => show(j)); film.appendChild(b); });
  root.querySelector('.prev').addEventListener('click', () => show(at - 1));
  root.querySelector('.next').addEventListener('click', () => show(at + 1));
  root.tabIndex = 0;
  root.addEventListener('keydown', e => { if (e.key === 'ArrowRight') show(at + 1); if (e.key === 'ArrowLeft') show(at - 1); });
  if (list.length < 2) { film.hidden = true; root.querySelectorAll('.nav').forEach(n => n.hidden = true); }
  show(at);
  return root;
}

/* ── Videos ──────────────────────────────────────────────────────────── */
function videosApp(args) {
  const root = el(`<div class="player"><video controls playsinline preload="metadata"></video><p class="meta"></p></div>`);
  if (!args?.root) { root.innerHTML = `<div class="empty">Open a film or a song from Files.</div>`; return root; }
  const v = root.querySelector('video'), meta = root.querySelector('.meta');
  meta.textContent = `${args.name} · checking how to play it…`;
  api.get(`/api/files/media?${q({ root: args.root, path: args.path })}`).then(info => {
    const k = info.playability?.kind;
    if (k === 'direct') { v.src = `/api/files/raw?${q({ root: args.root, path: args.path })}`; meta.textContent = `${args.name} · played as it is`; }
    else if (k === 'remux' || k === 'transcode') { v.src = `/api/files/stream?${q({ root: args.root, path: args.path })}`; meta.textContent = `${args.name} · ${k === 'remux' ? 'rewrapped' : 'converted on this PC'} as it plays, for this browser${k === 'transcode' ? ' (seeking starts it again from there)' : ''}`; }
    else { meta.textContent = `${args.name} · this browser can't play it, and it can't be converted.`; }
  }).catch(e => { meta.textContent = `${args.name} · ${e.message}`; });
  root.__close = () => { v.pause(); v.removeAttribute('src'); v.load(); };
  return root;
}

/* ── Services ────────────────────────────────────────────────────────── */
function servicesApp(args, body) {
  const root = el(`<div class="list"></div>`);
  async function draw() {
    await loadServices();
    let found = []; try { found = await api.get('/api/discover'); } catch (e) {}
    root.innerHTML = '';
    if (!facets.length) root.appendChild(el(`<div class="empty">No services yet. Add one in profile.toml, or from what PRISM finds listening, below.</div>`));
    facets.forEach(f => {
      const pol = /face-stream/.test(f.command || '');
      const tag = { running: ['run', 'Running'], foreign: ['out', 'Not PRISM’s'], stopped: ['off', 'Stopped'], failed: ['off', 'Failed'] }[f.state] || ['off', f.state];
      const lim = [f.limits?.memory_max && `at most ${f.limits.memory_max}`, f.limits?.swap_max != null && (f.limits.swap_max === '0' ? 'no swap' : `up to ${f.limits.swap_max} in swap`)].filter(Boolean).join(', ');
      const r = el(`<div class="row"><span class="ic" style="--c:${pol ? '#7C5CE0' : '#2E9E62'}">${esc((f.name || f.id)[0].toUpperCase())}</span><b>${esc(f.name)}${pol ? ' · POLARIS' : ''}</b><span class="meta">${f.state === 'running' && f.memory_mib != null ? mib(f.memory_mib) + ' now' : ''}${lim ? (f.state === 'running' ? ' · ' : '') + lim : ''}${f.state === 'foreign' ? 'Started outside PRISM, so PRISM only asks it, never stops it' : ''}${!f.available && f.unavailable_because ? esc(f.unavailable_because) : ''}</span><span class="state"><span class="tag ${tag[0]}">${tag[1]}</span></span></div>`);
      const st = r.querySelector('.state');
      if (f.expose && (f.state === 'running' || f.state === 'foreign')) { const o = el(`<button class="btn">Open</button>`); o.addEventListener('click', () => openApp('web:' + f.id)); st.appendChild(o); }
      if (f.state === 'running') { const s = el(`<button class="btn q">Stop</button>`); s.addEventListener('click', async () => { s.disabled = true; try { await api.post(`/api/facets/${f.id}/stop`); toast(`${f.name} stopped.`); } catch (e) { toast(e.message, true); } draw(); }); st.appendChild(s); }
      else if ((f.state === 'stopped' || f.state === 'failed') && f.available && !f.pty) { const s = el(`<button class="btn q">Start</button>`); s.addEventListener('click', async () => { s.disabled = true; try { await api.post(`/api/facets/${f.id}/start`); toast(`${f.name} is starting.`); } catch (e) { toast(e.message, true); } setTimeout(draw, 1500); }); st.appendChild(s); }
      root.appendChild(r);
    });
    const known = new Set(facets.map(f => f.expose?.port).filter(Boolean));
    const loose = found.filter(d => d.http && !d.known && !known.has(d.port));
    if (loose.length) {
      root.appendChild(el(`<div class="group">Found listening on this PC</div>`));
      loose.forEach(d => root.appendChild(el(`<div class="row"><span class="ic">${esc((d.title || d.process || '?')[0].toUpperCase())}</span><b>${esc(d.title || d.process)}</b><span class="meta">port ${d.port} · ${esc(d.process)} · add it to profile.toml to open it through PRISM</span><span class="state"></span></div>`)));
    }
    renderHosted();
  }
  draw(); const t = setInterval(() => { if (root.isConnected) draw(); }, 8000);
  body.__close = () => clearInterval(t);
  return root;
}

/* ── Vitals ──────────────────────────────────────────────────────────── */
let vitals = null; const spark = [];
async function pollVitals() {
  if (!signedIn) return;
  try { vitals = await api.get('/api/vitals'); spark.push(vitals.honest_headroom_mib); while (spark.length > 60) spark.shift(); renderVitalsWidgets(); renderVitalsPane(); } catch (e) {}
  setTimeout(pollVitals, document.hidden ? 10000 : 2000);
}
const TIER_WORD = { green: "All's well", amber: 'Getting tight', red: 'Under pressure', black: 'Stepping in' };
const TIER_SAY = { green: 'Nothing is using more than its share.', amber: 'Memory is getting short. PRISM looks four times a second and works out who is growing.', red: 'PRISM is asking the service that is growing to give memory back.', black: 'PRISM is stopping the service responsible, if PRISM started it, and nothing else.' };
function renderVitalsWidgets() {
  if (!vitals) return;
  $('#lamp').dataset.t = vitals.tier; const l2 = $('#lamp2'); if (l2) l2.dataset.t = vitals.tier;
  const w = $('#wellWord'); if (w) w.textContent = TIER_WORD[vitals.tier] || vitals.tier;
  const line = $('#wellLine'); if (line) line.textContent = `${mib(vitals.honest_headroom_mib)} free in practice, of ${mib(vitals.total_mib)}.`;
  const L = $('#sparkL'); if (L && spark.length > 1) {
    const max = vitals.total_mib || 1;
    const d = spark.map((f, i) => `${i ? 'L' : 'M'}${(i * 280 / (spark.length - 1)).toFixed(1)} ${(38 - f / max * 34).toFixed(1)}`).join(' ');
    L.setAttribute('d', d); $('#sparkA').setAttribute('d', d + ' L280 40 L0 40 Z');
  }
}
function vitalsApp() {
  return el(`<div class="two vitals"><div class="vbig">
    <svg viewBox="0 0 230 230" aria-label="Free memory, in practice"><circle class="dtrack" cx="115" cy="115" r="92" transform="rotate(135 115 115)" stroke-dasharray="433.5 578"/><circle class="dfill" cx="115" cy="115" r="92" transform="rotate(135 115 115)" stroke-dasharray="433.5 578" stroke-dashoffset="433.5"/>
    <text x="115" y="118" text-anchor="middle" font-size="52" font-weight="700" class="num dnum" style="fill:currentColor;font-family:var(--round)">…</text><text x="115" y="148" text-anchor="middle" font-size="16" style="fill:var(--on-mount-soft);font-family:var(--round)">GiB free in practice</text></svg>
    <p class="say">Reading this PC…</p></div>
    <div class="list vside">
      <div class="meter"><div class="t2">Memory<span class="num ml"></span></div><div class="mtrack"><i class="mu" style="background:#77756E"></i><i class="mz" style="background:var(--warn)"></i><i class="mf" style="background:var(--good)"></i></div><div class="key"><span style="--c:#77756E">in use</span><span style="--c:var(--warn)">what swap really costs</span><span style="--c:var(--good)">free in practice</span></div></div>
      <div class="meter gpu"><div class="t2">Graphics card<span class="num gl"></span></div><div class="mtrack"><i class="gf" style="background:#77756E"></i><i class="go" style="background:var(--starlight)"></i></div><div class="key"><span style="--c:#77756E">the desktop and games</span><span style="--c:var(--starlight)">services PRISM runs</span></div></div>
      <div class="meter disk"><div class="t2">Disk<span class="num dl"></span></div><div class="mtrack"><i class="du" style="background:#77756E"></i><i class="df" style="background:var(--good)"></i></div></div>
      <div class="card" style="box-shadow:none;background:var(--mount);color:var(--on-mount)"><span class="k" style="color:var(--on-mount-soft)">Under pressure</span><p style="color:var(--on-mount-soft)">Amber: PRISM looks four times a second. Red: it asks the service that is growing to give memory back. Black: it stops that service, only if PRISM started it, and checks 15 seconds later that it helped. Every step is in Activity.</p></div>
    </div></div>`);
}
function renderVitalsPane() {
  const r = paneEl('vitals'); if (!r || !vitals) return;
  const v = vitals, T = v.total_mib || 1, free = v.honest_headroom_mib;
  const dl = r.querySelector('.dfill'); dl.setAttribute('stroke-dashoffset', String(433.5 * (1 - Math.min(1, free / T)))); dl.style.stroke = { green: 'var(--good)', amber: 'var(--warn)' }[v.tier] || 'var(--bad)';
  r.querySelector('.dnum').textContent = (free / 1024).toFixed(1);
  r.querySelector('.say').textContent = `${TIER_WORD[v.tier]}. ${TIER_SAY[v.tier]}`;
  const used = Math.max(0, T - v.available_mib), z = v.zram_cost_mib || 0;
  r.querySelector('.mu').style.width = used / T * 100 + '%'; r.querySelector('.mz').style.width = z / T * 100 + '%'; r.querySelector('.mf').style.width = Math.max(0, free / T * 100) + '%';
  r.querySelector('.ml').textContent = `${mib(used)} of ${mib(T)} in use`;
  const g = r.querySelector('.gpu');
  if (v.vram) { g.hidden = false; const G = v.vram.total_mib || 1; r.querySelector('.gf').style.width = v.vram.foreign_mib / G * 100 + '%'; r.querySelector('.go').style.width = v.vram.ours_mib / G * 100 + '%'; r.querySelector('.gl').textContent = `${mib(v.vram.ours_mib)} of ${mib(G)} PRISM's to give back`; }
  else g.hidden = true;
  const dk = r.querySelector('.disk');
  if (v.disk) { dk.hidden = false; const D = v.disk.total_mib || 1; r.querySelector('.du').style.width = (D - v.disk.free_mib) / D * 100 + '%'; r.querySelector('.df').style.width = v.disk.free_mib / D * 100 + '%'; r.querySelector('.dl').textContent = `${mib(v.disk.free_mib)} free`; }
  else dk.hidden = true;
}

/* ── Activity: what happened, and who came in ────────────────────────── */
function activityApp(args, body) {
  const root = el(`<div class="two"><div class="list"><h3 style="font-size:17px">What happened</h3><div class="list evs"></div></div>
    <div class="list"><h3 style="font-size:17px">Who came in</h3><div class="seal"></div><div class="list links"></div><div class="chips"></div><div class="list acc"></div></div></div>`);
  let who = 'all';
  const KIND = { info: ['obs', 'Seen'], warn: ['warn', 'Needs a look'], action: ['act', 'PRISM did'], error: ['err', 'Failed'] };
  const ACC = { signedin: ['ver', 'Signed in'], refused: ['err', 'Refused'], changed: ['act', 'Changed'], signedout: ['obs', 'Signed out'] };
  async function draw() {
    try {
      const evs = await api.get('/api/events');
      root.querySelector('.evs').innerHTML = evs.slice(0, 120).map(e => { const [c, w] = KIND[e.level] || ['obs', e.level]; return `<div class="ev"><span class="when">${clock(e.unix)}</span><b><span class="tag ${c}">${w}</span> ${esc(e.message)}</b>${e.detail ? `<p>${esc(e.detail)}</p>` : `<p>${esc(e.source)} · ${ago(e.unix)}</p>`}</div>`; }).join('') || '<div class="empty">Nothing yet.</div>';
    } catch (e) {}
    try {
      const a = await api.get(`/api/access?${q({ limit: 200, ...(who !== 'all' ? { who } : {}) })}`);
      const v = a.verified || {};
      const seal = root.querySelector('.seal');
      seal.classList.toggle('broken', !!v.broken_at);
      seal.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="${v.broken_at ? 'var(--bad)' : 'var(--good)'}" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3l7 3v5c0 5-3 8-7 10-4-2-7-5-7-10V6z"/>${v.broken_at ? '<path d="M12 8v5M12 16v.5"/>' : '<path d="M8.5 12l2.5 2.5 4.5-5"/>'}</svg><span>${v.broken_at ? `<b>The record was changed.</b> Its chain breaks at entry ${v.broken_at}: something edited or removed what came after.` : `<b>Sealed.</b> Each entry carries the fingerprint of the one before, so nothing can be changed or removed without showing. ${v.entries || 0} entries, intact.`}</span>`;
      try {
        const L = await api.get('/api/links');
        const live = (L.links || []).filter(l => l.live);
        const box = root.querySelector('.links'); box.innerHTML = live.length ? '<div class="group">Links that still work</div>' : '';
        live.forEach(l => {
          const r = el(`<div class="row"><span class="ic" style="--c:#33BFE2">${svg('link', '#fff', 2.4)}</span><b>${esc(l.label)}</b><span class="meta">${esc(l.says)} · ${l.once ? 'one device · ' : ''}${l.visits} visit${l.visits === 1 ? '' : 's'}${l.received ? ` · ${l.received} received` : ''} · ends ${esc(new Date(l.expires * 1000).toLocaleString(undefined, { weekday: 'short', hour: '2-digit', minute: '2-digit' }))}</span><span class="state"><button class="btn q">Take back</button></span></div>`);
          r.querySelector('button').addEventListener('click', async () => { try { await api.post(`/api/links/${l.id}/revoke`); toast(`"${l.label}" no longer opens.`); draw(); } catch (e) { toast(e.message, true); } });
          box.appendChild(r);
        });
      } catch (e) {}
      const people = new Set(['all']); (a.entries || []).forEach(e => { people.add(e.who); if (e.for_account) people.add(e.for_account); });
      const chips = root.querySelector('.chips'); chips.innerHTML = '';
      [...people].forEach(p => { const b = el(`<button aria-pressed="${p === who}">${esc({ all: 'Everyone', admin: 'You (PRISM)', polaris: 'POLARIS', unknown: 'Refused' }[p] || (p.startsWith('link:') ? 'Link: ' + p.slice(5) : p))}</button>`); b.addEventListener('click', () => { who = p; draw(); }); chips.appendChild(b); });
      root.querySelector('.acc').innerHTML = (a.entries || []).map(e => { const [c, w] = ACC[e.kind] || ['obs', e.kind]; const name = e.who === 'admin' ? 'You' : e.who === 'polaris' ? `POLARIS${e.for_account ? ', for ' + esc(e.for_account) : ''}` : e.who === 'unknown' ? 'Someone' : e.who.startsWith('link:') ? `Through the link "${esc(e.who.slice(5))}"` : esc(e.who); return `<div class="ev"><span class="when">${clock(e.unix)}</span><b><span class="tag ${c}">${w}</span> ${name}</b><p>${esc(e.what)} · ${esc(e.how)} · from ${esc(e.from)} · ${ago(e.unix)}</p></div>`; }).join('') || '<div class="empty">Nobody yet.</div>';
    } catch (e) {}
  }
  draw(); const t = setInterval(() => { if (root.isConnected) draw(); }, 6000);
  body.__close = () => clearInterval(t);
  return root;
}

/* ── Terminal: a PTY on this PC, surviving the connection (ADR 0003) ─── */
function terminalApp(args, body, pane) {
  const host = el(`<div class="xt"></div>`);
  if (typeof Terminal === 'undefined') { host.innerHTML = '<div class="empty" style="color:#DDE7F5">The terminal could not load.</div>'; return host; }
  const term = new Terminal({ fontFamily: '"PrismTerm", ui-monospace, Menlo, monospace', fontSize: 13.5, lineHeight: 1.2, cursorBlink: true, scrollback: 5000, allowProposedApi: true,
    theme: { background: '#070A11', foreground: '#DDE7F5', cursor: '#96ECFA', selectionBackground: 'rgba(86,210,228,.20)', black: '#0B1018', red: '#fb7185', green: '#4ade80', yellow: '#fbbf24', blue: '#60a5fa', magenta: '#c084fc', cyan: '#67e8f9', white: '#cfd6df', brightBlack: '#61738f', brightRed: '#fda4af', brightGreen: '#86efac', brightYellow: '#fcd34d', brightBlue: '#93c5fd', brightMagenta: '#d8b4fe', brightCyan: '#a5f3fc', brightWhite: '#DDE7F5' } });
  const fit = new FitAddon.FitAddon(); term.loadAddon(fit);
  let ws = null, sid = args?.sid || null, closed = false;
  async function start() {
    try { await document.fonts.load('13px PrismTerm'); } catch (e) {}
    term.open(host); requestAnimationFrame(() => { try { fit.fit(); } catch (e) {} });
    if (sid) { try { const l = await api.get('/api/term'); if (!l.sessions.some(s => s.id === sid)) sid = null; } catch (e) {} }
    if (!sid) {
      try { sid = (await api.post('/api/term', { rows: term.rows, cols: term.cols, title: 'Terminal' })).id; pane.args = { sid }; persist(); }
      catch (e) { term.writeln(`\x1b[31m${e.message}\x1b[0m`); return; }
    }
    let attempt = 0, again = false;
    const connect = () => {
      if (closed) return;
      ws = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/term/${sid}/attach`); ws.binaryType = 'arraybuffer';
      let replayed = false;
      ws.onopen = () => { attempt = 0; try { term.focus(); } catch (e) {} };
      ws.onmessage = e => { if (!replayed) { replayed = true; if (again) term.reset(); } term.write(new Uint8Array(e.data)); };
      ws.onclose = () => { if (closed) return; attempt++; again = true; setTimeout(connect, Math.min(500 * 2 ** (attempt - 1), 10000)); };
      ws.onerror = () => { try { ws.close(); } catch (e) {} };
    };
    connect();
    term.onData(d => { if (ws && ws.readyState === 1) ws.send(new TextEncoder().encode(d)); });
    term.onResize(({ rows, cols }) => { if (sid) api.post(`/api/term/${sid}/resize`, { rows, cols }).catch(() => {}); });
  }
  const ro = new ResizeObserver(() => { try { fit.fit(); } catch (e) {} });
  requestAnimationFrame(() => { ro.observe(host); start(); });
  host.addEventListener('pointerdown', () => { try { term.focus(); } catch (e) {} });
  // Closing the pane detaches; the session and what it runs carry on.
  body.__close = () => { closed = true; if (ws) ws.close(); ro.disconnect(); };
  return host;
}

/* ── A service's own page, through PRISM ─────────────────────────────── */
function webApp(args, body, pane) {
  const id = pane.id.slice(4);
  return el(`<iframe class="webpane" src="/facet/${encodeURIComponent(id)}/" title="${esc(id)}" allow="fullscreen; clipboard-read; clipboard-write"></iframe>`);
}

boot();
