/* Exercise the shipped UI against a synthetic service; no phone is contacted. */
'use strict';
const vm = require('node:vm');
const fs = require('node:fs');
const path = require('node:path');
const assert = require('node:assert/strict');
const {setTimeout:nativeTimeout, clearTimeout:nativeClearTimeout} = require('node:timers');
const root = path.resolve(__dirname,'..');
const mockHost = require('./webui-fixture.cjs');
const web = path.join(root,'module-template/webroot');
async function main() {
const {parseHTML} = await import('./test-support/linkedom.mjs');
function fixture(options) {
  const {window:domWindow, document:d} = parseHTML(fs.readFileSync(path.join(web,'index.html'),'utf8'));
  const w=Object.create(domWindow);
  Object.defineProperties(w,{window:{value:w},self:{value:w},globalThis:{value:w},document:{value:d},visualViewport:{value:undefined,writable:true,configurable:true},NovaHost:{value:undefined,writable:true,configurable:true},NovaRelease:{value:undefined,writable:true,configurable:true}});
  const timers=new Set(), storage=new Map();
  const later=(fn,ms=0)=>{const id=nativeTimeout(()=>{timers.delete(id);fn();},ms);timers.add(id);return id;};
  const cancel=id=>{nativeClearTimeout(id);timers.delete(id);};
  Object.defineProperty(w,"navigator",{value:{},configurable:true,writable:true});
  Object.assign(w,{console, URL, TextEncoder, TextDecoder, AbortController, performance, Date, Promise,
    location:{href:'http://127.0.0.1:8765/',origin:'http://127.0.0.1:8765',protocol:'http:',host:'127.0.0.1:8765',hostname:'127.0.0.1',pathname:'/'},
    localStorage:{getItem:key=>storage.get(key)||null,setItem:(key,value)=>storage.set(key,value)},
    navigator:{vibrate(){},clipboard:{async writeText(text){w.copied=text;}}},
    innerWidth:393,innerHeight:852,devicePixelRatio:1,scrollY:0,scrollX:0,
    matchMedia:query=>({matches:!options.motion&&query.includes('prefers-reduced-motion'),addEventListener(){}}),
    setTimeout:later,clearTimeout:cancel,setInterval,clearInterval,
    requestAnimationFrame:fn=>later(()=>fn(performance.now()),16),cancelAnimationFrame:cancel,
    scrollTo(options){w.scrollY=options.top||0;},
    Image:class {constructor(){this.naturalWidth=941;this.naturalHeight=1672;}set src(value){this._src=value;later(()=>this.onload?.());}get src(){return this._src;}},
    ResizeObserver:class {observe(){}unobserve(){}disconnect(){}}
  });
  if(options.viewport) {
    const viewport=d.createElement("span");
    Object.assign(viewport,{height:options.viewport.height,offsetTop:0,scale:1});
    w.visualViewport=viewport;
  }
  const states=[];let cursor=-1;
  w.history={get state(){return states[cursor];},replaceState(state){states[cursor=Math.max(0,cursor)]=state;},pushState(state){states.splice(cursor+1);states[++cursor]=state;},back(){if(cursor<=0)return;cursor--;const event=new w.Event('popstate');event.state=states[cursor];later(()=>w.dispatchEvent(event));}};
  Object.defineProperty(d,'visibilityState',{value:'visible'});Object.defineProperty(d,'hidden',{value:false});d.activeElement=d.body;
  w.HTMLElement.prototype.focus=function(){d.activeElement=this;};
  const animations=new Set();
  w.HTMLElement.prototype.animate=function(){const a={cancel(){animations.delete(a);},onfinish:null,oncancel:null,finished:Promise.resolve()};animations.add(a);return a;};
  w.HTMLElement.prototype.getBoundingClientRect=function(){
    const page=this.closest('.page'),dialog=this.closest('.dialog-layer,.appearance-layer');
    const visible=(!page||page.classList.contains('active')||page.classList.contains('leaving'))&&(!dialog||dialog.classList.contains('open'))&&!this.closest('[hidden]');
    return {x:0,y:0,left:0,top:0,right:visible?320:0,bottom:visible?100:0,width:visible?320:0,height:visible?100:0};
  };
  w.HTMLElement.prototype.getClientRects=function(){const r=this.getBoundingClientRect();return r.width?[r]:[];};
  w.HTMLElement.prototype.scrollIntoView=function(){};
  w.HTMLCanvasElement.prototype.getContext=function(kind){
    if(kind==='webgl')return null;
    return {save(){},restore(){},clearRect(){},fillRect(){},drawImage(){},scale(){},fillText(){},getImageData:()=>({data:new Uint8ClampedArray(4096)})};
  };
  w.NovaGlass=undefined;
  const context=vm.createContext(w);
  vm.runInContext(`(${mockHost.toString()})(${JSON.stringify(options)})`,context);
  for(const file of ['version.js','host.js','haptics.js','liquid-glass.js','app.js'])vm.runInContext(fs.readFileSync(path.join(web,'assets',file),'utf8'),context,{filename:file});
  return {w,d,animations,cleanup:()=>timers.forEach(nativeClearTimeout)};
}
async function until(fn,timeout=4000){const end=Date.now()+timeout;while(!fn()){if(Date.now()>end)throw new Error('Timed out waiting for UI state');await new Promise(resolve=>nativeTimeout(resolve,10));}}
const pause=()=>new Promise(resolve=>nativeTimeout(resolve,30));
function click(d,query){const node=d.querySelector(query);assert(node,query+' missing');assert(!node.disabled,query+' disabled');node.click();}
for(const options of [{linked:true,automatic:true,presence:'available'},{presence:'absent'},{presence:'unknown'},{external:true,presence:'available'},{root:true,presence:'absent'},{labels:false,presence:'absent'},{samsung:true,presence:'absent',fasEngine:'off',fasState:'absent'}]) {
  const {w,d,cleanup}=fixture(options);
  try {
    await until(()=>['已连接','外部接管'].includes(d.querySelector('#nav-cta-text').textContent));
    await until(()=>d.querySelectorAll('.rule-row').length===1);
    assert.equal(d.querySelectorAll('.dock button[data-page]').length,4);
    assert.equal(d.querySelector('#stat-pkg-raw').textContent,'com.omarea.vtools');
    if(options.linked)assert(d.querySelector('#auto-switch').disabled);
    assert.equal(d.querySelector('#samsung-compat').hidden, !options.samsung);
    if(options.samsung){
      assert.equal(d.querySelector('#fas-engine-title').textContent,'三星兼容策略');
      assert(d.querySelector('#fas-engine-desc').textContent.includes('不采样 SurfaceFlinger'));
    }
    if(options.presence==='unknown')assert.equal(d.querySelector('#stat-scene').textContent,'Scene 检测暂不可用');
    if(options.presence==='absent')assert.equal(d.querySelector('#stat-scene').textContent,'WebUI 接管');
    click(d,'.dock [data-page="about"]');click(d,'#appearance-settings');
    assert(d.querySelector('#appearance-layer').classList.contains('open'));
    click(d,'[data-theme-choice="dark"]');assert.equal(d.documentElement.dataset.theme,'dark');
    for(const [id,value,label,expected] of [['glass-level','90','glass-value','90'],['wall-bright','35','wall-bright-val','35%'],['card-alpha','18','card-alpha-val','18%'],['card-radius','34','card-radius-val','34']]){
      const slider=d.querySelector('#'+id);slider.value=value;slider.dispatchEvent(new w.Event('input'));slider.dispatchEvent(new w.Event('change'));assert.equal(d.querySelector('#'+label).textContent,expected);
    }
    click(d,'[data-card-color="#ffbfd1"]');assert.equal(d.querySelector('[data-color-mode="solid"]').getAttribute('aria-pressed'),'true');
    click(d,'#dispersion-switch');assert.equal(d.querySelector('#dispersion-switch').getAttribute('aria-checked'),'false');
    click(d,'#personalize-close');await pause();
    click(d,'.dock [data-page="overview"]');
    if(options.external)assert(d.querySelector('[data-mode="fast"]').disabled);
    else {click(d,'[data-mode="fast"]');await until(()=>d.querySelector('[data-mode="fast"]').dataset.state==='active');assert.equal(w.fixture.effective,'fast');}
    click(d,'.dock [data-page="rules"]');
    assert.equal(d.querySelector('.page.active').id,'page-rules');
    if(!options.external){
      click(d,'#add-rule');const input=d.querySelector('#dialog-input');input.value='invalid name';click(d,'#dialog-save');assert(!d.querySelector('#dialog-error').hidden);
      input.value='com.example.test';click(d,'[data-chip="performance"]');click(d,'#dialog-save');await until(()=>!d.querySelector('#dialog-layer').classList.contains('open'));await pause();assert.equal(d.querySelectorAll('.rule-row').length,2);
      const search=d.querySelector('#rule-search');search.value='com.example.test';search.dispatchEvent(new w.Event('input'));assert.equal(d.querySelectorAll('.rule-row').length,1);
      click(d,'[data-edit="com.example.test"]');click(d,'#dialog-delete');click(d,'#dialog-confirm');await until(()=>!d.querySelector('#dialog-layer').classList.contains('open'));await pause();
      search.value='';search.dispatchEvent(new w.Event('input'));assert.equal(d.querySelectorAll('.rule-row').length,1);
    }
    click(d,'.dock [data-page="overview"]');click(d,'#scene-diagnose');assert(d.querySelector('main').inert);click(d,'#dialog-run');await until(()=>!d.querySelector('#dialog-report').hidden);assert(d.querySelector('#dialog-report').textContent.includes('scene_presence=available'));
    click(d,'#dialog-close');await pause();
    click(d,'.dock [data-page="logs"]');await until(()=>d.querySelectorAll('#log-list article').length>=2);
    click(d,'.dock [data-page="about"]');click(d,'#about-license');assert(d.querySelector('#dialog-body').textContent.includes('NovaFAS'));click(d,'#dialog-close');
    assert(d.body.classList.contains('glass-flat'));
    console.log('PASS',JSON.stringify(options),'navigation, preferences, rules, diagnostics, logs, controller guards');
  } finally {cleanup();}
}

function pointer(w,node,type,x,y=20) {
  const event=new w.Event(type,{bubbles:true,cancelable:true});
  Object.assign(event,{button:0,pointerId:1,clientX:x,clientY:y});
  node.dispatchEvent(event);
}
for(const options of [{insets:'modern',viewport:{height:852}},{insets:'legacy',viewport:{height:852}},{motion:true}]) {
  const {w,d,cleanup}=fixture(options);
  try {
    await until(()=>d.querySelector('#nav-cta-text').textContent==='已连接');
    if(options.insets)assert.equal(w.fixture.edgeToEdge,true);
    assert.equal(d.documentElement.style.getPropertyValue('--app-height'),'852px');
    const nav=d.querySelector('.dock'),tab=d.querySelector('.dock [data-page="logs"]');
    let captures=0,releases=0,captured=false;
    nav.setPointerCapture=()=>{captures++;captured=true;};nav.hasPointerCapture=()=>captured;
    nav.releasePointerCapture=()=>{releases++;captured=false;};
    pointer(w,tab,'pointerdown',60);assert.equal(captures,0,'a tap must keep its button target');
    pointer(w,tab,'pointerup',60);tab.click();assert.equal(d.querySelector('.page.active').id,'page-logs');
    pointer(w,tab,'pointerdown',60);pointer(w,tab,'pointermove',80);assert.equal(captures,1);
    pointer(w,nav,'pointercancel',80);assert.equal(releases,1);
    pointer(w,d.querySelector('#main'),'pointerdown',200,100);
    pointer(w,d.querySelector('#main'),'pointerup',40,102);
    assert.equal(d.querySelector('.page.active').id,'page-logs','moving content must not change the route');
    const main=d.querySelector('#main');main.scrollTop=132;
    click(d,'.dock [data-page="about"]');main.scrollTop=74;click(d,'.dock [data-page="rules"]');
    await pause();assert.equal(main.scrollTop,0);
    click(d,'.dock [data-page="about"]');await pause();
    assert.equal(main.scrollTop,74);assert.equal(w.scrollY,0);
    click(d,'#about-license');assert.equal(d.querySelector('#dock-wrap').inert,true);
    click(d,'#dialog-close');await until(()=>!d.querySelector('#dialog-layer').classList.contains('open'));
    assert.equal(d.querySelector('#dock-wrap').inert,false);
    if(w.visualViewport){w.visualViewport.height=560;w.visualViewport.dispatchEvent(new w.Event('resize'));await until(()=>d.documentElement.style.getPropertyValue('--app-height')==='560px');}
    console.log('PASS fixed shell, host insets, tap/drag separation, route scroll and modal release',JSON.stringify(options));
  } finally {cleanup();}
}
for(const bridgeStyle of ['native0','native2','callback2','promise1','sync1']) {
  const {w,d,cleanup}=fixture({root:true,presence:'absent',bridgeStyle});
  try {
    await until(()=>d.querySelector('#nav-cta-text').textContent==='已连接');
    click(d,'[data-mode="performance"]');
    await until(()=>d.querySelector('[data-mode="performance"]').dataset.state==='active');
    assert.equal(w.fixture.messages.filter(message=>message==='performance').length,1,'mutation must execute once');
    console.log('PASS root host signature',bridgeStyle,'and one-shot mode mutation');
  } finally {cleanup();}
}
{
  const {w,d,cleanup}=fixture({root:true,presence:'available',linked:true,emptyAsync:true});
  try {
    await until(()=>d.querySelector('#nav-cta-text').textContent==='已连接');
    click(d,'[data-mode="fast"]');
    await until(()=>d.querySelector('[data-mode="fast"]').dataset.state==='active');
    assert.equal(w.fixture.messages.filter(message=>message==='fast').length,1);
    assert(d.querySelector('#stat-scene-note').textContent.includes('原调度与初始化脚本已备份'));
    console.log('PASS native async placeholder and linked Scene mutation');
  } finally {cleanup();}
}
{
  const secret='b'.repeat(64),error='novasched: 服务启动失败：配置不可读取 '+secret;
  const {d,cleanup}=fixture({authError:error});
  try {
    await until(()=>d.querySelector('#nav-cta-text').textContent==='离线');
    assert(d.querySelector('#hero-sub').textContent.includes('配置不可读取'));
    assert(!d.querySelector('#hero-sub').textContent.includes(secret));
    click(d,'.hero-card');assert(d.querySelector('#service-start'));
    console.log('PASS startup errors visible and credentials redacted');
  } finally {cleanup();}
}
{
  const {d,cleanup}=fixture({bridgeDelay:30,root:true});
  try {
    await until(()=>d.querySelector('#nav-cta-text').textContent==='已连接',6500);
    console.log('PASS delayed host bridge reconnects automatically');
  } finally {cleanup();}
}
{
  const {w,d,cleanup}=fixture({version:'2.2.8',legacy:true,legacyMode:'balance'});
  try {
    await until(()=>d.querySelector('#about-engine-version').textContent==='v2.2.8');
    assert.equal(d.querySelector('#about-version').textContent,'v2.11.2');
    assert.equal(d.querySelector('#version-note').hidden,false);
    await until(()=>d.querySelector('#fas-legacy-desc').textContent.includes('旧版服务'));
    assert(d.querySelector('#fas-legacy-desc').textContent.includes('fas-rs'));
    console.log('PASS separate UI/service version and read-only legacy FAS probe');
  } finally {cleanup();}
}
{
  const {w,d,cleanup}=fixture({fasEngine:'active',fasEngineSrc:'timestats',fasFps:59.4,fasBoost:24,fasOverride:true});
  try {
    await until(()=>d.querySelector('#fas-engine-title').textContent==='NovaFAS 已接管');
    assert(!d.querySelector('#fas-engine-meter').hidden);
    assert.equal(d.querySelector('#fas-engine-fps').textContent,'59.4 fps');
    assert.equal(d.querySelector('#fas-engine-boost').textContent,'24%');
    assert.equal(d.querySelector('#fas-engine-badge').textContent,'已接管 fas-rs');
    assert(d.querySelector('#fas-engine-desc').textContent.includes('fas-rs 已被暂停'));
    assert.equal(d.querySelector('#fas-engine-bar').style.width,'24%');
    w.fixture.fasEngine='idle';w.fixture.fasOverride=false;w.fixturePush();
    await until(()=>d.querySelector('#fas-engine-title').textContent==='NovaFAS 待命');
    assert(d.querySelector('#fas-engine-meter').hidden);
    console.log('PASS NovaFAS engine status and override reporting');
  } finally {cleanup();}
}
{
  const {w,d,cleanup}=fixture({fasEngine:'active',fasEngineSrc:'heuristic',fasGpu:41,fasTick:120,fasFps:0,fasBoost:24});
  try {
    await until(()=>d.querySelector('#fas-engine-title').textContent==='NovaFAS 已接管');
    assert(d.querySelector('#fas-engine-source').textContent.includes('GPU 41%'),'heuristic shows gpu reading');
    assert(d.querySelector('#fas-engine-tick').textContent.includes('120'),'engine tick visible');
    console.log('PASS engine telemetry shows GPU load and heartbeat tick');
  } finally {cleanup();}
}
{
  const {w,d,cleanup}=fixture({freqs:'1459200;1612800;1497600;1824000'});
  try {
    await until(()=>d.querySelector('#live-freqs').textContent.includes('1.46'));
    assert(d.querySelector('#live-freqs').textContent.includes('1.82'),'prime cluster frequency rendered');
    assert(d.querySelector('#live-freqs').textContent.includes('GHz'),'unit rendered');
    // 触感：fixture 注入 navigator.vibrate → vibrate 通道；开关可切换并持久化
    assert.equal(w.NovaHaptics.channel(),'vibrate');
    const sw=d.querySelector('#haptics-switch');
    assert(sw,'haptics switch present');
    assert.equal(sw.getAttribute('aria-checked'),'true','haptics on by default');
    click(d,'#haptics-switch');
    assert.equal(w.NovaHaptics.isEnabled(),false,'toggle disables haptics');
    assert.equal(w.localStorage.getItem('novasched.haptics.v1'),'0','preference persisted');
    assert(d.querySelector('#haptics-note').textContent.includes('已关闭'),'note reflects off');
    console.log('PASS live cluster frequencies and haptics settings switch');
  } finally {cleanup();}
}
{
  const {w,d,cleanup}=fixture({});
  try {
    await until(()=>d.querySelector('#nav-cta-text').textContent==='已连接');
    // 原生触感桥（宿主注入 NovaHostHaptics）优先于 vibrate；切页触发一次 select
    const calls=[];
    w.NovaHostHaptics=semantic=>{calls.push(semantic);};
    assert.equal(w.NovaHaptics.channel(),'native','native bridge wins when present');
    click(d,'.dock button[data-page="logs"]');
    await until(()=>d.querySelector('#page-logs').classList.contains('active'));
    assert(calls.includes('select'),'nav switch fired one select haptic');
    // 点击当前页：只有轻反馈(tick)，不重复 select
    calls.length=0;
    click(d,'.dock button[data-page="logs"]');
    await pause();
    assert(!calls.includes('select'),'re-tap current page must not re-select');
    assert(calls.includes('tick'),'re-tap gives one light tick');
    // 关闭触感后不再触发
    w.NovaHaptics.setEnabled(false);
    calls.length=0;
    click(d,'.dock button[data-page="overview"]');
    await until(()=>d.querySelector('#page-overview').classList.contains('active'));
    assert(calls.length===0,'disabled haptics never fire');
    console.log('PASS haptics channel priority, nav semantics and kill switch');
  } finally {cleanup();}
}
{
  const {w,d,cleanup}=fixture({});
  try {
    await until(()=>d.querySelector('#nav-cta-text').textContent==='已连接');
    // 云端更新入口存在；无执行权限时按钮禁用并如实展示
    click(d,'#about-update');
    await until(()=>d.querySelector('#dialog-layer').classList.contains('open'));
    assert(d.querySelector('#dialog-title').textContent.includes('检查更新'),'update dialog opened');
    assert(d.querySelector('#update-status').textContent.includes('当前版本 v2.11.2'),'update dialog shows current version');
    console.log('PASS cloud update entry and dialog');
  } finally {cleanup();}
}
{
  const {w,d,cleanup}=fixture({fasState:'absent',fasSync:'unavailable',fasEngine:'off'});
  try {
    await until(()=>d.querySelector('#fas-engine-title').textContent==='NovaFAS 未启用');
    assert.equal(d.querySelector('#fas-engine-badge').textContent,'未启用');
    console.log('PASS engine-off keeps fas-rs interface read-only');
  } finally {cleanup();}
}
{
  const {w,d,cleanup}=fixture({});
  try {
    await until(()=>d.querySelector('#nav-cta-text').textContent==='已连接');
    w.fixture.pendingControls=1;w.fixture.phase='degraded';
    click(d,'[data-mode="fast"]');await pause();
    assert.equal(d.querySelector('[data-mode="fast"]').dataset.state,'pending');
    w.fixture.pendingControls=0;w.fixture.phase='ready';w.fixturePush();
    await until(()=>d.querySelector('[data-mode="fast"]').dataset.state==='active');
    console.log('PASS no success confirmation while CPU ceilings await readback');
  } finally {cleanup();}
}
{
 const {w,d,animations,cleanup}=fixture({motion:true});
 try{
  await until(()=>d.querySelector('#nav-cta-text').textContent==='已连接');
  for(const page of ['rules','logs','overview','about','overview','about'])click(d,`.dock [data-page="${page}"]`);
  await until(()=>d.querySelectorAll('.page.leaving').length===0);
  assert.equal(d.querySelectorAll('.page.active').length,1);
  for(const page of d.querySelectorAll('.page'))assert.equal(page.inert,!page.classList.contains('active'));
  assert.equal(animations.size,0);
  click(d,'#appearance-settings');click(d,'#personalize-close');click(d,'#appearance-settings');
  await new Promise(resolve=>nativeTimeout(resolve,600));
  assert(d.querySelector('#appearance-layer').classList.contains('open'));assert(!d.querySelector('#appearance-layer').classList.contains('closing'));assert.equal(animations.size,0);
  click(d,'#about-license');click(d,'#dialog-close');await until(()=>!d.querySelector('#dialog-layer').classList.contains('open'));
  assert.equal(d.querySelector('main').inert,true);assert.equal(d.body.style.overflow,'hidden');
  click(d,'#personalize-close');w.dispatchEvent(new w.Event('pagehide'));
  assert.equal(d.querySelector('main').inert,false);assert.equal(d.body.style.overflow,'');
  const dock=d.querySelector('.dock'),tab=d.querySelector('.dock [data-page="about"]');
  pointer(w,tab,'pointerdown',60,20);await pause();assert(dock.classList.contains('dock-pressing'));pointer(w,tab,'pointermove',62,60);assert(!dock.classList.contains('dock-pressing'));pointer(w,tab,'pointerup',62,60);assert(!dock.classList.contains('dock-dragging'));
  console.log('PASS motion fallback, rapid navigation, close/reopen, stacked modals and gesture cancellation');
 }finally{cleanup();}
}
{
 const {w,d,cleanup}=fixture({ackDelay:160});
 try{
  await until(()=>d.querySelector('#nav-cta-text').textContent==='已连接');
  click(d,'.dock [data-page="rules"]');await until(()=>d.querySelectorAll('.rule-row').length===1);click(d,'#add-rule');
  d.querySelector('#dialog-input').value='com.example.delayed';click(d,'#dialog-save');click(d,'#dialog-close');await pause();
  click(d,'.dock [data-page="about"]');click(d,'#about-license');await until(()=>w.fixture.rules.length===2);await new Promise(resolve=>nativeTimeout(resolve,100));
  assert(d.querySelector('#dialog-layer').classList.contains('open'));assert(d.querySelector('#dialog-title').textContent.includes('来源与许可'));
  console.log('PASS delayed rule acknowledgement preserves the newer dialog');
 }finally{cleanup();}
}
console.log('PASS: DOM and host scenarios. Geometry and Canvas are synthetic; this is not pixel verification.');
}
main().catch(error=>{console.error(error);process.exitCode=1;});
