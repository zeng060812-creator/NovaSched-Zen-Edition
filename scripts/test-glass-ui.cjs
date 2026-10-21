'use strict';
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const root = path.resolve(__dirname, '..');
const read = p => fs.readFileSync(path.join(root,p),'utf8');
const html = read('module-template/webroot/index.html');
const ui = read('module-template/webroot/assets/app.js');
const css = read('module-template/webroot/assets/app.css');
const glass = read('module-template/webroot/assets/liquid-glass.js');
const ws = read('native/src/websocket.rs');
let count=0;
function check(name,fn) { fn();count++;console.log('PASS:',name); }
check('liquid glass renderer ships the refraction shader with no vendored copy',()=>{
  for(const marker of ['sdRoundedRect','gradSdRoundedRect','circleMap','refractContent','dispersionIntensity','recordScene','getTexture'])
    assert.ok(glass.includes(marker),'shader marker missing: '+marker);
  assert.ok(glass.includes('Apache-2.0'),'upstream attribution missing');
  assert.ok(!fs.existsSync(path.join(root,'module-template/webroot/vendor')),'vendor directory should be gone');
});
check('four tab pages, mode grid, NovaFAS engine card and diagnostics present',()=>{
  for(const page of ['overview','rules','logs','about']) assert(html.includes(`id="page-${page}"`));
  for(const id of ['stat-pkg-raw','scene-diagnose','auto-switch','dialog-layer','glass-level','glass-canvas','mode-grid','fas-engine-title','fas-engine-meter','about-engine-version']) assert(html.includes(`id="${id}"`));
  assert(!html.includes('id="page-modes"'),'mode grid now lives on overview');
});
check('no remote assets needed for the live UI',()=>{
  assert(!/<(?:script|link)[^>]+(?:src|href)="https?:/.test(html));
  assert(!/url\(\s*https?:/.test(css));
});
check('authenticated protocol and root fallback still match native core',()=>{
  const key=ui.match(/const KEY = "([^"]+)"/)[1];
  assert(ws.includes(`"${key}"`));
  assert(ui.includes('[KEY, AUTH_PREFIX + sessionToken]'));
  assert(ui.includes('moduleCommand("webui-session"'));
  assert(ui.includes('moduleCommand("webui-rpc"'));
  assert(ui.includes('s.discovered = true;\n            bindSocket'));
});
check('Scene priority and unknown detection reflected in UI',()=>{
  assert(ui.includes('autoSwitch.disabled = !editable() || !!s.pending || s.linked'));
  assert(ui.includes('s.scenePresence === "unknown"'));
  assert(ws.includes('scenePresence:{}'));
});
check('extreme power is persisted, bounded by CPU OPPs and reports readback state',()=>{
  const scheduler=read('native/src/scheduler.rs');
  assert(html.includes('id="extreme-switch"'));
  assert(html.includes('降低性能上限，延长续航'));
  assert(ui.includes('extreme\\t${enabled ? "1" : "0"}'));
  assert(ui.includes('s.extremeStatus'));
  assert(ws.includes('"extreme\\t1" | "extreme\\t0"'));
  assert(scheduler.includes('conservative_frequency_ceiling'));
  assert(scheduler.includes('scaling_available_frequencies'));
});
check('dialog accessibility, motion preference and safe areas',()=>{
  assert(ui.includes('dialogFocus'));
  assert(ui.includes('event.key !== "Tab"'));
  assert(css.includes('prefers-reduced-motion'));
  assert(css.includes('safe-area-inset-bottom'));
});
check('symmetric motion system: one token set drives open and close',()=>{
  assert(css.includes('--ease-out:cubic-bezier(.32,.72,0,1)'));
  assert(ui.includes('frames : [...frames].reverse()') || ui.includes('[...frames].reverse()'));
  assert(ui.includes('animatePageSwitch'));
  assert(css.includes('.page.leaving'));
  assert(ui.includes('reducedMotion()'));
});
check('glass renderer degrades without WebGL and honours reduced motion',()=>{
  assert(glass.includes('glass-flat'));
  assert(glass.includes('prefers-reduced-motion'));
  assert(glass.includes('webglcontextlost'));
});
check('fixed screen shell with independent scroll and host-provided CSS insets',()=>{
  assert(css.includes('overflow:hidden;overscroll-behavior:none'));
  assert(css.includes('--app-height'));
  assert(css.includes('--safe-area-inset-top,var(--window-inset-top'));
  assert(css.includes('overflow-x:hidden;overflow-y:auto;overscroll-behavior:none;touch-action:pan-y'));
  assert(!css.includes('--scroll-offset'));
  assert(!ui.includes('window.scrollTo('));
  assert(html.includes('assets/host.js'));
});
check('release and backend identities stay separate',()=>{
  assert(html.includes('id="about-engine-version"'));
  assert(ui.includes('"v" + UI_VERSION'));
  assert(ui.includes('s.version !== UI_VERSION'));
  assert(html.includes('assets/version.js'));
});
check('FAS readiness, engine status and pending ceilings are separate',()=>{
  assert(ui.includes('data.fasState'));
  assert(ui.includes('s.fasSync === "synced"'));
  assert(ui.includes('s.pendingControls === 0'));
  assert(ui.includes('data.fasEngine'));
  assert(ws.includes('fasEngine:{}'));
  assert(ws.includes('fasState:{}'));
  assert(ws.includes('pendingControls:{}'));
});
for(const mode of ['powersave','balance','performance','fast'])
  check('mode '+mode,()=>assert(html.includes(`data-mode="${mode}"`)));
for(const file of fs.readdirSync(path.join(root,'module-template/config'))){
  check('profile '+file,()=>{
    const cfg=JSON.parse(fs.readFileSync(path.join(root,'module-template/config',file),'utf8'));
    assert.equal(cfg.meta.author,'ZenJooo');
    assert.deepEqual(Object.keys(cfg.modes).sort(),['balance','fast','performance','powersave']);
    assert.ok(cfg.features.fas,'NovaFAS block missing');
    assert.equal(cfg.features.feed_video,undefined,'feed lockframe config must be removed');
    assert.ok(cfg.features.fas.enabled,'NovaFAS disabled');
    for(const mode of ['powersave','balance','performance','fast'])
      assert.ok(cfg.features.fas.modes[mode],`NovaFAS ${mode} limits missing`);
    const generation=Number(fs.readFileSync(path.join(root,'native/src/config.rs'),'utf8')
      .match(/PROFILE_VERSION: i64 = (\d+)/)[1]);
    assert.equal(cfg.meta.version,generation);
  });
}
console.log(`${count} checks passed`);
