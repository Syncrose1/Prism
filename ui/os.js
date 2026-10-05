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
  search: '<circle cx="11" cy="11" r="6.5"/><path d="M16 16l4.5 4.5"/>',
  pin: '<path d="M9 4h6l-1 6 3 3H7l3-3z"/><path d="M12 13v7"/>',
  pen: '<path d="M4 20l1-4L16 5l3 3L8 19z"/><path d="M14 7l3 3"/>',
  trash: '<path d="M5 7h14M10 7V4h4v3M7 7l1 13h8l1-13"/>',
  down: '<path d="M12 4v11M7 10l5 5 5-5M5 19h14"/>',
  down2: '<path d="M7 10l5 5 5-5"/>',
  stop: '<rect x="6" y="6" width="12" height="12" rx="2"/>',
  again: '<path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5"/>',
  loop: '<path d="M4 11V9a3 3 0 0 1 3-3h12l-3-3M20 13v2a3 3 0 0 1-3 3H5l3 3"/>',
  vol: '<path d="M4 9h4l5-4v14l-5-4H4z"/><path d="M16 9a4 4 0 0 1 0 6M18.5 6.5a8 8 0 0 1 0 11"/>',
  mute: '<path d="M4 9h4l5-4v14l-5-4H4z"/><path d="M16 9l5 6M21 9l-5 6"/>',
  pause: '<path d="M8 5v14M16 5v14"/>',
  full: '<path d="M4 9V4h5M20 9V4h-5M4 15v5h5M20 15v5h-5"/>',
  pip: '<rect x="3" y="5" width="18" height="14" rx="2"/><rect x="12" y="11" width="7" height="6" rx="1"/>',
  chat: '<path d="M4 6a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H10l-5 4v-4H6a2 2 0 0 1-2-2z"/>',
  send: '<path d="M4 12l16-8-6 16-2-6z"/>',
  check: '<path d="M5 12.5l4.5 4.5L19 7.5"/>',
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
// The sky is drawn at one pixel per CSS pixel and thirty frames a second:
// soft stars need neither more, and the PC (often a remote one, on a
// laptop's battery) gets the rest. It stops when hidden behind windows.
const dpr = () => 1;
// The sky as POLARIS draws it (shell Field.kt): a tile wider than any
// screen, stars scattered from a fixed seed (the same sky everywhere), and
// near neighbours sometimes joined into constellations: at most three lines
// at a star, each stopping short of both ends. It all drifts as one, so the
// shapes hold.
const SKY_W = 2700, SKY_H = 1000;
const { stars, joins } = (() => {
  let seed = 0x5eed; const rnd = () => { seed |= 0; seed = seed + 0x6D2B79F5 | 0; let t = Math.imul(seed ^ seed >>> 15, 1 | seed); t = t + Math.imul(t ^ t >>> 7, 61 | t) ^ t; return ((t ^ t >>> 14) >>> 0) / 4294967296; };
  const st = [];
  for (let n = 0; st.length < 70 && n < 3000; n++) {
    const c = { x: rnd() * SKY_W, y: 30 + rnd() * (SKY_H - 60), r: 2 + rnd() * 4.5, tw: rnd() * 6.28 };
    if (st.every(o => Math.hypot(o.x - c.x, o.y - c.y) > (o.r + c.r) * 9)) st.push(c);
  }
  const jn = [], deg = st.map(() => 0);
  st.forEach((a, i) => {
    if (rnd() > .62) return;
    const want = rnd() < .55 ? 1 : 2; let made = 0;
    const near = st.map((b, j) => [j, Math.hypot(b.x - a.x, b.y - a.y)]).filter(([j, d]) => j !== i && d < 230).sort((p, q) => p[1] - q[1]);
    for (const [j] of near) {
      if (made >= want) break;
      if (deg[i] >= 3 || deg[j] >= 3 || jn.some(([p, q]) => (p === i && q === j) || (p === j && q === i))) continue;
      jn.push([i, j]); deg[i]++; deg[j]++; made++;
    }
  });
  return { stars: st, joins: jn };
})();
let gather = 0, skyOn = true, skyColour = '#33BFE2', skyRaf = 0, skyLast = 0;
function size(c) { c.width = innerWidth * dpr(); c.height = innerHeight * dpr(); c.getContext('2d').setTransform(dpr(), 0, 0, dpr(), 0, 0); }
addEventListener('resize', () => { size(sky); size($('#veil')); });
size(sky); size($('#veil'));
const readSky = () => { skyColour = getComputedStyle(document.documentElement).getPropertyValue('--sky').trim() || '#33BFE2'; };
readSky();
new MutationObserver(readSky).observe(document.documentElement, { attributes: true, attributeFilter: ['data-theme', 'class', 'style'] });
matchMedia('(prefers-color-scheme: dark)').addEventListener?.('change', readSky);
function starPath(g, x, y, r) { const b = r * .34; g.beginPath(); g.moveTo(x, y - r); g.quadraticCurveTo(x + b, y - b, x + r, y); g.quadraticCurveTo(x + b, y + b, x, y + r); g.quadraticCurveTo(x - b, y + b, x - r, y); g.quadraticCurveTo(x - b, y - b, x, y - r); g.closePath(); }
function drawSky(t) {
  skyRaf = 0;
  if (!skyOn || document.hidden) { sg.clearRect(0, 0, innerWidth, innerHeight); return; }
  if (t - skyLast >= 32) {
    skyLast = t;
    sg.clearRect(0, 0, innerWidth, innerHeight);
    const W = innerWidth, k = innerHeight / SKY_H, off = (t * .006) % SKY_W, g = Math.min(1, gather);
    const xs = x => { const b = ((x - off) % SKY_W + SKY_W) % SKY_W; return b > W + 260 ? b - SKY_W : b; };
    sg.strokeStyle = skyColour; sg.lineWidth = 1.5; sg.lineCap = 'round'; sg.globalAlpha = .2 * g;
    sg.beginPath();
    for (const [i, j] of joins) {
      const a = stars[i], b = stars[j], ax = xs(a.x), bx = ax + (b.x - a.x);
      if (Math.max(ax, bx) < -20 || Math.min(ax, bx) > W + 20) continue;
      const ay = a.y * k, by = b.y * k, dx = bx - ax, dy = by - ay, len = Math.hypot(dx, dy), ga = (a.r + 6) / len, gb = (b.r + 6) / len;
      sg.moveTo(ax + dx * ga, ay + dy * ga); sg.lineTo(bx - dx * gb, by - dy * gb);
    }
    sg.stroke();
    sg.fillStyle = skyColour;
    for (const s of stars) { const x = xs(s.x); if (x < -10 || x > W + 10) continue; sg.globalAlpha = .45 * (.6 + .4 * Math.sin(t / 1400 + s.tw)) * g; starPath(sg, x, s.y * k, s.r); sg.fill(); }
    sg.globalAlpha = 1;
  }
  if (!reduce) skyRaf = requestAnimationFrame(drawSky);
}
const wakeSky = () => { if (!skyRaf) skyRaf = requestAnimationFrame(drawSky); };
document.addEventListener('visibilitychange', wakeSky);
wakeSky();
function tween(ms, f, done) { const t0 = performance.now(); (function s(n) { const k = Math.min(1, (n - t0) / (reduce ? 1 : ms)); f(k); k < 1 ? requestAnimationFrame(s) : done && done(); })(t0); }
// The chevron is the transition itself: what's ahead of it (sign-in) is
// wiped away as it passes, and what's behind it (Home, or the Apps left
// open) is already there. Both scenes are drawn during the sweep, each
// clipped to its side of the band; the stars carry on underneath.
let sweepOn = false;
async function chevronSweep(mid, done) {
  const veil = $('#veil'), vg = veil.getContext('2d'), si = $('#s-signin'), top = $('#top');
  if (reduce) { await mid(); done && done(); return; }
  if (sweepOn) return; sweepOn = true;
  si.classList.add('leaving'); document.body.classList.add('sweeping'); top.classList.add('hold');
  // Whatever enter() shows stays hidden until the chevron reaches it.
  $$('.scene').forEach(x => { if (x !== si) x.style.clipPath = 'inset(100% 0 0 0)'; });
  await mid();
  const W = innerWidth, H = innerHeight, A = H * .45, slope = A / (W * .6);
  const css = getComputedStyle(document.documentElement); const c1 = css.getPropertyValue('--veil-1').trim(), c2 = css.getPropertyValue('--veil-2').trim();
  const to = $$('.scene.on').find(x => x !== si);
  const e = x => x < .5 ? 4 * x * x * x : 1 - Math.pow(-2 * x + 2, 3) / 2;
  const band = (y0, y1, col) => { vg.fillStyle = col; vg.beginPath(); vg.moveTo(-W * .1, y0 + A); vg.lineTo(W / 2, y0); vg.lineTo(W * 1.1, y0 + A); vg.lineTo(W * 1.1, y1 + A); vg.lineTo(W / 2, y1); vg.lineTo(-W * .1, y1 + A); vg.closePath(); vg.fill(); };
  const edge = y => y + slope * W / 2;   // the chevron's height at the screen's edges
  tween(1250, k => {
    const e1 = H * (1.02 - 2.5 * e(k)), e2 = e1 + H * .12, e3 = e2 + H * .2;
    vg.clearRect(0, 0, W, H);
    band(e1, e2 + 1, c1); band(e2, e3, c2);
    si.style.clipPath = `polygon(0 0, ${W}px 0, ${W}px ${edge(e1) + 1}px, ${W / 2}px ${e1 + 1}px, 0 ${edge(e1) + 1}px)`;
    if (to) to.style.clipPath = `polygon(0 ${edge(e3) - 1}px, ${W / 2}px ${e3 - 1}px, ${W}px ${edge(e3) - 1}px, ${W}px ${H}px, 0 ${H}px)`;
    if (edge(e3) < 40) top.classList.remove('hold');
  }, () => {
    vg.clearRect(0, 0, W, H);
    si.classList.remove('leaving'); $$('.scene').forEach(x => x.style.clipPath = '');
    document.body.classList.remove('sweeping'); top.classList.remove('hold'); sweepOn = false;
    done && done();
  });
}

/* ── State, kept on the host ─────────────────────────────────────────── */
const state = { scene: 'signin', wm: 'float', panes: [], focus: null, pos: {}, host: 'this PC', role: 'owner', me: null, people: [], pins: [], recent: [] };
const GUEST_APPS = ['files', 'photos', 'videos'];
const allowed = id => state.role !== 'guest' || GUEST_APPS.includes(id);
let signedIn = false, saveTimer = null, workspace = {};
function persist() {
  // A guest's place isn't the PC's layout: it stays on their own device.
  if (state.role === 'guest') return;
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    const os = { v: 1, wm: state.wm, panes: state.panes.map(p => ({ id: p.id, args: p.args })), focus: state.focus, pos: state.pos, split: state.split, pins: state.pins, recent: state.recent };
    workspace = { ...(workspace || {}), os };
    api.put('/api/workspace', workspace).catch(() => {});
  }, 600);
}
async function restore() {
  if (state.role === 'guest') {
    try { state.pins = JSON.parse(localStorage.getItem('prism.pins') || '[]'); state.recent = JSON.parse(localStorage.getItem('prism.recent') || '[]'); } catch (e) {}
    return null;
  }
  try { workspace = (await api.get('/api/workspace')) || {}; } catch (e) { workspace = {}; }
  const os = workspace.os;
  if (os) { state.wm = os.wm === 'tile' ? 'tile' : 'float'; state.pos = os.pos || {}; if (os.split) state.split = os.split; state.pins = os.pins || []; state.recent = os.recent || []; return os; }
  return null;
}

/* ── Sign-in ─────────────────────────────────────────────────────────── */
const lockup = $('#lockup');
let seq = 0, prompt = 'code';
function dash(e, k) { const L = e.getTotalLength(); e.style.strokeDasharray = L; e.style.strokeDashoffset = L * (1 - k); }
function showScene(id) { $$('.scene').forEach(s => s.classList.toggle('on', s.id === 's-' + id)); state.scene = id; skyOn = id !== 'work'; wakeSky(); renderStrip(); }
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
  signedIn = false; closeSheet('#menu'); if (solisSide) { solisSide.remove(); solisSide = null; } playSignin();
});

/* ── Top bar ─────────────────────────────────────────────────────────── */
setInterval(() => { $('#clock').textContent = new Date().toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', hour12: false }); }, 1000);
$('#clock').textContent = new Date().toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', hour12: false });
$('#device').addEventListener('click', () => { if (allowed('vitals')) openApp('vitals'); });
$('#me').addEventListener('click', () => toggleSheet('#menu'));
// Solis beside everything, as POLARIS's Alt+Z: a column that slides in on
// the right and keeps its conversation while it's away.
let solisSide = null;
function toggleSolis(on) {
  if (!allowed('solis')) return;
  if (!solisSide) {
    solisSide = el(`<aside class="side" aria-label="Solis"><div class="sidehead">${svg('star')}<b>Solis</b><small>Alt+Z</small><button class="b" aria-label="Put Solis away">${svg('x', 'currentColor', 2.6)}</button></div><div class="sidebody"></div></aside>`);
    const holder = solisSide.querySelector('.sidebody');
    holder.appendChild(solisApp({}, holder));
    solisSide.querySelector('.b').addEventListener('click', () => toggleSolis(false));
    document.body.appendChild(solisSide);
  }
  on = on ?? !solisSide.classList.contains('on');
  requestAnimationFrame(() => solisSide.classList.toggle('on', on));
  $('#solisBtn').setAttribute('aria-pressed', String(on));
  if (on) setTimeout(() => solisSide.querySelector('textarea')?.focus(), 260);
}
$('#solisBtn').addEventListener('click', () => toggleSolis());
document.addEventListener('keydown', e => {
  if (e.altKey && !e.ctrlKey && e.key.toLowerCase() === 'z' && signedIn) { e.preventDefault(); toggleSolis(); }
  else if (e.key === 'Escape' && solisSide?.classList.contains('on') && solisSide.contains(document.activeElement)) { e.stopPropagation(); toggleSolis(false); }
}, true);
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
  solis: { name: 'Solis', c: '#1F2A3A', icon: 'star', build: solisApp, flush: true },
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
  $('#solisBtn').hidden = !allowed('solis');
  const h = $('#s-home');
  const tiles = Object.entries(APPS).filter(([id]) => allowed(id)).map(([id, a], i) => `<button class="tile" data-open="${id}"><span class="m" style="--c:${a.c};--d:${(i * 0.35).toFixed(2)}s">${svg(a.icon)}</span>${a.name}</button>`).join('');
  h.innerHTML = `
    <div class="jump"><h3>Jump back in</h3><div class="posters" id="posters"><div class="poster" aria-hidden="true"><span class="art">${svg('photo', 'var(--shade)')}</span></div></div></div>
    <div class="tiles">${tiles}</div>
    <div class="shelf" id="polarisShelf" hidden><div class="head2"><h3>POLARIS</h3><span>its Apps, live from this PC, in any browser</span></div><div class="tiles" id="polarisApps"></div></div>
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
    // Sign-ins only: everything else (saving the layout, a link opened) is
    // recorded too, and would bury them.
    const a = await api.get('/api/access?kind=signedin&limit=10');
    const last = (a.entries || []).find(e => !e.who.startsWith('link:'));
    $('#lastIn').innerHTML = last ? `<span class="k">Last signed in</span><b>${esc(last.who === 'admin' ? 'You' : last.who)}${last.how === 'console' ? ', on this PC' : `, from ${esc(last.from)}`}</b><p>${ago(last.unix)} · ${esc({ code: 'with a code', password: 'with your password', console: 'from this PC', bridge: 'through POLARIS' }[last.how] || last.how)}</p>` : `<span class="k">Last signed in</span><b>Nobody yet</b><p>Every sign-in is recorded here, sealed.</p>`;
  } catch (e) {}
}
// **Recent and pinned**: what the person opened, and the folders they keep
// in the sidebar. Kept on the host with the layout (so they follow the
// person between devices); a guest's stay on their own device. Only what
// was opened: nothing is ever surfaced on its own.
function remember(item) {
  const key = JSON.stringify([item.kind, item.root, item.path]);
  state.recent = [item, ...state.recent.filter(r => JSON.stringify([r.kind, r.root, r.path]) !== key)].slice(0, 8);
  if (state.role === 'guest') { try { localStorage.setItem('prism.recent', JSON.stringify(state.recent)); } catch (e) {} }
  persist();
}
function forget(item) {
  state.recent = state.recent.filter(r => r !== item);
  if (state.role === 'guest') { try { localStorage.setItem('prism.recent', JSON.stringify(state.recent)); } catch (e) {} }
  persist(); posters();
}
function pinned(root, path) { return state.pins.some(p => p.root === root && p.path === path); }
function pin(root, path, on) {
  state.pins = state.pins.filter(p => !(p.root === root && p.path === path));
  if (on) state.pins.push({ root, path });
  if (state.role === 'guest') { try { localStorage.setItem('prism.pins', JSON.stringify(state.pins)); } catch (e) {} }
  persist(); $$('.pane[data-app="files"] .pbody').forEach(b => b.__pins && b.__pins());
}
function posters() {
  const box = $('#posters'); if (!box) return;
  box.innerHTML = '';
  for (const r of [...state.recent.filter(r => r.kind !== 'dir'), ...state.recent.filter(r => r.kind === 'dir')].slice(0, 4)) {
    const full = r.path;
    const b = r.kind === 'photo'
      ? el(`<button class="poster"><img alt="" src="/api/files/thumb?${q({ root: r.root, path: full })}"><span class="cap"><span style="min-width:0"><b>${esc(full.split('/').pop())}</b><span>${esc(r.root)}${full.includes('/') ? ' › ' + esc(full.split('/').slice(0, -1).join('/')) : ''}</span></span></span></button>`)
      : r.kind === 'video'
      ? el(`<button class="poster"><img alt="" src="/api/files/thumb?${q({ root: r.root, path: full })}"><span class="cap"><span class="playbtn">${svg('play', '#2F3138', 2.6)}</span><span style="min-width:0"><b>${esc(full.split('/').pop())}</b><span>${r.at ? `${Math.floor(r.at / 60)}:${String(Math.floor(r.at % 60)).padStart(2, '0')} in` : 'Watch'}</span></span></span></button>`)
      : el(`<button class="poster"><span class="art">${svg('folder', '#E5A23A')}</span><span class="cap"><span style="min-width:0"><b>${esc(full ? full.split('/').pop() : r.root)}</b><span>${full ? esc(r.root) + (full.includes('/') ? ' › ' + esc(full.split('/').slice(0, -1).join('/')) : '') : 'Folder'}</span></span></span></button>`);
    const open = () => r.kind === 'photo' ? openApp('photos', { root: r.root, dir: full.split('/').slice(0, -1).join('/'), name: full.split('/').pop() }) : r.kind === 'video' ? openApp('videos', { root: r.root, path: full, name: full.split('/').pop(), at: r.at }) : openApp('files', { root: r.root, path: full });
    b.addEventListener('click', open);
    b.addEventListener('contextmenu', ev => { ev.preventDefault(); menu(ev.clientX, ev.clientY, [{ label: 'Open', icon: r.kind === 'dir' ? 'folder' : r.kind === 'video' ? 'play' : 'photo', act: open }, r.kind !== 'dir' && { label: 'Show in Files', icon: 'folder', act: () => openApp('files', { root: r.root, path: full.split('/').slice(0, -1).join('/') }) }, 'sep', { label: 'Remove from Jump back in', icon: 'x', danger: true, act: () => forget(r) }]); });
    box.appendChild(b);
  }
  if (!box.children.length) box.appendChild(el(`<div class="card"><b>Nothing to carry on yet</b><p>What you open, pictures, films and folders, waits here for next time.</p></div>`));
}
async function loadServices() { try { facets = await api.get('/api/facets'); } catch (e) {} return facets; }
// A POLARIS App's colour, steady for its name (its own icon lives in POLARIS).
const POL_HUES = ['#7C5CE0', '#2E71C8', '#33BFE2', '#2E9E62', '#E5A23A', '#C0485C', '#5A5F68', '#D98A12'];
const polColour = name => POL_HUES[[...name].reduce((h, c) => (h * 31 + c.charCodeAt(0)) >>> 0, 7) % POL_HUES.length];
const titleOf = f => f.expose?.title || f.name;
function renderHosted() {
  const shelf = $('#hostedShelf'), box = $('#hosted'); if (!box) return;
  const pshelf = $('#polarisShelf'), pbox = $('#polarisApps');
  const exposed = facets.filter(f => f.expose);
  // POLARIS's own Apps get their shelf; everything else hosted here, its own.
  const isPol = f => /face-stream/.test(f.command || '');
  const pol = exposed.filter(isPol).sort((a, b) => titleOf(a).localeCompare(titleOf(b)));
  const other = exposed.filter(f => !isPol(f));
  shelf.hidden = !other.length; if (pshelf) pshelf.hidden = !pol.length;
  box.innerHTML = ''; if (pbox) pbox.innerHTML = '';
  const tile = (f, i, into) => {
    const p = isPol(f), running = f.state === 'running' || f.state === 'foreign';
    const face = p ? `<span class="mono">${esc(titleOf(f).split(/\s+/).map(w => w[0]).join('').slice(0, 2))}</span><span class="pstar">${svg('star')}</span>` : svg('globe');
    const b = el(`<button class="tile" data-facet="${esc(f.id)}"><span class="m" style="--c:${p ? polColour(titleOf(f)) : running ? '#2E9E62' : '#9A978F'};--d:${(3 + i * 0.35).toFixed(2)}s">${face}</span>${esc(titleOf(f))}<small>${running ? (p ? 'open now' : 'running') : p ? 'opens live' : 'stopped'}</small></button>`);
    b.addEventListener('click', () => openHosted(f.id));
    b.addEventListener('contextmenu', ev => { ev.preventDefault(); serviceMenu(f, ev.clientX, ev.clientY); });
    into.appendChild(b);
  };
  if (pbox) pol.forEach((f, i) => tile(f, i, pbox));
  other.forEach((f, i) => tile(f, i, box));
  const run = facets.filter(f => f.state === 'running').length;
  const card = $('#svcCard'); if (card) card.innerHTML = `<span class="k">Services</span><b>${run} running</b><p>${esc(facets.filter(f => f.state === 'running').map(titleOf).join(', ') || 'Nothing PRISM started is running.')}</p>`;
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

/* ── Right-click menus ───────────────────────────────────────────────── */
// Every thing on screen can say what can be done with it. Items: {label,
// icon, act, danger, disabled, hint} or 'sep'.
function menu(x, y, items) {
  closeMenus();
  const m = el(`<div class="ctx" role="menu"></div>`);
  for (const it of items) {
    if (it === 'sep') { m.appendChild(el('<span class="sep"></span>')); continue; }
    if (!it) continue;
    const b = el(`<button role="menuitem"${it.danger ? ' class="danger"' : ''}>${it.icon ? svg(it.icon, 'currentColor', 2.2) : '<i></i>'}<span>${esc(it.label)}</span>${it.hint ? `<small>${esc(it.hint)}</small>` : ''}</button>`);
    if (it.disabled) b.disabled = true;
    b.addEventListener('click', () => { closeMenus(); it.act && it.act(); });
    m.appendChild(b);
  }
  document.body.appendChild(m);
  const r = m.getBoundingClientRect();
  m.style.left = Math.max(8, Math.min(x, innerWidth - r.width - 8)) + 'px';
  m.style.top = Math.max(8, Math.min(y, innerHeight - r.height - 8)) + 'px';
  requestAnimationFrame(() => m.classList.add('on'));
  m.querySelector('button:not([disabled])')?.focus({ preventScroll: true });
  m.addEventListener('keydown', e => {
    const bs = [...m.querySelectorAll('button:not([disabled])')], i = bs.indexOf(document.activeElement);
    if (e.key === 'ArrowDown') { bs[(i + 1) % bs.length]?.focus(); e.preventDefault(); }
    if (e.key === 'ArrowUp') { bs[(i - 1 + bs.length) % bs.length]?.focus(); e.preventDefault(); }
    if (e.key === 'Escape') { e.stopPropagation(); closeMenus(); }
  });
}
function closeMenus() { $$('.ctx').forEach(m => { m.classList.remove('on'); setTimeout(() => m.remove(), 160); }); }
document.addEventListener('pointerdown', e => { if (!e.target.closest('.ctx')) closeMenus(); }, true);
addEventListener('blur', closeMenus);
// The browser's own menu only where text is being edited.
document.addEventListener('contextmenu', e => { if (!e.target.closest('input, textarea, .xt, iframe, [data-native-menu]')) e.preventDefault(); });

/* ── The window manager: floating or tiled, the person's choice ──────── */
// Tiled is a master and a stack, as Hyprland's master layout: the first pane
// on the left, the rest stacked on the right. The gaps between them drag to
// resize; Ctrl+drag picks a pane up (swap, or move when floating),
// Ctrl+right-drag resizes it; focus follows the mouse.
const MAXN = 6, GAP = 8;
let zTop = 10;
state.split = { main: .56, rows: [] };
function openApp(id, args, alone) {
  const a = appOf(id); if (!a || !allowed(id)) return;
  const existing = state.panes.find(p => p.id === id);
  if (existing) { if (args) { existing.args = args; const pe = paneEl(id); if (pe) rebuild(pe, existing); } state.focus = id; }
  else if (alone || state.scene !== 'work') { state.panes = [{ id, args }]; state.focus = id; }
  else {
    if (state.panes.length >= MAXN) state.panes[state.panes.findIndex(p => p.id === state.focus)] = { id, args };
    else state.panes.push({ id, args });
    state.focus = id;
  }
  showScene('work'); renderPanes(); persist();
}
const paneEl = id => $(`.pane[data-app="${CSS.escape(id)}"]`);
// Where each pane sits when tiled: [l, t, w, h] in pixels.
function tileRects(n, W, H) {
  if (n <= 1) return [[0, 0, W, H]];
  const mw = Math.round((W - GAP) * Math.min(.8, Math.max(.2, state.split.main)));
  const rows = state.split.rows.length === n - 1 ? state.split.rows : (state.split.rows = Array(n - 1).fill(1));
  const sum = rows.reduce((a, b) => a + b, 0), avail = H - GAP * (n - 2);
  const out = [[0, 0, mw, H]];
  let y = 0;
  rows.forEach((r, i) => { const h = i === n - 2 ? H - y : Math.round(avail * r / sum); out.push([mw + GAP, y, W - mw - GAP, h]); y += h + GAP; });
  return out;
}
function floatSpot(i, n, W, H) {
  const r = tileRects(Math.min(n, 4), W, H)[Math.min(i, 3)] || [W * .1, H * .08, W * .5, H * .6];
  const inset = 18 + i * 10;
  return { l: r[0] + inset, t: r[1] + inset, w: Math.max(340, r[2] - inset * 1.4), h: Math.max(240, r[3] - inset * 1.4) };
}
function place() {
  const box = $('#panes'), W = box.clientWidth, H = box.clientHeight;
  if (state.wm === 'tile') {
    const rects = tileRects(state.panes.length, W, H);
    state.panes.forEach((p, i) => { const e = paneEl(p.id); if (!e) return; const [l, t, w, h] = rects[i]; Object.assign(e.style, { left: l + 'px', top: t + 'px', width: w + 'px', height: h + 'px', zIndex: '' }); });
    gutters(rects);
  } else {
    $$('.gutter').forEach(g => g.remove());
    state.panes.forEach((p, i) => {
      const e = paneEl(p.id); if (!e) return;
      let s = state.pos[p.id];
      if (!s || s.l > W - 80 || s.t > H - 40) s = state.pos[p.id] = floatSpot(i, state.panes.length, W, H);
      Object.assign(e.style, { left: s.l + 'px', top: s.t + 'px', width: Math.min(s.w, W) + 'px', height: Math.min(s.h, H) + 'px' });
      if (!e.style.zIndex) e.style.zIndex = ++zTop;
    });
  }
}
// The gaps between tiles, to drag.
function gutters(rects) {
  $$('.gutter').forEach(g => g.remove());
  const box = $('#panes'), n = rects.length; if (n < 2 || matchMedia('(max-width:640px)').matches) return;
  const v = el(`<div class="gutter v" style="left:${rects[0][2]}px;top:0;height:${box.clientHeight}px"></div>`);
  drag(v, (dx) => { state.split.main = Math.min(.8, Math.max(.2, (rects[0][2] + dx) / (box.clientWidth - GAP))); place(); });
  box.appendChild(v);
  for (let i = 1; i < n - 1; i++) {
    const r = rects[i];
    const g = el(`<div class="gutter h" style="left:${r[0]}px;top:${r[1] + r[3]}px;width:${r[2]}px"></div>`);
    const a0 = state.split.rows[i - 1], b0 = state.split.rows[i], hsum = rects[i][3] + rects[i + 1][3];
    drag(g, (dx, dy) => { const t = Math.min(.85, Math.max(.15, (rects[i][3] + dy) / hsum)); state.split.rows[i - 1] = (a0 + b0) * t; state.split.rows[i] = (a0 + b0) * (1 - t); place(); });
    box.appendChild(g);
  }
}
function drag(handle, move) {
  handle.addEventListener('pointerdown', e => {
    e.preventDefault(); handle.classList.add('on'); $('#panes').classList.add('resizing');
    const sx = e.clientX, sy = e.clientY;
    track(m => move(m.clientX - sx, m.clientY - sy), () => { handle.classList.remove('on'); $('#panes').classList.remove('resizing'); persist(); });
  });
}
function glide(change) {
  const before = new Map(state.panes.map(p => [p.id, paneEl(p.id)?.getBoundingClientRect()]));
  change();
  if (reduce) return;
  state.panes.forEach(p => {
    const e = paneEl(p.id), a = before.get(p.id); if (!e || !a) return;
    const b = e.getBoundingClientRect(); if (!b.width) return;
    e.animate([{ transformOrigin: 'top left', transform: `translate(${a.left - b.left}px,${a.top - b.top}px) scale(${a.width / b.width},${a.height / b.height})` }, { transformOrigin: 'top left', transform: 'none' }], { duration: 420, easing: 'cubic-bezier(.2,.9,.25,1.05)' });
  });
}
function setWM(mode) {
  if (state.wm === mode) return;
  glide(() => { state.wm = mode; $$('.pane').forEach(e => e.classList.remove('max')); $('#panes').classList.toggle('float', mode === 'float'); place(); });
  renderStrip(); persist();
}
function setFocus(id, quiet) {
  if (state.focus === id) return;
  state.focus = id;
  $$('.pane').forEach(x => x.classList.toggle('focus', x.dataset.app === id));
  if (state.wm === 'float') { const f = paneEl(id); if (f) f.style.zIndex = ++zTop; }
  renderStrip();
  if (!quiet) persist();
}
function renderPanes() {
  const box = $('#panes');
  box.classList.toggle('float', state.wm === 'float');
  const have = new Map([...box.querySelectorAll('.pane')].map(c => [c.dataset.app, c]));
  for (const [id, e] of have) if (!state.panes.some(p => p.id === id)) { e.__close && e.__close(); e.remove(); }
  state.panes.forEach(p => { const e = have.get(p.id) || makePane(p); if (!e.isConnected) box.appendChild(e); });
  if (!state.focus || !state.panes.some(p => p.id === state.focus)) state.focus = state.panes[state.panes.length - 1]?.id;
  $$('.pane').forEach(x => x.classList.toggle('focus', x.dataset.app === state.focus));
  place();
  if (state.wm === 'float') { const f = paneEl(state.focus); if (f && +f.style.zIndex !== zTop) f.style.zIndex = ++zTop; }
  renderStrip();
}
function rebuild(e, p) { const body = e.querySelector('.pbody'); body.__close && body.__close(); body.innerHTML = ''; body.appendChild(appOf(p.id).build(p.args, body, p)); }
function paneMenu(p, x, y) {
  const i = state.panes.findIndex(q => q.id === p.id);
  menu(x, y, [
    { label: 'Only this one', icon: 'solo', act: () => { state.panes = [p]; state.focus = p.id; renderPanes(); persist(); } },
    state.wm === 'tile' && i > 0 && { label: 'Make it the main one', icon: 'tiles', act: () => glide(() => { state.panes.splice(i, 1); state.panes.unshift(p); renderPanes(); persist(); }) },
    { label: state.wm === 'float' ? 'Tile everything' : 'Float everything', icon: state.wm === 'float' ? 'tiles' : 'windows', act: () => { setWM(state.wm === 'float' ? 'tile' : 'float'); syncMenu(); } },
    p.id.startsWith('web:') && { label: 'Reload its page', icon: 'again', act: () => { const fr = paneEl(p.id)?.querySelector('iframe'); if (fr) fr.src = fr.src; } },
    p.id.startsWith('web:') && { label: 'Service…', icon: 'stack', act: () => { const f = facets.find(x => x.id === p.id.slice(4)); f && serviceMenu(f, x, y); } },
    'sep',
    { label: 'Close', icon: 'x', danger: true, hint: 'Esc', act: () => closePane(p.id) },
  ]);
}
function makePane(p) {
  const a = appOf(p.id);
  const e = el(`<section class="pane" data-app="${esc(p.id)}"><div class="phead"><span class="tk" style="background:${a.c}">${svg(a.icon)}</span><h2>${esc(a.name)}</h2><button class="b solo" title="Only this one" aria-label="Only ${esc(a.name)}">${svg('solo', 'currentColor', 2.4)}</button><button class="b x" aria-label="Close ${esc(a.name)}">${svg('x', 'currentColor', 2.6)}</button></div><div class="pbody${a.flush ? ' flush' : ''}"></div><span class="grip" aria-hidden="true"></span></section>`);
  const body = e.querySelector('.pbody');
  body.appendChild(a.build(p.args, body, p));
  e.__close = () => body.__close && body.__close();
  // Focus follows the mouse, as a tiling desktop does.
  e.addEventListener('pointerenter', () => { if (!dragging) setFocus(p.id, true); });
  e.addEventListener('pointerdown', () => setFocus(p.id));
  e.querySelector('.x').addEventListener('click', ev => { ev.stopPropagation(); closePane(p.id); });
  e.querySelector('.solo').addEventListener('click', ev => { ev.stopPropagation(); state.panes = [state.panes.find(x => x.id === p.id)]; state.focus = p.id; renderPanes(); persist(); });
  const head = e.querySelector('.phead');
  head.addEventListener('contextmenu', ev => { if (!ev.ctrlKey) { ev.preventDefault(); paneMenu(p, ev.clientX, ev.clientY); } });
  head.addEventListener('dblclick', ev => { if (state.wm === 'float' && !ev.target.closest('button')) glide(() => e.classList.toggle('max')); });
  head.addEventListener('pointerdown', ev => { if (ev.button === 0 && !ev.target.closest('button')) { ev.preventDefault(); pickUp(e, p, ev); } });
  // Ctrl+left anywhere on a pane picks it up; Ctrl+right resizes it.
  e.addEventListener('pointerdown', ev => {
    if (!ev.ctrlKey || matchMedia('(max-width:640px)').matches) return;
    ev.preventDefault(); ev.stopPropagation();
    if (ev.button === 0) pickUp(e, p, ev);
    if (ev.button === 2) resizeBy(e, p, ev);
  }, true);
  // Ctrl+right is a resize, never a menu (and not just after one either).
  e.addEventListener('contextmenu', ev => { if (ev.ctrlKey || dragging || performance.now() - lastGesture < 400) { ev.preventDefault(); ev.stopPropagation(); } }, true);
  const grip = e.querySelector('.grip');
  grip.addEventListener('pointerdown', ev => { ev.stopPropagation(); resizeBy(e, p, ev); });
  return e;
}
let dragging = false, lastGesture = 0;
// Follow the pointer anywhere on the page until it lets go. Window-level
// listeners rather than pointer capture: capture was lost the moment the
// pointer crossed an app's page (an iframe), which is why a drag stopped
// after a pixel. Pages are shielded from the pointer while dragging.
function track(move, up) {
  dragging = true; document.body.classList.add('dragging');
  const mv = m => { m.preventDefault(); move(m); };
  const end = m => {
    removeEventListener('pointermove', mv); removeEventListener('pointerup', end); removeEventListener('pointercancel', end);
    dragging = false; lastGesture = performance.now(); document.body.classList.remove('dragging');
    up && up(m);
  };
  addEventListener('pointermove', mv); addEventListener('pointerup', end); addEventListener('pointercancel', end);
}
function pickUp(e, p, ev) {
  if (matchMedia('(max-width:640px)').matches) return;
  const box = $('#panes'), sx = ev.clientX, sy = ev.clientY;
  if (state.wm === 'float') {
    if (e.classList.contains('max')) return;
    const s = state.pos[p.id], l0 = s.l, t0 = s.t;
    let moved = false;
    track(m => {
      if (!moved && Math.hypot(m.clientX - sx, m.clientY - sy) < 3) return;
      moved = true; e.classList.add('lifted');
      s.l = Math.min(box.clientWidth - 120, Math.max(-s.w + 120, l0 + m.clientX - sx)); s.t = Math.min(box.clientHeight - 40, Math.max(0, t0 + m.clientY - sy));
      e.style.left = s.l + 'px'; e.style.top = s.t + 'px';
    }, () => { e.classList.remove('lifted'); if (moved) persist(); });
  } else {
    let over = null, moved = false;
    track(m => {
      if (!moved && Math.hypot(m.clientX - sx, m.clientY - sy) < 4) return;
      moved = true; e.classList.add('lifted');
      e.style.transform = `translate(${m.clientX - sx}px,${m.clientY - sy}px) scale(.97)`;
      const t = document.elementsFromPoint(m.clientX, m.clientY).map(x => x.closest('.pane')).find(x => x && x !== e) || null;
      if (over !== t) { over && over.classList.remove('drop'); over = t; over && over.classList.add('drop'); }
    }, () => {
      e.classList.remove('lifted'); e.style.transform = '';
      if (over) { over.classList.remove('drop'); const ia = state.panes.findIndex(x => x.id === p.id), ib = state.panes.findIndex(x => x.id === over.dataset.app); glide(() => { [state.panes[ia], state.panes[ib]] = [state.panes[ib], state.panes[ia]]; place(); }); persist(); }
    });
  }
}
function resizeBy(e, p, ev) {
  if (matchMedia('(max-width:640px)').matches) return;
  const box = $('#panes'), sx = ev.clientX, sy = ev.clientY; box.classList.add('resizing');
  let mv;
  if (state.wm === 'float') {
    const s = state.pos[p.id], w0 = s.w, h0 = s.h;
    mv = m => { s.w = Math.max(320, w0 + m.clientX - sx); s.h = Math.max(220, h0 + m.clientY - sy); e.style.width = s.w + 'px'; e.style.height = s.h + 'px'; };
  } else {
    const i = state.panes.findIndex(x => x.id === p.id), main0 = state.split.main, W = box.clientWidth - GAP;
    const rows0 = [...state.split.rows], H = box.clientHeight;
    mv = m => {
      const dx = m.clientX - sx, dy = m.clientY - sy;
      // The main pane grows to the right; a stacked one grows to the left.
      state.split.main = Math.min(.8, Math.max(.2, main0 + (i === 0 ? dx : -dx) / W));
      if (i > 0 && rows0.length > 1) { const k = i - 1, sum = rows0.reduce((a, b) => a + b, 0), grow = dy / H * sum; const j = k < rows0.length - 1 ? k + 1 : k - 1; state.split.rows[k] = Math.max(.15, rows0[k] + grow); state.split.rows[j] = Math.max(.15, rows0[j] - grow); }
      place();
    };
  }
  track(mv, () => { box.classList.remove('resizing'); persist(); });
}
function closePane(id) {
  const e = paneEl(id); if (!e) return;
  e.classList.add('closing');
  setTimeout(() => {
    e.__close && e.__close(); e.remove();
    state.panes = state.panes.filter(p => p.id !== id); delete state.pos[id];
    if (state.focus === id) state.focus = state.panes[state.panes.length - 1]?.id;
    persist();
    if (!state.panes.length) goHome(); else glide(() => renderPanes());
  }, 220);
}
function renderStrip() {
  const s = $('#strip'); s.innerHTML = '';
  const h = el(`<button class="${state.scene === 'home' ? 'on' : ''}"><span class="tk" style="background:#2F3138">${svg('home')}</span>Home</button>`);
  h.addEventListener('click', goHome); s.appendChild(h);
  if (state.panes.length) s.appendChild(el('<span class="sep"></span>'));
  state.panes.forEach(p => {
    const a = appOf(p.id); if (!a) return;
    const b = el(`<button class="chip${state.scene === 'work' && p.id === state.focus ? ' on' : ''}"><span class="tk" style="background:${a.c}">${svg(a.icon)}</span>${esc(a.name)}<span class="x" role="button" aria-label="Close ${esc(a.name)}">${svg('x', 'currentColor', 3)}</span></button>`);
    b.addEventListener('click', ev => { if (ev.target.closest('.x')) { closePane(p.id); return; } if (state.scene !== 'work') showScene('work'); setFocus(p.id); });
    b.addEventListener('contextmenu', ev => { ev.preventDefault(); paneMenu(p, ev.clientX, ev.clientY); });
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
  $('#openHint').textContent = 'Up to six at once. One more replaces the one you are in.';
  const all = [...Object.keys(APPS).filter(allowed), ...(state.role === 'guest' ? [] : facets).filter(f => f.expose).map(f => 'web:' + f.id)];
  all.forEach(id => {
    const a = appOf(id);
    const b = el(`<button><span class="m" style="--c:${a.c}">${svg(a.icon)}</span>${esc(a.name)}</button>`);
    if (state.scene === 'work' && state.panes.some(p => p.id === id)) b.disabled = true;
    b.addEventListener('click', () => { closeSheet('#open'); id.startsWith('web:') ? openHosted(id.slice(4)) : openApp(id); }); row.appendChild(b);
  });
  toggleSheet('#open');
}
addEventListener('resize', () => { if (state.scene === 'work') place(); });
document.addEventListener('keydown', e => {
  if (e.key !== 'Escape') return;
  if ($('.ctx')) { closeMenus(); return; }
  if ($('#open').classList.contains('on') || $('#menu').classList.contains('on')) { closeSheet('#open'); closeSheet('#menu'); return; }
  if (e.target.closest && e.target.closest('.xt, input, textarea, video')) return;
  if (state.scene === 'work' && state.focus) closePane(state.focus);
});

/* ── Files ───────────────────────────────────────────────────────────── */
const KIND_ICON = { dir: ['folder', '#E5A23A'], image: ['photo', '#2E71C8'], video: ['play', '#C0485C'], audio: ['music', '#7C5CE0'], text: ['doc', '#868B94'], pdf: ['doc', '#C0485C'], archive: ['box', '#9A978F'], other: ['doc', '#9A978F'] };
function filesApp(args, body, pane) {
  const root = el(`<div class="files"><aside class="places"></aside><div class="browse"><div class="fbar"><div class="crumbs" title="Click the empty part to type a path"></div><label class="search">${svg('search', 'currentColor', 2.4)}<input type="search" placeholder="Search this folder" aria-label="Search this folder"></label></div><div class="chosenbar" hidden></div><div class="grid" tabindex="0"></div></div></div>`);
  let roots = [], cur = { root: args?.root, path: args?.path || '' }, entries = [], shown = [], sel = 0, anchor = 0, typed = '', typedAt = 0, stay = 0;
  const chosen = new Set();
  const places = root.querySelector('.places'), crumbs = root.querySelector('.crumbs'), grid = root.querySelector('.grid'), search = root.querySelector('.search input'), bar = root.querySelector('.chosenbar');
  const writable = () => state.role !== 'guest' && roots.find(r => r.name === cur.root)?.writable;
  const fullOf = x => cur.path ? `${cur.path}/${x.name}` : x.name;
  const things = () => grid.querySelectorAll('.thing');
  const picked = () => shown.filter(x => chosen.has(x.name));
  const zipUrl = (names, name) => `/api/files/zip?${q({ root: cur.root, dir: cur.path, names: JSON.stringify(names), name: name || '' })}`;
  const save = href => { const a = document.createElement('a'); a.href = href; a.download = ''; document.body.appendChild(a); a.click(); a.remove(); };
  // The sidebar: only what the person pinned. Pin a folder from its menu.
  function drawPins() {
    places.innerHTML = '<div class="group">Pinned</div>';
    if (!state.pins.length) places.appendChild(el(`<p class="hint">Right-click a folder and choose Pin to keep it here.</p>`));
    for (const p of state.pins) {
      if (!roots.some(r => r.name === p.root)) continue;
      const b = el(`<button class="place" aria-pressed="${p.root === cur.root && p.path === cur.path}"><span class="tk" style="background:#E5A23A">${svg('folder')}</span><span>${esc(p.path ? p.path.split('/').pop() : p.root)}</span></button>`);
      b.addEventListener('click', () => go(p.root, p.path));
      b.addEventListener('contextmenu', ev => { ev.preventDefault(); menu(ev.clientX, ev.clientY, [{ label: 'Open', icon: 'folder', act: () => go(p.root, p.path) }, { label: 'Unpin', icon: 'x', act: () => pin(p.root, p.path, false) }]); });
      places.appendChild(b);
    }
  }
  body.__pins = drawPins;
  function go(r, path) { cur = { root: r, path }; load(); }
  // Selection: a cursor (keys move it) and the set chosen (Ctrl, Shift, a box).
  function paint() {
    things().forEach((t, j) => { t.classList.toggle('sel', j === sel); t.classList.toggle('chosen', chosen.has(shown[j]?.name)); });
    const n = chosen.size;
    bar.hidden = n < 2;
    if (n >= 2) {
      bar.innerHTML = `<b>${n} chosen</b><button class="btn q dl">${svg('down', 'currentColor', 2.4)}Download as zip</button>${writable() ? `<button class="btn q del">${svg('trash', 'currentColor', 2.2)}Delete</button>` : ''}<button class="btn q clr">Clear</button>`;
      bar.querySelector('.dl').addEventListener('click', () => save(zipUrl([...chosen])));
      bar.querySelector('.del')?.addEventListener('click', () => removeAll(picked()));
      bar.querySelector('.clr').addEventListener('click', () => { chosen.clear(); paint(); });
    }
  }
  function select(i, scroll = true, how = 'only') {
    if (!shown.length) return;
    i = Math.max(0, Math.min(shown.length - 1, i));
    if (how === 'only') { chosen.clear(); chosen.add(shown[i].name); anchor = i; }
    else if (how === 'toggle') { chosen.has(shown[i].name) ? chosen.delete(shown[i].name) : chosen.add(shown[i].name); anchor = i; }
    else if (how === 'range') { chosen.clear(); for (let k = Math.min(anchor, i); k <= Math.max(anchor, i); k++) chosen.add(shown[k].name); }
    sel = i; paint();
    if (scroll) things()[sel]?.scrollIntoView({ block: 'nearest', behavior: reduce ? 'auto' : 'smooth' });
  }
  function open(x) {
    const full = fullOf(x);
    if (x.is_dir) { cur.path = full; load(); }
    else if (x.kind === 'image') { openApp('photos', { root: cur.root, dir: cur.path, name: x.name, list: entries.filter(e => e.kind === 'image').map(e => e.name) }); }
    else if (x.kind === 'video' || x.kind === 'audio') openApp('videos', { root: cur.root, path: full, name: x.name });
    else window.open(`/api/files/raw?${q({ root: cur.root, path: full })}`, '_blank', 'noopener');
  }
  async function removeAll(xs) {
    if (!xs.length) return;
    const ok = await ask(xs.length === 1 ? `Delete ${xs[0].name}? There's no undo.` : `Delete these ${xs.length}? There's no undo.`, null, 'Delete'); if (!ok) return;
    let done = 0;
    for (const x of xs) { try { await api.post('/api/files/delete', { root: cur.root, path: fullOf(x), recursive: x.is_dir }); done++; } catch (e) { toast(`${x.name}: ${e.message}`, true); } }
    if (done) toast(done === 1 ? `${xs[0].name} deleted.` : `${done} deleted.`);
    chosen.clear(); load(true);
  }
  function itemMenu(x, ev) {
    const many = chosen.size > 1 && chosen.has(x.name);
    if (many) {
      const xs = picked();
      return menu(ev.clientX, ev.clientY, [
        { label: `Download ${xs.length} as zip`, icon: 'down', act: () => save(zipUrl(xs.map(y => y.name))) },
        { label: 'Copy their paths', icon: 'doc', act: () => navigator.clipboard?.writeText(xs.map(y => `${cur.root}/${fullOf(y)}`).join('\n')).then(() => toast('Paths copied.'), () => {}) },
        writable() && 'sep',
        writable() && { label: `Delete ${xs.length}`, icon: 'trash', danger: true, hint: 'Del', act: () => removeAll(xs) },
      ]);
    }
    const full = fullOf(x), w = writable();
    menu(ev.clientX, ev.clientY, [
      { label: 'Open', icon: x.is_dir ? 'folder' : (KIND_ICON[x.kind] || KIND_ICON.other)[0], act: () => open(x), hint: 'Enter' },
      !x.is_dir && { label: 'Download', icon: 'down', act: () => save(`/api/files/raw?${q({ root: cur.root, path: full, download: true })}`) },
      x.is_dir && { label: 'Download as zip', icon: 'down', act: () => save(zipUrl([x.name])) },
      x.is_dir && { label: pinned(cur.root, full) ? 'Unpin' : 'Pin to the sidebar', icon: 'pin', act: () => { pin(cur.root, full, !pinned(cur.root, full)); draw(); } },
      x.is_dir && state.role !== 'guest' && { label: 'Share a link…', icon: 'link', act: () => openShare(cur.root, full, !!w) },
      { label: 'Copy its path', icon: 'doc', act: () => navigator.clipboard?.writeText(`${rootPath(cur.root)}/${full}`).then(() => toast('Path copied.'), () => {}) },
      w && 'sep',
      w && { label: 'Rename…', icon: 'pen', hint: 'F2', act: () => rename(x) },
      w && { label: 'Delete', icon: 'trash', danger: true, hint: 'Del', act: () => removeAll([x]) },
      'sep',
      { label: `${x.is_dir ? 'Folder' : bytes(x.size)}${x.modified ? ' · ' + new Date(x.modified * 1000).toLocaleString(undefined, { day: 'numeric', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit' }) : ''}`, disabled: true },
    ]);
  }
  async function rename(x) { const n = await ask(`A new name for ${x.name}`, x.name); if (!n || n === x.name) return; try { await api.post('/api/files/rename', { root: cur.root, path: fullOf(x), name: n }); load(true); } catch (e) { toast(e.message, true); } }
  function draw() {
    const f = search.value.trim().toLowerCase();
    shown = f ? entries.filter(x => x.name.toLowerCase().includes(f)) : entries;
    grid.innerHTML = shown.length ? '' : `<div class="empty">${f ? `Nothing here matches “${esc(search.value)}”.` : writable() ? 'Empty. Drop files here to upload them.' : 'Empty.'}</div>`;
    for (const x of shown.slice(0, 2000)) {
      const [ic, c] = KIND_ICON[x.kind] || KIND_ICON.other;
      const full = fullOf(x), thumb = x.kind === 'image' || x.kind === 'video';
      const b = el(`<button class="thing" tabindex="-1"><span class="th">${svg(ic, c, 2.2)}${thumb ? `<img loading="lazy" decoding="async" alt="" src="/api/files/thumb?${q({ root: cur.root, path: full })}" onerror="this.remove()" onload="this.classList.add('in')">` : ''}${x.is_dir && pinned(cur.root, full) ? `<span class="pinmark">${svg('pin', '#fff', 2.4)}</span>` : ''}<span class="tick">${svg('check', '#fff', 3)}</span></span><b title="${esc(x.name)}">${esc(x.name)}</b><small>${x.is_dir ? 'Folder' : bytes(x.size)}${x.modified ? ' · ' + ago(x.modified) : ''}</small></button>`);
      b.addEventListener('click', ev => {
        const i = shown.indexOf(x);
        if (ev.ctrlKey || ev.metaKey) return select(i, false, 'toggle');
        if (ev.shiftKey) return select(i, false, 'range');
        if ((sel === i && chosen.size === 1 && chosen.has(x.name)) || matchMedia('(hover:none)').matches) open(x); else select(i, false);
      });
      b.addEventListener('dblclick', ev => { if (!ev.ctrlKey && !ev.shiftKey) open(x); });
      b.addEventListener('contextmenu', ev => { ev.preventDefault(); const i = shown.indexOf(x); if (!chosen.has(x.name)) select(i, false); else { sel = i; paint(); } itemMenu(x, ev); });
      grid.appendChild(b);
    }
    sel = Math.min(sel, Math.max(0, shown.length - 1)); paint();
  }
  async function load(keep) {
    pane.args = { ...cur }; if (!keep) { search.value = ''; sel = 0; chosen.clear(); }
    // A folder counts once the person stays in it, not each one passed through.
    clearTimeout(stay); const here = { kind: 'dir', root: cur.root, path: cur.path }; stay = setTimeout(() => remember(here), 4000);
    drawCrumbs(); drawPins();
    if (!keep) grid.innerHTML = `<div class="empty">Opening…</div>`;
    try {
      const l = await api.get(`/api/files/list?${q({ root: cur.root, path: cur.path, limit: 4000 })}`);
      entries = (l.entries || []).filter(x => !x.name.startsWith('.')).sort((a, b) => (b.is_dir - a.is_dir) || a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' }));
    } catch (e) { entries = []; grid.innerHTML = `<div class="empty">${esc(e.message)}</div>`; return; }
    for (const n of [...chosen]) if (!entries.some(x => x.name === n)) chosen.delete(n);
    draw();
  }
  function drawCrumbs() {
    const parts = cur.path ? cur.path.split('/') : [];
    crumbs.innerHTML = '';
    // The root is a switcher: every folder this PC shares.
    const rb = el(`<button class="rootb">${svg('box', 'currentColor', 2.2)}${esc(cur.root)}${svg('down2', 'currentColor', 2.4)}</button>`);
    rb.addEventListener('click', ev => { ev.stopPropagation(); const r = rb.getBoundingClientRect(); menu(r.left, r.bottom + 6, roots.map(x => ({ label: x.name, icon: 'folder', hint: x.writable ? 'can change' : 'look only', act: () => go(x.name, '') }))); });
    crumbs.appendChild(rb);
    parts.forEach((p, i) => { crumbs.appendChild(el('<span class="chev">›</span>')); const b = el(`<button>${esc(p)}</button>`); b.addEventListener('click', ev => { ev.stopPropagation(); cur.path = parts.slice(0, i + 1).join('/'); load(); }); crumbs.appendChild(b); });
    crumbs.appendChild(el('<span class="fill" aria-hidden="true"></span>'));
    const acts = el(`<span class="acts"></span>`);
    if (state.role !== 'guest') { const sb = el(`<button class="ib" title="Share this folder" aria-label="Share this folder">${svg('link', 'currentColor', 2.4)}</button>`); sb.addEventListener('click', ev => { ev.stopPropagation(); openShare(cur.root, cur.path, !!writable()); }); acts.appendChild(sb); }
    const zb = el(`<button class="ib" title="Download this folder as a zip" aria-label="Download this folder as a zip">${svg('down', 'currentColor', 2.4)}</button>`); zb.addEventListener('click', ev => { ev.stopPropagation(); save(zipUrl([], cur.path ? cur.path.split('/').pop() : cur.root)); }); acts.appendChild(zb);
    if (writable()) {
      const nb = el(`<button class="ib" title="New folder" aria-label="New folder">${svg('newdir', 'currentColor', 2.2)}</button>`);
      nb.addEventListener('click', ev => { ev.stopPropagation(); newFolder(); });
      const ub = el(`<label class="ib" title="Upload" aria-label="Upload">${svg('up', 'currentColor', 2.4)}<input type="file" multiple hidden></label>`);
      ub.addEventListener('click', ev => ev.stopPropagation());
      ub.querySelector('input').addEventListener('change', ev => upload([...ev.target.files]));
      acts.append(nb, ub);
    }
    crumbs.appendChild(acts);
  }
  async function newFolder() { const name = await ask('A name for the new folder'); if (!name) return; try { await api.post('/api/files/mkdir', { root: cur.root, path: cur.path, name }); load(true); } catch (e) { toast(e.message, true); } }
  /* Typing a path: click the bar's empty part. Tab takes the suggestion,
     ↑/↓ choose another, Enter goes, Esc puts the folders back. Paths are
     this PC's own (/home/…) for the owner; a guest types the folder names. */
  const rootPath = n => { const r = roots.find(x => x.name === n); return r?.path || '/' + n; };
  const absOf = (r, rel) => rootPath(r) + (rel ? '/' + rel : '');
  function where(abs) {
    let best = null;
    for (const r of roots) { const p = rootPath(r.name); if ((abs === p || abs.startsWith(p + '/')) && (!best || p.length > rootPath(best.name).length)) best = r; }
    return best ? { root: best.name, rel: abs.slice(rootPath(best.name).length).replace(/^\/+|\/+$/g, '') } : null;
  }
  const listCache = new Map();
  async function dirsIn(abs) {
    if (listCache.has(abs)) return listCache.get(abs);
    let out = [];
    const w = where(abs);
    if (w) { try { const l = await api.get(`/api/files/list?${q({ root: w.root, path: w.rel, limit: 4000 })}`); out = (l.entries || []).filter(x => x.is_dir && !x.name.startsWith('.')).map(x => x.name); } catch (e) {} }
    else { const pre = abs === '/' ? '/' : abs + '/'; out = [...new Set(roots.map(r => rootPath(r.name)).filter(p => p.startsWith(pre)).map(p => p.slice(pre.length).split('/')[0]))]; }
    out.sort((a, b) => a.localeCompare(b, undefined, { numeric: true, sensitivity: 'base' }));
    listCache.set(abs, out); return out;
  }
  function editPath() {
    if (crumbs.querySelector('.pathin')) return;
    const box = el(`<div class="pathbox"><input class="pathin" spellcheck="false" autocomplete="off" aria-label="Path"><span class="ghost" aria-hidden="true"></span><div class="sugg" role="listbox"></div></div>`);
    const inp = box.querySelector('input'), ghost = box.querySelector('.ghost'), sugg = box.querySelector('.sugg');
    crumbs.replaceChildren(box);
    inp.value = absOf(cur.root, cur.path) + '/';
    inp.focus(); inp.setSelectionRange(inp.value.length, inp.value.length);
    let list = [], pick = 0, n = 0;
    async function suggest() {
      const my = ++n, v = inp.value, cut = v.lastIndexOf('/');
      const dir = cut <= 0 ? '/' : v.slice(0, cut), part = v.slice(cut + 1).toLowerCase();
      const all = await dirsIn(dir); if (my !== n) return;
      list = all.filter(d => d.toLowerCase().startsWith(part)).slice(0, 8); pick = Math.min(pick, Math.max(0, list.length - 1));
      sugg.innerHTML = list.map((d, i) => `<button type="button" role="option" aria-selected="${i === pick}" data-i="${i}">${svg('folder', '#E5A23A', 2.2)}<span><b>${esc(d.slice(0, part.length))}</b>${esc(d.slice(part.length))}</span></button>`).join('');
      sugg.querySelectorAll('button').forEach(b => b.addEventListener('mousedown', ev => { ev.preventDefault(); pick = +b.dataset.i; take(); }));
      // The rest of the suggestion, shown faintly after what's typed.
      ghost.textContent = list[pick] ? list[pick].slice(part.length) : '';
      ghost.style.left = `calc(14px + ${measure(v)}px)`;
    }
    const cv = document.createElement('canvas').getContext('2d');
    const measure = t => { cv.font = getComputedStyle(inp).font; return cv.measureText(t).width; };
    function take() { const v = inp.value, cut = v.lastIndexOf('/'); if (!list[pick]) return; inp.value = v.slice(0, cut + 1) + list[pick] + '/'; pick = 0; suggest(); }
    const close = () => { n++; drawCrumbs(); };
    function goTo() {
      const v = inp.value.trim().replace(/\/+$/, '') || '/';
      const w = where(v.startsWith('/') ? v : '/' + v);
      if (!w) { toast("That isn't in a folder this PC shares.", true); return; }
      cur = { root: w.root, path: w.rel }; load(); grid.focus({ preventScroll: true });
    }
    inp.addEventListener('input', () => { pick = 0; suggest(); });
    inp.addEventListener('keydown', e => {
      if (e.key === 'Tab') { e.preventDefault(); take(); }
      else if (e.key === 'ArrowDown') { e.preventDefault(); pick = (pick + 1) % Math.max(1, list.length); suggest(); }
      else if (e.key === 'ArrowUp') { e.preventDefault(); pick = (pick - 1 + list.length) % Math.max(1, list.length); suggest(); }
      else if (e.key === 'Enter') { e.preventDefault(); goTo(); }
      else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); close(); }
      e.stopPropagation();
    });
    inp.addEventListener('blur', () => setTimeout(() => { if (!box.contains(document.activeElement) && box.isConnected) close(); }, 120));
    suggest();
  }
  crumbs.addEventListener('click', ev => { if (!ev.target.closest('button, label, .pathbox')) editPath(); });
  async function upload(files) {
    for (const f of files) {
      toast(`Uploading ${f.name}…`);
      try { await call('POST', `/api/files/upload?${q({ root: cur.root, path: cur.path, name: f.name })}`, f, true); }
      catch (e) { toast(`${f.name}: ${e.message}`, true); }
    }
    load(true);
  }
  search.addEventListener('input', () => { sel = 0; draw(); });
  search.addEventListener('keydown', e => { if (e.key === 'Enter' && shown[0]) open(shown[sel] || shown[0]); if (e.key === 'ArrowDown') { grid.focus(); select(0); e.preventDefault(); } if (e.key === 'Escape' && search.value) { e.stopPropagation(); search.value = ''; draw(); } });
  // Keys: arrows (Shift to extend), Enter, Backspace, Delete, F2, Ctrl+A,
  // Ctrl+F, L to type a path; letters jump to the first name that starts so.
  root.addEventListener('keydown', e => {
    if (e.target.closest('input')) return;
    const k = e.key, ext = e.shiftKey ? 'range' : 'only';
    const cols = Math.max(1, Math.round(grid.clientWidth / ((things()[0]?.offsetWidth || 140) + 10)));
    if ((e.ctrlKey || e.metaKey) && k.toLowerCase() === 'a') { shown.forEach(x => chosen.add(x.name)); paint(); }
    else if ((e.ctrlKey || e.metaKey) && k.toLowerCase() === 'f') search.focus();
    else if ((e.ctrlKey || e.metaKey) && k.toLowerCase() === 'l') editPath();
    else if (e.ctrlKey || e.metaKey || e.altKey) return;
    else if (k === 'ArrowRight') select(sel + 1, true, ext); else if (k === 'ArrowLeft') select(sel - 1, true, ext);
    else if (k === 'ArrowDown') select(sel + cols, true, ext); else if (k === 'ArrowUp') select(sel - cols, true, ext);
    else if (k === 'Enter' && shown[sel]) open(shown[sel]);
    else if (k === 'Backspace' && cur.path) { cur.path = cur.path.split('/').slice(0, -1).join('/'); load(); }
    else if (k === 'Delete' && writable()) removeAll(chosen.size ? picked() : shown[sel] ? [shown[sel]] : []);
    else if (k === 'F2' && writable() && shown[sel]) rename(shown[sel]);
    else if (k === 'Escape' && chosen.size > 1) { chosen.clear(); if (shown[sel]) chosen.add(shown[sel].name); paint(); }
    else if (k === 'ContextMenu' && shown[sel]) { const r = things()[sel].getBoundingClientRect(); itemMenu(shown[sel], { clientX: r.left + 20, clientY: r.top + 20 }); }
    else if (k === '/') editPath();
    else if (k.length === 1 && /\S/.test(k)) {
      const now = performance.now(); typed = (now - typedAt > 900 ? '' : typed) + k.toLowerCase(); typedAt = now;
      const i = shown.findIndex(x => x.name.toLowerCase().startsWith(typed));
      if (i >= 0) select(i);
    } else return;
    e.preventDefault();
  });
  // A box drawn on the empty part chooses what it touches.
  grid.addEventListener('pointerdown', ev => {
    if (ev.button !== 0 || ev.target.closest('.thing') || matchMedia('(hover:none)').matches) return;
    const g = grid.getBoundingClientRect(), sx = ev.clientX, sy = ev.clientY + grid.scrollTop, add = ev.ctrlKey || ev.metaKey, before = new Set(chosen);
    const band = el('<div class="band"></div>'); let on = false;
    grid.focus({ preventScroll: true });
    track(m => {
      const x = m.clientX, y = m.clientY + grid.scrollTop;
      if (!on) { if (Math.hypot(x - sx, y - sy) < 5) return; on = true; grid.appendChild(band); }
      const l = Math.min(x, sx) - g.left, t = Math.min(y, sy) - g.top, w = Math.abs(x - sx), h = Math.abs(y - sy);
      Object.assign(band.style, { left: l + 'px', top: t + 'px', width: w + 'px', height: h + 'px' });
      chosen.clear(); if (add) before.forEach(n => chosen.add(n));
      things().forEach((tEl, j) => { const r = tEl.getBoundingClientRect(), rt = r.top - g.top + grid.scrollTop, rl = r.left - g.left; if (rl < l + w && rl + r.width > l && rt < t + h && rt + r.height > t) chosen.add(shown[j].name); });
      paint();
    }, () => { band.remove(); if (!on && !add) { chosen.clear(); paint(); } });
  });
  grid.addEventListener('contextmenu', ev => { if (ev.target.closest('.thing')) return; ev.preventDefault(); const w = writable(); menu(ev.clientX, ev.clientY, [
    w && { label: 'New folder…', icon: 'newdir', act: newFolder },
    { label: 'Choose everything', icon: 'check', hint: 'Ctrl+A', act: () => { shown.forEach(x => chosen.add(x.name)); paint(); } },
    { label: 'Download this folder as a zip', icon: 'down', act: () => save(zipUrl([], cur.path ? cur.path.split('/').pop() : cur.root)) },
    { label: pinned(cur.root, cur.path) ? 'Unpin this folder' : 'Pin this folder', icon: 'pin', act: () => pin(cur.root, cur.path, !pinned(cur.root, cur.path)) },
    state.role !== 'guest' && { label: 'Share this folder…', icon: 'link', act: () => openShare(cur.root, cur.path, !!w) },
    { label: 'Type a path', icon: 'pen', hint: '/', act: editPath },
    cur.path && { label: 'Up a folder', icon: 'up', hint: 'Backspace', act: () => { cur.path = cur.path.split('/').slice(0, -1).join('/'); load(); } },
  ]); });
  root.addEventListener('dragover', e => { if (writable() && e.dataTransfer?.types?.includes('Files')) { e.preventDefault(); grid.classList.add('over'); } });
  root.addEventListener('dragleave', () => grid.classList.remove('over'));
  root.addEventListener('drop', e => { e.preventDefault(); grid.classList.remove('over'); if (e.dataTransfer.files.length) upload([...e.dataTransfer.files]); });
  api.get('/api/files/roots').then(rs => {
    roots = rs;
    if (!rs.length) { grid.innerHTML = `<div class="empty">${state.role === 'guest' ? 'No folders are open to guests here.' : 'No folders are shared in prism.toml yet.'}</div>`; return; }
    if (!cur.root || !rs.some(r => r.name === cur.root)) { const p0 = state.pins.find(p => rs.some(r => r.name === p.root)); cur = p0 ? { ...p0 } : { root: rs[0].name, path: '' }; }
    load(); setTimeout(() => grid.focus({ preventScroll: true }), 50);
  }).catch(e => { grid.innerHTML = `<div class="empty">${esc(e.message)}</div>`; });
  return root;
}
// A small in-page question: the browser's prompt() isn't allowed everywhere.
function ask(question, value, yes) {
  return new Promise(res => {
    const s = el(`<div class="asksheet" role="dialog"><h3>${esc(question)}</h3>${value === null ? '' : `<input value="${esc(value || '')}">`}<div class="row2b"><button class="btn q">Cancel</button><button class="btn${yes ? ' bad' : ''}">${esc(yes || 'OK')}</button></div></div>`);
    document.body.appendChild(s); const i = s.querySelector('input'); (i || s.querySelector('.btn:not(.q)')).focus(); if (i) i.select();
    const done = v => { s.remove(); res(v); };
    s.querySelector('.btn.q').addEventListener('click', () => done(null));
    s.querySelector('.btn:not(.q)').addEventListener('click', () => done(i ? (i.value.trim() || null) : true));
    s.addEventListener('keydown', e => { if (e.key === 'Enter') done(i ? (i.value.trim() || null) : true); if (e.key === 'Escape') { e.stopPropagation(); done(null); } });
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
  const srcOf = n => /\.(gif|svg|apng)$/i.test(n) ? `/api/files/raw?${q({ root: args.root, path: pathOf(n) })}` : `/api/files/preview?${q({ root: args.root, path: pathOf(n) })}`;
  function show(i) {
    at = (i + list.length) % list.length; const n = list[at];
    remember({ kind: 'photo', root: args.root, path: pathOf(n) });
    img.style.opacity = 0;
    // A screen-sized copy first (a few hundred KB, made once and kept), then
    // the neighbours, so stepping through is instant. Moving pictures and
    // drawings are shown as they are.
    const next = new Image(); next.decoding = 'async';
    next.onload = () => { if (list[at] !== n) return; img.src = next.src; img.style.opacity = 1; };
    next.src = srcOf(n);
    [1, -1, 2].forEach(d => { const m = list[(at + d + list.length) % list.length]; if (m && m !== n) { const pre = new Image(); pre.src = srcOf(m); } });
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
// A player of our own: speed, loop, volume, seeking by keys, picture in
// picture. Keys follow the ones people know: Space/K play, J/L ten seconds,
// ←/→ five, ↑/↓ volume, M mute, F full screen, [ ] speed, 0–9 jump.
const SPEEDS = [0.25, 0.5, 0.75, 1, 1.25, 1.5, 1.75, 2, 3];
function videosApp(args, body, pane) {
  const root = el(`<div class="player" tabindex="0"><div class="stage"><video playsinline preload="auto"></video><div class="bigplay">${svg('play', '#fff', 2.4)}</div><div class="osd"></div></div>
    <div class="vbar"><input class="seek" type="range" min="0" max="1000" value="0" aria-label="Position"><div class="vrow">
      <button class="vb play" aria-label="Play">${svg('play', 'currentColor', 2.4)}</button>
      <span class="time">0:00 / 0:00</span>
      <span class="vname"></span>
      <button class="vb loop" aria-label="Loop" aria-pressed="false" title="Loop">${svg('loop', 'currentColor', 2.2)}</button>
      <button class="vb speed" aria-label="Speed" title="Speed  [ ]">1×</button>
      <span class="volw"><button class="vb vol" aria-label="Mute" title="Mute  M">${svg('vol', 'currentColor', 2.2)}</button><input class="volr" type="range" min="0" max="1" step="0.02" value="1" aria-label="Volume"></span>
      <button class="vb pipb" aria-label="Picture in picture" title="Picture in picture">${svg('pip', 'currentColor', 2.2)}</button>
      <button class="vb fullb" aria-label="Full screen" title="Full screen  F">${svg('full', 'currentColor', 2.2)}</button>
    </div></div><p class="meta"></p></div>`);
  if (!args?.root) { root.innerHTML = `<div class="empty">Open a film or a song from Files.</div>`; return root; }
  const v = root.querySelector('video'), meta = root.querySelector('.meta'), seek = root.querySelector('.seek'), osd = root.querySelector('.osd');
  const fmt = t => { if (!isFinite(t)) return '–:––'; t = Math.floor(t); const h = Math.floor(t / 3600), m = Math.floor(t % 3600 / 60), x = String(t % 60).padStart(2, '0'); return h ? `${h}:${String(m).padStart(2, '0')}:${x}` : `${m}:${x}`; };
  let say = 0;
  const flash = text => { osd.textContent = text; osd.classList.add('on'); clearTimeout(say); say = setTimeout(() => osd.classList.remove('on'), 700); };
  let prefs = { vol: 1, rate: 1 }; try { prefs = { ...prefs, ...JSON.parse(localStorage.getItem('prism.player') || '{}') }; } catch (e) {}
  const keep = () => { try { localStorage.setItem('prism.player', JSON.stringify({ vol: v.volume, rate: v.playbackRate, muted: v.muted })); } catch (e) {} };
  v.volume = prefs.vol; v.muted = !!prefs.muted; v.playbackRate = prefs.rate; v.defaultPlaybackRate = prefs.rate;
  root.querySelector('.vname').textContent = args.name;
  const playB = root.querySelector('.play'), speedB = root.querySelector('.speed'), volB = root.querySelector('.vol'), volR = root.querySelector('.volr'), loopB = root.querySelector('.loop');
  const sync = () => {
    playB.innerHTML = svg(v.paused ? 'play' : 'pause', 'currentColor', 2.4); playB.setAttribute('aria-label', v.paused ? 'Play' : 'Pause');
    root.classList.toggle('paused', v.paused);
    speedB.textContent = `${v.playbackRate}×`;
    volB.innerHTML = svg(v.muted || v.volume === 0 ? 'mute' : 'vol', 'currentColor', 2.2); volR.value = v.muted ? 0 : v.volume; volR.style.setProperty('--p', volR.value * 100 + '%');
    loopB.setAttribute('aria-pressed', String(v.loop));
  };
  const toggle = () => { v.paused ? v.play().catch(() => {}) : v.pause(); };
  const jump = d => { v.currentTime = Math.max(0, Math.min((v.duration || 0) - .1, v.currentTime + d)); flash(`${d > 0 ? '+' : '−'}${Math.abs(d)} s`); };
  const rate = d => { const i = SPEEDS.indexOf(v.playbackRate); v.playbackRate = SPEEDS[Math.max(0, Math.min(SPEEDS.length - 1, (i < 0 ? 3 : i) + d))]; flash(`${v.playbackRate}×`); keep(); };
  const vol = d => { v.muted = false; v.volume = Math.max(0, Math.min(1, Math.round((v.volume + d) * 20) / 20)); flash(`Volume ${Math.round(v.volume * 100)}%`); keep(); };
  let lastSave = 0;
  v.addEventListener('timeupdate', () => {
    if (!seek.matches(':active')) seek.value = v.duration ? Math.round(v.currentTime / v.duration * 1000) : 0;
    seek.style.setProperty('--p', seek.value / 10 + '%');
    root.querySelector('.time').textContent = `${fmt(v.currentTime)} / ${fmt(v.duration)}`;
    // Where it was left, for Jump back in.
    if (performance.now() - lastSave > 5000 && v.currentTime > 5) { lastSave = performance.now(); remember({ kind: 'video', root: args.root, path: args.path, at: Math.floor(v.currentTime) }); }
  });
  ['play', 'pause', 'ratechange', 'volumechange'].forEach(ev => v.addEventListener(ev, sync));
  v.addEventListener('loadedmetadata', () => { if (args.at && args.at < v.duration - 5) v.currentTime = args.at; sync(); });
  seek.addEventListener('input', () => { seek.style.setProperty('--p', seek.value / 10 + '%'); if (v.duration) v.currentTime = seek.value / 1000 * v.duration; });
  v.addEventListener('click', toggle); root.querySelector('.bigplay').addEventListener('click', toggle);
  v.addEventListener('dblclick', () => full());
  playB.addEventListener('click', toggle);
  loopB.addEventListener('click', () => { v.loop = !v.loop; flash(v.loop ? 'Looping' : 'Loop off'); sync(); });
  speedB.addEventListener('click', ev => { const r = speedB.getBoundingClientRect(); menu(r.left, r.top - 8 - SPEEDS.length * 34, SPEEDS.map(x => ({ label: `${x}×${x === 1 ? '  normal' : ''}`, icon: v.playbackRate === x ? 'play' : null, act: () => { v.playbackRate = x; keep(); } }))); });
  volB.addEventListener('click', () => { v.muted = !v.muted; keep(); });
  volR.addEventListener('input', () => { v.muted = false; v.volume = +volR.value; keep(); });
  const full = () => { const st = root; document.fullscreenElement ? document.exitFullscreen().catch(() => {}) : st.requestFullscreen?.().catch(() => {}); };
  root.querySelector('.fullb').addEventListener('click', full);
  const pip = root.querySelector('.pipb');
  if (!document.pictureInPictureEnabled) pip.hidden = true;
  pip.addEventListener('click', () => { document.pictureInPictureElement ? document.exitPictureInPicture().catch(() => {}) : v.requestPictureInPicture?.().catch(e => toast(e.message, true)); });
  root.addEventListener('keydown', e => {
    if (e.target.closest('input[type=range]') && /Arrow/.test(e.key)) return;
    if (e.ctrlKey || e.metaKey || e.altKey) return;
    const k = e.key.toLowerCase();
    if (k === ' ' || k === 'k') toggle(); else if (k === 'j') jump(-10); else if (k === 'l') jump(10);
    else if (k === 'arrowleft') jump(-5); else if (k === 'arrowright') jump(5);
    else if (k === 'arrowup') vol(.05); else if (k === 'arrowdown') vol(-.05);
    else if (k === 'm') { v.muted = !v.muted; flash(v.muted ? 'Muted' : 'Sound on'); keep(); }
    else if (k === 'f') full(); else if (k === '[' || k === '<') rate(-1); else if (k === ']' || k === '>') rate(1);
    else if (k === 'r') { v.loop = !v.loop; flash(v.loop ? 'Looping' : 'Loop off'); sync(); }
    else if (/^[0-9]$/.test(k) && v.duration) v.currentTime = v.duration * (+k / 10);
    else return;
    e.preventDefault();
  });
  root.addEventListener('pointerdown', () => root.focus({ preventScroll: true }));
  let idle = 0; root.addEventListener('pointermove', () => { root.classList.add('awake'); clearTimeout(idle); idle = setTimeout(() => root.classList.remove('awake'), 2200); });
  v.addEventListener('contextmenu', ev => { ev.preventDefault(); menu(ev.clientX, ev.clientY, [
    { label: v.paused ? 'Play' : 'Pause', icon: v.paused ? 'play' : 'pause', hint: 'Space', act: toggle },
    { label: v.loop ? 'Stop looping' : 'Loop', icon: 'loop', hint: 'R', act: () => { v.loop = !v.loop; sync(); } },
    { label: 'Picture in picture', icon: 'pip', act: () => pip.click() },
    { label: 'Download', icon: 'down', act: () => { const a = document.createElement('a'); a.href = `/api/files/raw?${q({ root: args.root, path: args.path, download: true })}`; a.download = args.name; a.click(); } },
    { label: 'Show in Files', icon: 'folder', act: () => openApp('files', { root: args.root, path: args.path.split('/').slice(0, -1).join('/') }) },
  ]); });
  // What browsers play as they are starts at once; the check runs beside
  // it, and only a file that needs rewrapping or converting is switched.
  const raw = `/api/files/raw?${q({ root: args.root, path: args.path })}`;
  if (/\.(mp4|m4v|webm|mov|mp3|m4a|ogg|opus|wav|flac|aac)$/i.test(args.path)) { v.src = raw; meta.textContent = 'Played as it is'; }
  else meta.textContent = 'Checking how to play it…';
  api.get(`/api/files/media?${q({ root: args.root, path: args.path })}`).then(info => {
    const k = info.playability?.kind;
    if (k === 'direct') { if (v.getAttribute('src') !== raw) v.src = raw; meta.textContent = 'Played as it is'; }
    else if (k === 'remux' || k === 'transcode') { v.src = `/api/files/stream?${q({ root: args.root, path: args.path })}`; meta.textContent = `${k === 'remux' ? 'Rewrapped' : 'Converted on this PC'} as it plays, for this browser${k === 'transcode' ? ' (seeking starts it again from there)' : ''}`; }
    else { meta.textContent = "This browser can't play it, and it can't be converted."; }
    v.playbackRate = prefs.rate; sync();
  }).catch(e => { meta.textContent = e.message; });
  remember({ kind: 'video', root: args.root, path: args.path, at: args.at || 0 });
  sync(); setTimeout(() => root.focus({ preventScroll: true }), 60);
  root.__close = () => { v.pause(); v.removeAttribute('src'); v.load(); };
  body.__close = root.__close;
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
      const r = el(`<div class="row"><span class="ic" style="--c:${pol ? '#7C5CE0' : '#2E9E62'}">${esc((f.name || f.id)[0].toUpperCase())}</span><b>${esc(titleOf(f))}${pol ? ' · POLARIS' : ''}</b><span class="meta">${f.state === 'running' && f.memory_mib != null ? mib(f.memory_mib) + ' now' : ''}${lim ? (f.state === 'running' ? ' · ' : '') + lim : ''}${f.state === 'foreign' ? 'Started outside PRISM, so PRISM only asks it, never stops it' : ''}${!f.available && f.unavailable_because ? esc(f.unavailable_because) : ''}</span><span class="state"><span class="tag ${tag[0]}">${tag[1]}</span></span></div>`);
      const st = r.querySelector('.state');
      r.addEventListener('contextmenu', ev => { ev.preventDefault(); serviceMenu(f, ev.clientX, ev.clientY, draw); });
      if (f.expose && (f.state === 'running' || f.state === 'foreign')) { const o = el(`<button class="btn">Open</button>`); o.addEventListener('click', () => openHosted(f.id)); st.appendChild(o); }
      if (f.state === 'running') { const s = el(`<button class="btn q">Stop</button>`); s.addEventListener('click', async () => { s.disabled = true; try { await api.post(`/api/facets/${f.id}/stop`); toast(`${f.name} stopped.`); } catch (e) { toast(e.message, true); } draw(); }); st.appendChild(s); }
      else if ((f.state === 'stopped' || f.state === 'failed') && f.available) { const s = el(`<button class="btn q">Start</button>`); s.addEventListener('click', async () => { s.disabled = true; if (f.expose) { await openHosted(f.id); } else { try { await api.post(`/api/facets/${f.id}/start`); toast(`${f.name} is starting.`); } catch (e) { toast(e.message, true); } } setTimeout(draw, 1500); }); st.appendChild(s); }
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
  try { vitals = await api.get('/api/vitals'); spark.push(vitals.honest_headroom_mib); while (spark.length > 60) spark.shift(); renderVitalsWidgets(); renderVitalsPane(); critical(vitals); } catch (e) {}
  setTimeout(pollVitals, document.hidden ? 10000 : 2000);
}
// **Critical Functions** (ADR 0002): at Red or Black, a page opened fresh
// is the rescue page; one already open isn't reloaded under the person
// (they may be mid-action) but says so, with the way there.
const DRIVER_SAID = { stall: 'the machine is stalling on memory', headroom: 'memory is running out', disk: 'the disk is nearly full', vram: "the graphics card's memory is full", spill: 'a model is spilling out of the graphics card' };
function critical(v) {
  const on = v.tier === 'red' || v.tier === 'black';
  let b = $('#critical');
  if (on && !b) {
    b = el(`<div class="critical" id="critical" role="alert"><span class="lamp" data-t="red"></span><span class="say"></span><a class="btn bad" href="/rescue">Open the rescue page</a><button class="b x" aria-label="Hide">${svg('x', 'currentColor', 2.6)}</button></div>`);
    b.querySelector('.x').addEventListener('click', () => b.classList.add('hidden'));
    document.body.appendChild(b); requestAnimationFrame(() => b.classList.add('on'));
  }
  if (b) {
    b.querySelector('.say').textContent = on ? `PRISM has stepped down: ${DRIVER_SAID[v.driver] || 'the PC is under pressure'}. It's stopping what it started if it has to. The rescue page needs nothing else to work.` : '';
    if (!on) { b.classList.remove('on'); setTimeout(() => b.remove(), 400); }
  }
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

/* ── Solis: POLARIS's agent, through PRISM (prismd/src/solis.rs) ───── */
// A text conversation over the gateway's own protocol: hello, then items;
// the answer arrives in pieces. Closing the pane leaves the conversation in
// POLARIS, so it carries on from any device.
function solisApp(args, body) {
  const root = el(`<div class="solis"><div class="sky2" aria-hidden="true"></div><div class="talk" aria-live="polite"></div><form class="say"><textarea rows="1" placeholder="Ask Solis anything" aria-label="Ask Solis"></textarea><button class="send" type="submit" aria-label="Send">${svg('send', '#fff', 2.4)}</button></form></div>`);
  const talk = root.querySelector('.talk'), ta = root.querySelector('textarea'), send = root.querySelector('.send');
  let ws = null, closed = false, ready = false, live = null, busy = false, tries = 0;
  const id = () => 'evt_' + Math.random().toString(36).slice(2, 10);
  const bubble = (who, text) => { const b = el(`<div class="msg ${who}"><p></p></div>`); b.querySelector('p').textContent = text; talk.appendChild(b); talk.scrollTop = talk.scrollHeight; return b; };
  const note = html => { const n = el(`<div class="note">${html}</div>`); talk.appendChild(n); return n; };
  const setBusy = on => { busy = on; send.innerHTML = svg(on ? 'stop' : 'send', '#fff', 2.4); send.setAttribute('aria-label', on ? 'Stop' : 'Send'); root.classList.toggle('thinking', on); };
  async function connect() {
    if (closed) return;
    let here = null; try { here = await api.get('/api/solis'); } catch (e) {}
    if (!here?.up) {
      talk.innerHTML = '';
      // POLARIS's runtime, when POLARIS made it one of PRISM's services
      // (its streamed Apps start it too): one press wakes Solis.
      await loadServices();
      const rt = facets.find(f => f.id === 'polaris-runtime');
      if (rt && rt.state !== 'running') {
        const n = note(`<b>Solis is asleep</b><p>POLARIS isn't running on ${esc(state.host)}. PRISM can start it here, without a window, so Solis can answer.</p><button class="btn wake">Wake Solis</button>`);
        n.querySelector('.wake').addEventListener('click', async ev => {
          ev.target.disabled = true; ev.target.textContent = 'Waking…';
          try { await api.post('/api/facets/polaris-runtime/start'); } catch (e) { toast(e.message, true); ev.target.disabled = false; ev.target.textContent = 'Wake Solis'; }
        });
      } else {
        note(`<b>Solis isn't here yet</b><p>Solis lives in POLARIS on ${esc(state.host)}. ${rt ? 'POLARIS is starting.' : 'Start POLARIS there, and in its Settings › Clients, turn on the gateway.'} This page connects by itself once it's up.</p>`);
      }
      setTimeout(connect, 5000); return;
    }
    ws = new WebSocket(`${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}/api/solis/realtime`);
    ws.onopen = () => { tries = 0; ws.send(JSON.stringify({ type: 'session.hello', event_id: id(), protocol: { min: '7.0.0', max: '7.0.0' }, client: { type: 'web', version: 'prism', instance_id: 'prism_' + (state.me || 'owner') }, capabilities: ['input.text', 'conversation.history'] })); };
    ws.onmessage = ev => {
      let m; try { m = JSON.parse(ev.data); } catch (e) { return; }
      if (m.type === 'session.ready') { ready = true; if (!talk.querySelector('.msg')) { talk.innerHTML = ''; note(`<b>Solis is here</b><p>Type, and Solis answers. What you say is kept in POLARIS, not in this browser.</p>`); } ta.focus(); }
      else if (m.type === 'conversation.item.input_audio_transcription.completed') bubble('me', m.transcript);
      else if (m.type === 'response.created') { setBusy(true); live = bubble('them', ''); live.classList.add('live'); }
      else if (m.type === 'response.output_audio_transcript.delta') { if (!live) live = bubble('them', ''); live.querySelector('p').textContent += m.delta; talk.scrollTop = talk.scrollHeight; }
      else if (m.type === 'response.output_audio_transcript.done') { if (live) live.querySelector('p').textContent = m.transcript; }
      else if (m.type === 'response.done') { setBusy(false); if (live) { live.classList.remove('live'); if (m.status === 'cancelled') live.classList.add('cut'); if (!live.querySelector('p').textContent) live.remove(); } live = null; }
      else if (m.type === 'session.ping') ws.send(JSON.stringify({ type: 'session.pong', event_id: id() }));
      else if (m.type === 'error') { setBusy(false); note(`<p>${esc(m.error?.message || 'Something went wrong.')}</p>`); }
    };
    ws.onclose = () => { ready = false; setBusy(false); if (!closed) setTimeout(connect, Math.min(1000 * 2 ** tries++, 10000)); };
  }
  function submit() {
    if (busy) { ws?.send(JSON.stringify({ type: 'response.cancel', event_id: id() })); return; }
    const text = ta.value.trim(); if (!text || !ready) return;
    bubble('me', text); ta.value = ''; ta.style.height = '';
    ws.send(JSON.stringify({ type: 'conversation.item.create', event_id: id(), item: { type: 'message', role: 'user', content: [{ type: 'input_text', text }] } }));
    setBusy(true);
  }
  root.querySelector('form').addEventListener('submit', e => { e.preventDefault(); submit(); });
  ta.addEventListener('keydown', e => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); submit(); } });
  ta.addEventListener('input', () => { ta.style.height = ''; ta.style.height = Math.min(160, ta.scrollHeight) + 'px'; });
  note('<p>Reaching Solis…</p>');
  connect();
  body.__close = () => { closed = true; ws?.close(); };
  return root;
}

/* ── A service's own page, through PRISM ─────────────────────────────── */
// Each hosted app is served at the root of a port of its own (ports.rs), so
// apps that write absolute paths (ComfyUI) work; /facet/<id>/ is the
// fallback. Same session either way: the browser sends this host's cookies
// whatever the port.
function webApp(args, body, pane) {
  const id = pane.id.slice(4), f0 = facets.find(x => x.id === id);
  const name = f0 ? (f0.expose?.title || f0.name) : id;
  const root = el(`<div class="webwrap"><iframe class="webpane" title="${esc(name)}" allow="fullscreen; clipboard-read; clipboard-write; autoplay"></iframe><div class="waiting"><span class="orb"><i></i><i></i><i></i></span><b>Starting ${esc(name)}…</b><p class="secs"></p></div></div>`);
  const fr = root.querySelector('iframe'), wait = root.querySelector('.waiting'), secs = root.querySelector('.secs');
  let closed = false;
  const t0 = performance.now();
  const tick = setInterval(() => { const n = Math.round((performance.now() - t0) / 1000); secs.textContent = n < 3 ? '' : n < 25 ? `${n} s` : `${n} s · some apps take a minute the first time`; }, 500);
  // Shown only once it answers: a page opened before the app is up is a
  // blank one.
  (async () => {
    // Gone from this PC (removed since the window was saved): said, not waited on.
    await loadServices();
    if (!facets.some(f => f.id === id)) {
      clearInterval(tick); wait.querySelector('.orb').remove();
      wait.querySelector('b').textContent = `${name} isn't on this PC any more`;
      secs.textContent = 'Its service was removed. Close this window, or find what replaced it on Home.';
      return;
    }
    while (!closed && !(await answers({ id }))) await new Promise(r => setTimeout(r, 600));
    if (closed) return;
    let src = `/facet/${encodeURIComponent(id)}/`;
    try { const r = await api.get(`/api/facets/${encodeURIComponent(id)}/port`); src = `${location.protocol}//${location.hostname}:${r.port}/`; } catch (e) {}
    fr.addEventListener('load', () => { clearInterval(tick); wait.classList.add('gone'); setTimeout(() => wait.remove(), 400); }, { once: true });
    fr.src = src;
  })();
  body.__close = () => { closed = true; clearInterval(tick); };
  return root;
}
// Open a hosted app: start it if it's stopped, and when it's a launcher that
// waits for answers in a terminal (a pty), show that terminal, since a page
// isn't there until it's answered.
async function openHosted(id) {
  await loadServices();
  const f = facets.find(x => x.id === id); if (!f) return;
  const pol = /face-stream/.test(f.command || '');
  let running = f.state === 'running' || f.state === 'foreign';
  if (!running) {
    if (!(f.state === 'stopped' || f.state === 'failed') || !f.available) { toast(f.unavailable_because || `${f.name} isn't running.`, true); return; }
    // The window opens at once and says it's starting; the tile glows meanwhile.
    $$(`[data-facet="${CSS.escape(f.id)}"]`).forEach(t => t.classList.add('starting'));
    if (!f.pty) openApp('web:' + f.id);
    try { await api.post(`/api/facets/${f.id}/start`); } catch (e) { toast(`${f.name} didn't start: ${e.message}`, true); closePane('web:' + f.id); return; }
    finally { setTimeout(() => $$(`[data-facet="${CSS.escape(f.id)}"]`).forEach(t => t.classList.remove('starting')), 1500); }
  }
  if (f.pty) {
    const sess = (await api.get('/api/term').catch(() => ({ sessions: [] }))).sessions.find(s => s.title === f.name && !s.exited);
    if (sess && !(await answers(f))) {
      toast(`${f.name} is asking something in its terminal. Answer it there; its page opens once it's up.`);
      openApp('terminal', { sid: sess.id });
      watchUp(f);
      return;
    }
  }
  openApp('web:' + f.id);
}
// Does the app's page answer yet?
async function answers(f) {
  try { const r = await fetch(`/facet/${encodeURIComponent(f.id)}/`, { method: 'HEAD', credentials: 'same-origin' }); return r.status < 500 && r.status !== 404; } catch (e) { return false; }
}
function watchUp(f) {
  let n = 0;
  const t = setInterval(async () => {
    if (++n > 120) return clearInterval(t);
    if (await answers(f)) { clearInterval(t); toast(`${f.name} is up.`); openApp('web:' + f.id); }
  }, 3000);
}
// What can be done with a service, wherever it shows (a tile, a row, a pane).
function serviceMenu(f, x, y, after) {
  const running = f.state === 'running', ext = f.state === 'foreign';
  const done = () => { loadServices().then(() => { renderHosted(); after && after(); }); };
  menu(x, y, [
    f.expose && { label: running || ext ? 'Open' : 'Start and open', icon: 'globe', act: () => openHosted(f.id) },
    !running && !ext && (f.state === 'stopped' || f.state === 'failed') && f.available && { label: 'Start', icon: 'play', act: async () => { try { await api.post(`/api/facets/${f.id}/start`); toast(`${f.name} is starting.`); } catch (e) { toast(e.message, true); } setTimeout(done, 1200); } },
    running && { label: 'Restart', icon: 'again', act: async () => { try { await api.post(`/api/facets/${f.id}/stop`); await new Promise(r => setTimeout(r, 800)); await api.post(`/api/facets/${f.id}/start`); toast(`${f.name} restarted.`); } catch (e) { toast(e.message, true); } setTimeout(done, 1200); } },
    f.pty && running && { label: 'Its terminal', icon: 'prompt', act: async () => { const sess = (await api.get('/api/term').catch(() => ({ sessions: [] }))).sessions.find(s => s.title === f.name && !s.exited); sess ? openApp('terminal', { sid: sess.id }) : toast(`${f.name} has no terminal open.`, true); } },
    running && 'sep',
    running && { label: 'Stop', icon: 'stop', danger: true, act: async () => { try { await api.post(`/api/facets/${f.id}/stop`); toast(`${f.name} stopped.`); closePane('web:' + f.id); } catch (e) { toast(e.message, true); } done(); } },
    ext && { label: 'Started outside PRISM, so PRISM never stops it', disabled: true },
  ]);
}

boot();
