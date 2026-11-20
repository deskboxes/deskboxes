// Record art.html's scenes with headless Chrome (DevTools protocol, its own throwaway profile in %TEMP%):
//   out\NAME_THEME.png      the still (frame 0) at full size, for export.py
//   res\settings\NAME[_l].mp4  the clip (scenes with a duration), 640x360 H.264, played on hover in Settings
// node tools/art/render.mjs [scene ...] [--themes dark,light] [--no-clips] [--fps 30]
// Needs Chrome and ffmpeg on PATH.
import { spawn, execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const A = path.dirname(fileURLToPath(import.meta.url));
const RES = path.resolve(A, '..', '..', 'res', 'settings');
const ALL = ['home', 'boxes', 'look', 'icons', 'tabs', 'rollup', 'rules', 'az', 'blend', 'colors', 'folder', 'above', 'hide', 'saved', 'search', 'locks', 'privacy'];
const args = process.argv.slice(2);
const opt = (n, d) => { const i = args.indexOf(n); return i < 0 ? d : args.splice(i, 2)[1]; };
const themes = opt('--themes', 'dark,light').split(',');
const fps = +opt('--fps', '30');
const noClips = args.includes('--no-clips') ? args.splice(args.indexOf('--no-clips'), 1) : false;
const scenes = args.length ? args : ALL;
const VW = 640, VH = 360;

const chrome = 'C:/Program Files/Google/Chrome/Application/chrome.exe';
const prof = path.join(os.tmpdir(), 'deskboxes-art-chrome' + (process.env.ART_PORT || ''));
const port = +(process.env.ART_PORT || 9333);
const proc = spawn(chrome, ['--headless=new', `--remote-debugging-port=${port}`, `--user-data-dir=${prof}`, '--hide-scrollbars', '--no-first-run', '--disable-frame-rate-limit', '--disable-gpu-vsync', '--disable-background-timer-throttling', '--disable-renderer-backgrounding', '--disable-backgrounding-occluded-windows',
  '--force-device-scale-factor=1', ...(process.env.ART_GPU ? ['--use-angle=d3d11', '--enable-gpu-rasterization', '--ignore-gpu-blocklist'] : ['--disable-gpu']), '--allow-file-access-from-files', '--window-size=1920,1080', 'about:blank'], { stdio: 'ignore' });
process.on('exit', () => { try { proc.kill(); } catch { } });

async function target() {
  for (let i = 0; i < 100; i++) {
    try {
      const l = await (await fetch(`http://127.0.0.1:${port}/json/list`)).json();
      const p = l.find(x => x.type === 'page');
      if (p) return p.webSocketDebuggerUrl;
    } catch { }
    await new Promise(r => setTimeout(r, 150));
  }
  throw new Error('no Chrome');
}
const ws = new WebSocket(await target());
await new Promise((r, j) => { ws.onopen = r; ws.onerror = j; });
let id = 0;
const wait = new Map(), events = [];
ws.onmessage = m => {
  const d = JSON.parse(m.data);
  if (d.id && wait.has(d.id)) { const [r, j] = wait.get(d.id); wait.delete(d.id); d.error ? j(new Error(JSON.stringify(d.error))) : r(d.result); }
  else if (d.method) events.push(d);
};
const send = (method, params = {}) => new Promise((r, j) => { const i = ++id; wait.set(i, [r, j]); ws.send(JSON.stringify({ id: i, method, params })); });
const run = async expr => {
  const r = await send('Runtime.evaluate', { expression: expr, awaitPromise: true, returnByValue: true });
  if (r.exceptionDetails) throw new Error(expr + ': ' + JSON.stringify(r.exceptionDetails).slice(0, 400));
  return r.result.value;
};
const paint = () => run('new Promise(r => requestAnimationFrame(() => requestAnimationFrame(() => r(1))))');

await send('Page.enable');
await send('Runtime.enable');
fs.mkdirSync(path.join(A, 'out'), { recursive: true });
const url = pathToFileURL(path.join(A, 'art.html')).href;
for (const s of scenes) {
  for (const th of themes) {
    await send('Page.navigate', { url: 'about:blank' });
    await send('Page.navigate', { url: `${url}#scene=${s}&theme=${th}` });
    await new Promise(r => setTimeout(r, 300));
    await run('new Promise(r => { const f = () => window.ready ? window.ready.then(r) : setTimeout(f, 50); f(); })');
    const meta = await run('META');
    await send('Emulation.setDeviceMetricsOverride', { width: meta.w, height: meta.h, deviceScaleFactor: 1, mobile: false });
    await run('frame(0)'); await paint(); await new Promise(r => setTimeout(r, 150));
    const still = await send('Page.captureScreenshot', { format: 'png' });
    fs.writeFileSync(path.join(A, 'out', `${s}_${th}.png`), Buffer.from(still.data, 'base64'));
    if (!meta.dur || noClips) { console.log(s, th, 'still'); continue; }
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'dbx-art-'));
    const n = +(process.env.ART_FRAMES || Math.round(meta.dur * fps));
    const sup = +(process.env.ART_SUP || 2);
    if (process.env.ART_NOBLUR) await run(`document.head.insertAdjacentHTML('beforeend', '<style>*{backdrop-filter:none!important}</style>')`);
    await send('Emulation.setDeviceMetricsOverride', { width: meta.w, height: meta.h, deviceScaleFactor: sup * VW / meta.w, mobile: false });
    for (let i = 0; i < n; i++) {
      const t0 = Date.now(); await run(`frame(${i / fps})`); const t1 = Date.now(); if (!process.env.ART_NOPAINT) await paint(); const t2 = Date.now();
      const f = await send('Page.captureScreenshot', { format: 'png', optimizeForSpeed: true, fromSurface: true });
      fs.writeFileSync(path.join(dir, `f${String(i).padStart(4, '0')}.png`), Buffer.from(f.data, 'base64'));
      if (process.env.ART_TIME) console.log('frame', t1 - t0, 'paint', t2 - t1, 'shot', Date.now() - t2);
    }
    const mp4 = path.join(RES, s + (th === 'light' ? '_l' : '') + '.mp4');
    execFileSync('ffmpeg', ['-y', '-loglevel', 'error', '-framerate', String(fps), '-i', path.join(dir, 'f%04d.png'), '-vf', `scale=${VW}:${VH}:flags=lanczos`,
      '-c:v', 'libx264', '-preset', 'veryslow', '-crf', '24', '-pix_fmt', 'yuv420p', '-profile:v', 'main', '-movflags', '+faststart', '-an', mp4]);
    fs.rmSync(dir, { recursive: true, force: true });
    console.log(s, th, n, 'frames', Math.round(fs.statSync(mp4).size / 1024), 'KB');
  }
}
ws.close();
proc.kill();
process.exit(0);
