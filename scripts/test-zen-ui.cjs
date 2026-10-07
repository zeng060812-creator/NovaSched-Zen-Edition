/* DOM/protocol regression tests, not a substitute for an Android WebView test.
   npm install --ignore-scripts && npm run test:webui */
// Harness instances must not interleave: linkedom windows share custom
// properties (window.ksu, dynamic callbacks) across parseHTML calls, so a
// harness only owns the bridge until the next harness() call. Run each
// harness to completion before constructing the next one.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const {parseHTML} = require('linkedom');
const cssTree = require('css-tree');
const root = path.resolve(__dirname,'..');
const web = path.join(root,'module-template/webroot');
const html = fs.readFileSync(path.join(web,'index.html'),'utf8');
const source = fs.readFileSync(path.join(web,'assets/zen.js'),'utf8');
let checks = 0;
function check(name, fn) { fn(); checks++; console.log('PASS ' + name); }
function harness({noMetadata=false, noExec=false, port=31415, cachedPort=port, token='a'.repeat(64), authErrno=0, authRaw=null, authStderr='', deferAuth=false, execStyle='three', bridgeName='ksu', callbackStyle='errno-first', origin='https://mui.kernelsu.org', rpc=false, rpcAutoApply=true, deferRpc=false, rpcErrno=0}={}) {
  const {window,document} = parseHTML(html);
  let now = 1000, next = 0;
  const timers = new Map(), sockets = [], storage = new Map();
  const labelCalls = [], execCalls = [];
  const authCallbacks = [];
  const rpcCallbacks = [];
  let credentials = {port,token,origin,errno:authErrno,raw:authRaw};
  const labels = {
    'org.example.photoalbum': '旅行相册',
    'com.tencent.tmgp.sgame': '王者荣耀',
    'com.video.player': '视频播放器',
    'org.example.adversarial': '<img src=x onerror=alert(1)>',
  };
  let state={mode:'balance',effective:'balance',package:'org.example.photoalbum',controller:'WebUI',sceneActive:'false',sceneLinked:'false',phase:'ready',heartbeatMs:'0',port:String(port),extremePowerSave:'false',smoothPowerSave:'false',powerSaveProfile:''};
  let app={type:'app-modes',rules:[],defaultMode:'balance',currentPackage:state.package,effectiveMode:'balance',controller:'WebUI',locked:false,sceneAvailable:false,sceneLinked:false,phase:'ready',error:'',version:'1.5.0',port,extremePowerSave:false,smoothPowerSave:false,powerSaveProfile:''};
  const setTimer = (fn,ms=0) => {timers.set(++next,{fn,at:now+ms});return next;};
  const clearTimer = id => timers.delete(id);
  const advance = ms => {
    const end = now+ms;
    for (let i=0;i<500;i++) {
      const entries = [...timers].filter(([,v])=>v.at<=end).sort((a,b)=>a[1].at-b[1].at);
      if (!entries.length) break;
      const [id,t] = entries[0]; timers.delete(id); now=t.at;t.fn();
    }
    now=end;
  };
  class FakeDate extends Date { static now(){return now;} }
  class Socket {
    static CONNECTING=0; static OPEN=1; static CLOSED=3;
    constructor(url, protocol) {this.url=url;this.requestedProtocols=Array.isArray(protocol)?[...protocol]:[protocol];this.protocol=this.requestedProtocols[0];this.readyState=0;this.messages=[];sockets.push(this);}
    open(){this.readyState=1;this.onopen?.();}
    message(data){this.onmessage?.({data});}
    send(data){this.messages.push(data);}
    close(){this.readyState=3;}
    fail(){this.onclose?.();}
  }
  const location={hash:'',href:origin+'/'};
  const history={pushState(_a,_b,hash){location.hash=hash;}};
  const media={matches:false,addEventListener(){}};
  Object.assign(window,{matchMedia:()=>media,scrollY:0,scrollTo({top}){window.scrollY=top;}});
  const dialog=document.querySelector('#sheet');
  dialog.showModal=function(){this.open=true;this.setAttribute('open','');};
  dialog.close=function(){this.open=false;this.removeAttribute('open');};
  delete window.ksu;
  window.ksu={
    listPackages(){return JSON.stringify(Object.keys(labels));},
    getPackagesInfo(raw){const pkgs=JSON.parse(raw);labelCalls.push(pkgs);return JSON.stringify(pkgs.map(pkg=>labels[pkg]?{packageName:pkg,appLabel:labels[pkg],versionName:'1.2.3',isSystem:false,uid:10001}:{packageName:pkg,error:'not found'}));},
    exec(command,_options,callback){
      execCalls.push(command);
      if(command.includes(' webui-session ')) {
        const finish=window[callback];
        const respond=()=>{
          const stdout=credentials.raw ?? JSON.stringify({port:credentials.port,token:credentials.token,origin:credentials.origin});
          if(callbackStyle==='stdout-first')finish(stdout,authStderr,credentials.errno);
          else if(callbackStyle==='stdout-code')finish(stdout,credentials.errno,authStderr);
          else if(callbackStyle==='object')finish({exitCode:credentials.errno,stdout,stderr:authStderr});
          else finish(credentials.errno,stdout,authStderr);
        };
        if(deferAuth) authCallbacks.push(respond); else respond();
      } else if(command.includes(' webui-rpc ')) {
        const finish=window[callback];
        if(!rpc) { finish(1,'','novasched: root bridge unavailable'); return; }
        const request=JSON.parse(command.match(/--request '([^']*)'/)[1]);
        if(request.message && request.endpoint==='modes') {
          if(['powersave','balance','performance','fast'].includes(request.message)) {
            state.mode=request.message;app.defaultMode=request.message;
            if(rpcAutoApply)state.effective=request.message;
          } else {
            const [name,raw]=request.message.split('\t'),value=raw==='1';
            state[name==='smooth'?'smoothPowerSave':'extremePowerSave']=String(value);
            app[name==='smooth'?'smoothPowerSave':'extremePowerSave']=value;
          }
        }
        if(request.message && request.endpoint==='apps') {
          const [action,pkg,mode]=request.message.split('\t');
          app.rules=app.rules.filter(r=>r.package!==pkg);if(action==='set')app.rules.push({package:pkg,mode});
        }
        const respond=()=>finish(rpcErrno,JSON.stringify({modes:request.endpoint==='apps'?'':Object.entries(state).map(([k,v])=>k+':'+v).join(','),
          apps:request.endpoint==='modes'?'':JSON.stringify(app),logs:request.logs?'2026-10-03 12:00:00 信息 -> root 通道日志\n':'',cursor:request.logs?'1:40':'',logsReset:request.logs&&!request.cursor}), '');
        if(deferRpc)rpcCallbacks.push(respond);else respond();
      } else window[callback](0,'read-only diagnostic data','');
    },
  };
  const nativeExec=window.ksu.exec;
  if(execStyle==='two') window.ksu.exec=function(command,callback){ nativeExec(command,'{}',callback); };
  if(execStyle==='sync') window.ksu.exec=function(command){
    execCalls.push(command); return credentials.errno ? authStderr : credentials.raw ?? JSON.stringify({port:credentials.port,token:credentials.token,origin:credentials.origin});
  };
  if(execStyle==='java-two') window.ksu.exec=function(...args){
    if(args.length!==2)throw new Error('No matching overload for argument count');
    nativeExec(args[0],'{}',args[1]);
  };
  if(execStyle==='denied') window.ksu.exec=function(){ execCalls.push('denied'); throw new Error('Root permission denied'); };
  if(execStyle==='thenable') window.ksu.exec=function(command,_options,_callback){
    execCalls.push(command); return {then(resolve){resolve({code:credentials.errno,stdout:JSON.stringify({port:credentials.port,token:credentials.token,origin:credentials.origin}),stderr:authStderr});}};
  };
  if(bridgeName!=='ksu'){window[bridgeName]=window.ksu;delete window.ksu;}
  const hostAPI=window[bridgeName];
  if(noMetadata){delete hostAPI.listPackages;delete hostAPI.getPackagesInfo;}
  if(noExec)delete hostAPI.exec;
  const context={window,document,location,history,WebSocket:Socket,HTMLImageElement:window.HTMLImageElement,Date:FakeDate,
    setTimeout:setTimer,clearTimeout:clearTimer,console,navigator:{},URL,Blob,
    localStorage:{getItem:k=>storage.get(k)||null,setItem:(k,v)=>storage.set(k,v)},};
  if(cachedPort!==31415) storage.set('novasched.zen.ui.v1',JSON.stringify({port:cachedPort}));
  vm.runInNewContext(source,context);
  const el=id=>document.querySelector(id);
  const click=(id)=>{const node=typeof id==='string'?el(id):id;assert(node,'Missing element '+id);if(node.disabled)return;node.onclick?.({target:node,preventDefault(){}});};
  const live=kind=>sockets.filter(x=>x.readyState!==3).find(x=>x.url.endsWith(kind));
  const status=(patch={})=>{state={...state,...patch};live('/modes').message(Object.entries(state).map(([k,v])=>k+':'+v).join(','));};
  const apps=(patch={})=>{app={...app,...patch};live('/app-modes').message(JSON.stringify(app));};
  const connect=()=>{const sock=live('/modes');sock.open();status();const a=live('/app-modes');a.open();apps();};
  const submit=()=>el('#rule-editor').onsubmit({preventDefault(){}});
  const setCredentials=patch=>{credentials={...credentials,...patch};};
  const resolveAuth=()=>{const respond=authCallbacks.shift();assert(respond,'No pending auth callback');respond();};
  const resolveRpc=()=>{const respond=rpcCallbacks.shift();assert(respond,'No pending root callback');respond();};
  const setRpcState=patch=>{state={...state,...patch};};
  return {window,document,el,click,live,status,apps,connect,advance,sockets,labelCalls,execCalls,dialog,storage,media,submit,setCredentials,resolveAuth,resolveRpc,setRpcState};
}
async function flush(){for(let i=0;i<12;i++)await Promise.resolve();}
async function main(){
const h=harness(); const {el,click,status,apps}=h;
check('startup remains disconnected and disables mutation',()=>{assert.equal(el('#connection-label').textContent,'连接中');assert.equal(el('#active-mode').textContent,'—');assert([...h.document.querySelectorAll('.profile')].every(x=>x.disabled));assert.equal(h.sockets.filter(x=>x.readyState!==3).length,1);});
h.connect();
check('valid fresh daemon state owns online status',()=>{assert.equal(el('#connection-label').textContent,'已连接');assert.equal(el('#active-mode').textContent,'均衡');assert.equal(el('#heartbeat').textContent,'心跳 0s');assert.equal(el('#server-address').textContent,'127.0.0.1:31415');});
check('Android metadata provides real labels outside fallback aliases',()=>{assert.equal(el('#foreground-name').textContent,'旅行相册');assert(el('#foreground-icon img').getAttribute('src').includes('org.example.photoalbum'));});
const calls=h.labelCalls.length; status(); status();
check('unchanged foreground does not requery Android at each heartbeat',()=>assert.equal(h.labelCalls.length,calls));
click('.profile[data-mode=fast]');
check('mode request stays pending and keeps actual profile highlight',()=>{assert.equal(h.live('/modes').messages.at(-1),'fast');assert.equal(el('#active-mode').textContent,'均衡');assert(el('.profile[data-mode=fast]').classList.contains('is-pending'));assert.match(el('#mode-feedback').textContent,/等待/);});
status({mode:'fast'});
check('saved default alone is not reported as actually applied',()=>{assert(el('.profile[data-mode=fast]').classList.contains('is-pending'));assert.equal(el('#active-mode').textContent,'均衡');});
status({effective:'fast'});
check('actual successful mode acknowledgement ends pending',()=>{assert.equal(el('#active-mode').textContent,'极速');assert.equal(el('.profile[data-mode=fast]').getAttribute('aria-pressed'),'true');assert.match(el('#mode-feedback').textContent,/已生效/);});
apps({rules:[{package:'org.example.photoalbum',mode:'fast'}],defaultMode:'fast'});
click('.profile[data-mode=powersave]');status({mode:'powersave'});
check('application override is explained when changing global default',()=>{assert.equal(el('#active-mode').textContent,'极速');assert.match(el('#mode-feedback').textContent,/当前应用规则优先/);});
status({mode:'powersave',effective:'powersave',powerSaveProfile:'extreme'});apps({defaultMode:'powersave',rules:[],powerSaveProfile:'extreme'});
click('.nav-item[data-page=settings]');
el('#smooth-save').checked=true;el('#smooth-save').onchange({target:el('#smooth-save')});
check('power preference is pending until daemon confirms it',()=>{assert.equal(h.live('/modes').messages.at(-1),'smooth\t1');assert.equal(el('#smooth-save').getAttribute('aria-busy'),'true');assert.match(el('#smooth-note').textContent,/提交中/);});
status({smoothPowerSave:'true'});
check('saved smooth preference is distinguished from live application',()=>assert.match(el('#smooth-note').textContent,/等待参数下发/));
status({powerSaveProfile:'smooth'});
check('smooth power saving is called active only after actual profile feedback',()=>{assert.match(el('#smooth-note').textContent,/已生效/);assert.match(el('#extreme-note').textContent,/流畅省电优先/);});
click('.nav-item[data-page=apps]');click('#current-rule');
el('#package-input').value='invalid';h.submit();
check('invalid package stays in editor with inline error',()=>{assert.equal(el('#package-input').getAttribute('aria-invalid'),'true');assert.equal(h.dialog.open,true);});
el('#package-input').value='com.video.player';click('[data-rule-mode=performance]');h.submit();
check('rule editor preserves entered value while save is pending',()=>{assert.equal(h.live('/app-modes').messages.at(-1),'set\tcom.video.player\tperformance');assert.equal(el('#package-input').value,'com.video.player');assert.equal(h.dialog.open,true);});
apps({rules:[{package:'com.video.player',mode:'performance'}],smoothPowerSave:true,powerSaveProfile:'smooth'});
check('acknowledged rule closes editor and renders Android label',()=>{assert.equal(h.dialog.open,false);assert.equal(el('#rule-count').textContent,'1');assert.match(el('#rule-list').textContent,/视频播放器/);});
el('#rule-search').value='视频';el('#rule-search').oninput();
check('rule search accepts real app names',()=>assert.match(el('#rule-list').textContent,/视频播放器/));
el('#rule-search').value='不存在';el('#rule-search').oninput();
check('search shows a useful empty state',()=>assert.match(el('#rule-list').textContent,/没有匹配/));
el('#rule-search').value='';el('#rule-search').oninput();
h.document.querySelector('#rule-list').onclick({target:el('[data-edit="com.video.player"]')});click('#rule-delete');click('#delete-confirm');
check('delete remains visible until daemon acknowledges removal',()=>{assert.equal(h.dialog.open,true);assert.equal(h.live('/app-modes').messages.at(-1),'delete\tcom.video.player');assert.equal(el('#rule-count').textContent,'1');});
apps({rules:[]});
check('delete acknowledgement returns to empty app list',()=>{assert.equal(h.dialog.open,false);assert.equal(el('#rule-count').textContent,'0');});
status({package:'org.example.adversarial'});
check('host-provided app names are rendered as text, not HTML',()=>{assert.equal(el('#foreground-name').textContent,'<img src=x onerror=alert(1)>');assert.equal(el('#foreground-name').querySelector('img'),null);});
status({package:'org.unknown.extremely_long_application_name:sandboxed_process123456789'});click('.nav-item[data-page=home]');click('#foreground-card');
check('unknown and long process identifiers stay in readable details',()=>{assert.equal(el('#foreground-name').textContent,'未知应用');assert.match(el('.package-detail').textContent,/sandboxed_process123456789/);});
click('#sheet-close');click('.nav-item[data-page=logs]');const logs=h.live('/logs');logs.open();logs.message('2026-10-01 10:00:01 信息 -> 调度正常\n2026-10-01 10:00:02 错误 -> /sys/test 写入失败\n2026-10-01 10:00:03 警告 -> 可选节点缺失\n');
check('log parser preserves timestamps and classifies severity',()=>{assert.equal(el('#log-count').textContent,'3 条记录');assert.equal(h.document.querySelectorAll('.log-entry[data-level=error]').length,1);});
click('.segmented [data-level=error]');
check('log severity filtering works without modifying stored records',()=>{assert.equal(h.document.querySelectorAll('.log-entry').length,1);assert.match(el('#log-output').textContent,/写入失败/);});
el('#log-search').value='不存在';el('#log-search').oninput();
check('log keyword and severity filters combine correctly',()=>assert.match(el('#log-output').textContent,/暂时没有匹配/));
el('#log-search').value='';el('#log-search').oninput();click('#log-follow');
check('log follow toggle is accessible and explicit',()=>assert.equal(el('#log-follow').getAttribute('aria-pressed'),'false'));
click('#log-menu');click('#clear-log-view');
check('clear affects view only and leaves connection usable',()=>{assert.equal(el('#log-count').textContent,'0 / 0 条');assert.equal(logs.readyState,1);});
click('.nav-item[data-page=settings]');
check('leaving logs unsubscribes and saves background work',()=>assert.equal(logs.readyState,3));
status({sceneActive:'true',phase:'suspended'});apps({locked:true,sceneAvailable:true});
check('external Scene owns controls and locks all scheduler mutation',()=>{assert([...h.document.querySelectorAll('.profile')].every(x=>x.disabled));assert.equal(el('#smooth-save').disabled,true);assert.equal(el('#extreme-save').disabled,true);assert.equal(el('#add-rule').disabled,true);assert.equal(el('#active-mode').textContent,'外部控制');});
status({sceneActive:'false',sceneLinked:'true',phase:'ready'});apps({locked:false,sceneAvailable:false,sceneLinked:true});
check('Scene-linked NovaSched stays editable and is distinct from external takeover',()=>{
  assert.equal(el('#source-title').textContent,'Scene 联动');
  assert.equal(el('#smooth-save').disabled,false);
  assert([...h.document.querySelectorAll('.profile')].every(x=>!x.disabled));
  assert.equal(el('#add-rule').disabled,false);
  assert.equal(el('#active-mode').textContent,'省电');
});
check('linked mode sends profile requests both ways and keeps energy preferences independent',()=>{
  const count=h.live('/modes').messages.length;click('.profile[data-mode=fast]');
  assert.equal(h.live('/modes').messages.at(-1),'fast');
  assert.equal(h.live('/modes').messages.length,count+1);
  status({mode:'fast',effective:'fast'});
  check('linked profile request confirms through the normal heartbeat',()=>{assert.equal(el('.profile[data-mode=fast]').getAttribute('aria-pressed'),'true');assert.match(el('#mode-feedback').textContent,/已生效/);});
  status({mode:'balance',effective:'balance'});apps({defaultMode:'balance'});
  el('#smooth-save').checked=false;el('#smooth-save').onchange({target:el('#smooth-save')});
  assert.equal(h.live('/modes').messages.at(-1),'smooth\t0');
  status({smoothPowerSave:'false'});
});
click('#theme-picker');click('[data-theme-choice=dark]');
check('dark theme persists in local UI preferences',()=>{assert.equal(h.document.documentElement.dataset.theme,'dark');assert.equal(JSON.parse(h.storage.get('novasched.zen.ui.v1')).theme,'dark');});
el('#glass-effects').checked=false;el('#glass-effects').onchange({target:el('#glass-effects')});
el('#reduce-motion').checked=true;el('#reduce-motion').onchange({target:el('#reduce-motion')});
check('simple surfaces and reduced motion are local UI options',()=>{assert.equal(h.document.documentElement.dataset.effects,'simple');assert.equal(h.document.documentElement.dataset.motion,'reduced');});
el('#glass-effects').checked=true;el('#glass-effects').onchange({target:el('#glass-effects')});
check('liquid glass transparency is tunable with live preview and saved on release',()=>{
  const slider=el('#glass-intensity');
  assert(slider,'missing glass intensity slider');
  assert.equal(slider.disabled,false);
  assert.equal(h.document.documentElement.style.getPropertyValue('--glass-level'),'45');
  assert.equal(el('#glass-value').textContent,'45%');
  slider.value='85';slider.oninput({target:slider});
  assert.equal(h.document.documentElement.style.getPropertyValue('--glass-level'),'85');
  assert.equal(el('#glass-value').textContent,'85%');
  assert.match(el('#glass-note').textContent,/极通透/);
  assert.equal(JSON.parse(h.storage.get('novasched.zen.ui.v1')).glassLevel,45);
  slider.onchange();
  assert.equal(JSON.parse(h.storage.get('novasched.zen.ui.v1')).glassLevel,85);
  slider.value='200';slider.oninput({target:slider});
  assert.equal(h.document.documentElement.style.getPropertyValue('--glass-level'),'100');
  assert.equal(JSON.parse(h.storage.get('novasched.zen.ui.v1')).glassLevel,85);
});
check('glass switch disables transparency tuning and simple mode hides the glint',()=>{
  el('#glass-effects').checked=false;el('#glass-effects').onchange({target:el('#glass-effects')});
  assert.equal(el('#glass-intensity').disabled,true);
  const css=fs.readFileSync(path.join(web,'assets/zen.css'),'utf8');
  assert.match(css,/html\[data-effects=simple\] \.glass-dock::after \{ display:none; \}/);
  el('#glass-effects').checked=true;el('#glass-effects').onchange({target:el('#glass-effects')});
  assert.equal(el('#glass-intensity').disabled,false);
});
click('#diagnostics');click('#read-diagnostics');
check('diagnostics executes a fixed read-only command without app interpolation',()=>assert.match(h.execCalls.at(-1), /exec.*novasched.*game-diagnose --module-dir/));
click('#sheet-close');
h.live('/modes').fail();
await flush();
check('lost main channel revokes online and actual mode claim',()=>{assert.equal(el('#connection-label').textContent,'未连接');assert.equal(el('#active-mode').textContent,'—');assert([...h.document.querySelectorAll('.profile')].every(x=>x.disabled));});
h.advance(4000);
check('reconnect uses only the root-reported daemon port',()=>assert.equal(h.sockets.filter(x=>x.readyState!==3).length,1));
const portHarness=harness({port:31421});portHarness.connect();
check('all endpoints use the selected conflict-free daemon port',()=>{assert(portHarness.live('/modes').url.includes(':31421/'));assert(portHarness.live('/app-modes').url.includes(':31421/'));});
portHarness.status({heartbeatMs:'31000'});
check('an expired daemon heartbeat cannot claim online',()=>assert.equal(portHarness.el('#connection-label').textContent,'未连接'));
const noBridge=harness({noMetadata:true});noBridge.connect();
check('missing manager metadata bridge degrades honestly',()=>assert.equal(noBridge.el('#foreground-name').textContent,'未知应用'));
noBridge.click('.nav-item[data-page=apps]');noBridge.click('#add-rule');
check('manual package entry stays available without installed-app API',()=>assert(noBridge.el('#manual-package')));
noBridge.click('#manual-package');noBridge.click('#sheet-close');noBridge.window.dispatchEvent(new noBridge.window.Event('pagehide'));
check('pagehide closes sockets and cancels retry activity',()=>{assert(noBridge.sockets.every(x=>x.readyState===3));const n=noBridge.sockets.length;noBridge.advance(60000);assert.equal(noBridge.sockets.length,n);});
noBridge.window.dispatchEvent(new noBridge.window.Event('pageshow'));
check('returning to the page restarts discovery',()=>assert(noBridge.sockets.some(x=>x.readyState===0)));
const timeoutHarness=harness();timeoutHarness.connect();
timeoutHarness.apps({rules:[{package:'com.video.player',mode:'balance'}]});
timeoutHarness.click('.nav-item[data-page=apps]');
timeoutHarness.el('#rule-list').onclick({target:timeoutHarness.el('[data-edit="com.video.player"]')});
timeoutHarness.click('#rule-delete');timeoutHarness.click('#delete-confirm');
timeoutHarness.advance(10001);
check('timed-out deletion retains the rule and makes retry available',()=>{
  assert.equal(timeoutHarness.el('#rule-count').textContent,'1');
  assert.equal(timeoutHarness.el('#delete-confirm').disabled,false);
  assert.equal(timeoutHarness.el('#rule-error').hidden,false);
});
timeoutHarness.click('#sheet-close');timeoutHarness.click('.nav-item[data-page=home]');
timeoutHarness.click('.profile[data-mode=fast]');timeoutHarness.advance(10001);
check('timed-out mode requests never replace actual mode with the requested one',()=>{
  assert.equal(timeoutHarness.el('#active-mode').textContent,'均衡');
  assert.equal(timeoutHarness.el('#mode-feedback').dataset.error,'true');
});
check('every endpoint sends public protocol plus secret outside the URL',()=>{
  for(const sock of h.sockets) {
    assert.equal(sock.requestedProtocols.length,2);
    assert.equal(sock.requestedProtocols[1],'novasched-auth.'+'a'.repeat(64));
    assert(!sock.url.includes('a'.repeat(64)));
    assert(!sock.url.includes('?'));
  }
});
check('credential getter is a fixed privileged command',()=>{
  assert.match(h.execCalls[0],/webui-session --module-dir.*--origin 'https:\/\/mui\.kernelsu\.org'/);
  assert(h.execCalls[0].includes("/data/adb/ap/modules/NovaSched_Zen_Edition"));
  assert(h.execCalls[0].includes("^id=NovaSched_Zen_Edition$"));
});
check('secret never enters persistent UI settings or visible text',()=>{
  assert(!JSON.stringify([...h.storage]).includes('a'.repeat(64)));
  assert(!h.document.body.textContent.includes('a'.repeat(64)));
});
const missingExec=harness({noExec:true});
check('missing root bridge blocks sockets and mutation',()=>{
  assert.equal(missingExec.sockets.length,0);
  assert.equal(missingExec.el('#connection-label').textContent,'未连接');
  assert([...missingExec.document.querySelectorAll('.profile')].every(x=>x.disabled));
  assert.match(missingExec.el('#mode-feedback').textContent,/root.*接口/);
});
check('malformed credential response and root command failure fail closed',()=>{
  for(const settings of [{authRaw:'not-json'},{authRaw:'null'},{token:'short'},{token:'A'.repeat(64)},{port:80},{authErrno:1,authRaw:'command exited with a nonzero status'}]) {
    const failed=harness(settings);
    assert.equal(failed.sockets.length,0);
    assert.equal(failed.el('#connection-label').textContent,'未连接');
  }
});
const delayedAuth=harness({deferAuth:true});
check('no socket opens before privileged credential read completes',()=>{
  assert.equal(delayedAuth.sockets.length,0);
  delayedAuth.resolveAuth();
  assert.equal(delayedAuth.sockets.length,1);
});
const timeoutAuth=harness({deferAuth:true});timeoutAuth.advance(30000);
check('authentication timeout cannot fall back to public protocol',()=>{
  assert.equal(timeoutAuth.sockets.length,0);
  assert.equal(timeoutAuth.el('#connection-label').textContent,'未连接');
  timeoutAuth.resolveAuth();
  assert.equal(timeoutAuth.sockets.length,0);
});
const hiddenAuth=harness({deferAuth:true});hiddenAuth.window.dispatchEvent(new hiddenAuth.window.Event('pagehide'));
check('backgrounding cancels credential callback and discards late reply',()=>{
  hiddenAuth.resolveAuth();assert.equal(hiddenAuth.sockets.length,0);
  hiddenAuth.window.dispatchEvent(new hiddenAuth.window.Event('pageshow'));
  hiddenAuth.resolveAuth();assert.equal(hiddenAuth.sockets.length,1);
});
const rotation=harness();rotation.connect();rotation.setCredentials({token:'b'.repeat(64)});
rotation.live('/modes').fail();rotation.advance(4000);
await flush();rotation.advance(4000);
check('daemon reconnect rereads rotated credentials',()=>{
  assert.equal(rotation.live('/modes').requestedProtocols[1],'novasched-auth.'+'b'.repeat(64));
  assert.equal(rotation.execCalls.filter(x=>x.includes(' webui-session ')).length,2);
});
const actualPort=harness({port:31424,cachedPort:31418});actualPort.connect();
check('cached port is ignored and token is never sprayed over alternate ports',()=>{
  assert(actualPort.sockets.every(x=>x.url.startsWith('ws://127.0.0.1:31424/')));
  assert.equal(actualPort.sockets.length,2);
});
check('legacy 2-arg, synchronous, Java overload and structured-result bridges authenticate',()=>{
  for(const execStyle of ['two','sync','java-two','thenable']){
    const host=harness({execStyle}); host.connect();
    assert.equal(host.el('#connection-label').textContent,'已连接',execStyle);
    assert.equal(host.execCalls.filter(x=>x.includes('webui-session')).length,1,execStyle);
  }
});
check('host aliases and stdout-first or structured callbacks authenticate without leaking credentials',()=>{
  for(const bridgeName of ['ksu','magisk','apatch','KSU','mmrl'])for(const callbackStyle of ['errno-first','stdout-first','stdout-code','object']){
    const host=harness({bridgeName,callbackStyle});host.connect();
    assert.equal(host.el('#connection-label').textContent,'已连接',bridgeName+'/'+callbackStyle);
    assert(!host.document.body.textContent.includes('a'.repeat(64)));
  }
});
check('stdout-first nonzero callback remains a real error instead of a successful credential',()=>{
  const host=harness({callbackStyle:'stdout-first',authErrno:1,authRaw:'permission failure'});
  assert.equal(host.sockets.length,0);assert.equal(host.el('#connection-label').textContent,'未连接');
});
check('a complete credential in stdout outranks a bridge-reported nonzero exit code',()=>{
  // Real-world trigger: WebUI X / KSU exec wrappers can report a nonzero exit
  // for a webui-session run that printed a full credential. The daemon only
  // prints it after root, identity and origin checks, and the WebSocket
  // handshake revalidates the token against the live daemon.
  const host=harness({authErrno:1});host.connect();
  assert.equal(host.el('#connection-label').textContent,'已连接');
  assert.equal(host.execCalls.filter(x=>x.includes(' webui-session ')).length,1);
});
check('nonzero exit code with credentials minted for another origin still fails closed',()=>{
  const host=harness({authErrno:1,authRaw:JSON.stringify({port:31415,token:'a'.repeat(64),origin:'https://evil.example'})});
  assert.equal(host.sockets.length,0);
  assert.equal(host.el('#connection-label').textContent,'未连接');
  assert.match(host.el('#mode-feedback').textContent,/不匹配/);
});
check('current hardware and optional capabilities come from daemon status and clear offline',()=>{
  const host=harness();host.connect();host.status({socName:'Snapdragon 8 Elite',socId:'SM8750',configProfile:'SDM8Elite.json',smoothSupported:'false',extremeSupported:'false'});
  assert.equal(host.el('#processor-tag').textContent,'SM8750');assert(host.el('#smooth-save').disabled);assert(host.el('#extreme-save').disabled);
  assert([...host.document.querySelectorAll('.profile')].every(x=>!x.disabled));
  host.click('#diagnostics');assert(host.document.querySelector('#sheet-body').textContent.includes('SDM8Elite.json'));host.click('#sheet-close');
  host.live('/modes').fail();assert.equal(host.el('#processor-tag').textContent,'自动识别');
});
check('missing identity shows backend startup reason and offers startup diagnostics',()=>{
  const failed=harness({authErrno:1,authRaw:'',authStderr:'novasched: daemon.identity missing; stage=环境校验; cgroup missing'});
  assert.equal(failed.sockets.length,0);
  assert.match(failed.el('#mode-feedback').textContent,/环境校验/);
  failed.click('#connection'); assert.match(failed.el('#connection-error').textContent,/cgroup missing/);
  failed.click('#open-diagnostics');failed.click('#read-diagnostics');
  assert.match(failed.execCalls.at(-1),/novasched.*diagnose --module-dir/);
});
check('slow initial root grant has 30 seconds and does not silently time out at 5',()=>{
  const slow=harness({deferAuth:true});slow.advance(7000);
  assert.equal(slow.el('#connection-label').textContent,'连接中');
  slow.resolveAuth();slow.connect();assert.equal(slow.el('#connection-label').textContent,'已连接');
});
check('permission failure does not retry alternative command signatures',()=>{
  const denied=harness({execStyle:'denied'});
  assert.equal(denied.execCalls.length,1);
  assert.equal(denied.sockets.length,0);
  assert.match(denied.el('#mode-feedback').textContent,/授权/);
});
check('opaque malformed and remote HTTP pages cannot request network credentials',()=>{
  for(const origin of ['null','http://evil.example','ftp://example.com']){
    const host=harness({origin});assert.equal(host.execCalls.length,0);assert.equal(host.sockets.length,0);
  }
});
check('custom HTTPS page without a root bridge cannot obtain a session',()=>{
  const host=harness({origin:'https://mmrl.custom.example',noExec:true});
  assert.equal(host.execCalls.length,0);assert.equal(host.sockets.length,0);
});
check('exact custom HTTPS domains and loopback HTTP can authenticate',()=>{
  for(const origin of ['https://mmrl.local','https://webui.custom-domain.example:9443','http://127.0.0.1:8090']) {
    const host=harness({origin});host.connect();
    assert.equal(host.el('#connection-label').textContent,'已连接',origin);
    assert(host.execCalls[0].includes(`--origin '${origin}'`));
  }
});
check('credentials minted for a different page origin fail closed',()=>{
  const origin='https://mmrl.local';
  const host=harness({origin,authRaw:JSON.stringify({port:31415,token:'a'.repeat(64),origin:'https://evil.example'})});
  assert.equal(host.sockets.length,0);assert.equal(host.el('#connection-label').textContent,'未连接');
});
check('error display strips bearer secrets and never renders HTML',()=>{
  const failed=harness({authErrno:1,authStderr:'denied token='+ 'a'.repeat(64)+' <img src=x onerror=alert(1)>'});
  assert(!failed.document.body.textContent.includes('a'.repeat(64)));
  assert.equal(failed.el('#mode-feedback').querySelector('img'),null);
});
const failedFallback=harness();failedFallback.live('/modes').fail();await flush();failedFallback.click('#connection');
check('both failed transports preserve a specific connection error',()=>{
  const failed=failedFallback;
  assert.match(failed.el('#connection-error').textContent,/备用通道失败.*unavailable/);
  assert([...failed.document.querySelectorAll('.profile')].every(x=>x.disabled));
});
check('WebUI X CSP permits daemon-selected loopback ports without remote WS access',()=>{
  const config=JSON.parse(fs.readFileSync(path.join(web,'config.mmrl.json'),'utf8'));
  assert(config.permissions.includes('kernelsu.permission.SHELL'));
  assert(!config.require);
  const connect=config.contentSecurityPolicy.split(';').find(x=>x.trim().startsWith('connect-src'));
  assert(connect.includes('ws://127.0.0.1:*'));
  assert(!connect.includes('ws://*'));
  assert(!connect.includes('0.0.0.0'));
});
const staleScene=harness();staleScene.connect();
staleScene.status({controller:'Scene（NovaSched Zen Edition）',sceneLinked:'true'});
staleScene.status({controller:'WebUI',sceneLinked:'false',sceneActive:'false'});
staleScene.apps({controller:'Scene（NovaSched Zen Edition）',sceneLinked:true,locked:true,sceneAvailable:true});
check('delayed rules frames cannot resurrect Scene ownership after unlink',()=>{
  assert.equal(staleScene.el('#source-title').textContent,'WebUI 接管');
  assert([...staleScene.document.querySelectorAll('.profile')].every(node=>!node.disabled));
});
staleScene.status({controller:'Scene（NovaSched Zen Edition）',sceneLinked:'true'});
check('a fresh Scene link resumes two-way editing immediately',()=>{
  assert.equal(staleScene.el('#source-title').textContent,'Scene 联动');
  assert([...staleScene.document.querySelectorAll('.profile')].every(node=>!node.disabled));
  assert.equal(staleScene.el('#add-rule').disabled,false);
});
const switchingController=harness();switchingController.connect();switchingController.click('.profile[data-mode=fast]');
switchingController.status({sceneLinked:'true',controller:'Scene（NovaSched Zen Edition）'});
check('a pending profile request survives Scene linking',()=>{
  assert(switchingController.el('.profile[data-mode=fast]').classList.contains('is-pending'));
});
switchingController.status({mode:'fast',effective:'fast'});
check('linked confirmation completes the pending profile request',()=>{
  assert.equal(switchingController.el('#active-mode').textContent,'极速');
  assert.match(switchingController.el('#mode-feedback').textContent,/已生效/);
});
const misreportedRpc=harness({rpc:true,rpcErrno:1});misreportedRpc.live('/modes').fail();await flush();
check('root bridge snapshots are honored despite a bridge-reported nonzero exit code',()=>{
  assert.equal(misreportedRpc.el('#connection-label').textContent,'已连接');
  assert.equal(misreportedRpc.el('#server-address').textContent,'宿主 root 通道');
  assert.equal(misreportedRpc.el('#active-mode').textContent,'均衡');
});
const blockedNetwork=harness({rpc:true});blockedNetwork.live('/modes').fail();await flush();
check('blocked WebSocket automatically connects through the authorized root bridge',()=>{
  assert.equal(blockedNetwork.el('#connection-label').textContent,'已连接');
  assert.equal(blockedNetwork.el('#server-address').textContent,'宿主 root 通道');
  assert.equal(blockedNetwork.el('#active-mode').textContent,'均衡');
  assert(blockedNetwork.execCalls.some(command=>command.includes(' webui-rpc ')));
  assert(blockedNetwork.sockets.every(socket=>socket.readyState===3));
});
blockedNetwork.click('.profile[data-mode=fast]');await flush();
check('root fallback executes a profile request once and waits for actual state',()=>{
  assert.equal(blockedNetwork.el('#active-mode').textContent,'极速');
  assert.match(blockedNetwork.el('#mode-feedback').textContent,/已生效/);
  assert.equal(blockedNetwork.execCalls.filter(command=>command.includes('"message":"fast"')).length,1);
});
blockedNetwork.advance(2000);await flush();
blockedNetwork.click('.nav-item[data-page=apps]');blockedNetwork.click('#add-rule');blockedNetwork.click('#manual-package');
blockedNetwork.el('#package-input').value='com.video.player';blockedNetwork.click('[data-rule-mode=performance]');blockedNetwork.submit();await flush();
check('root fallback shares application rules and receives save acknowledgements',()=>{
  assert.equal(blockedNetwork.el('#rule-count').textContent,'1');
  assert.equal(blockedNetwork.dialog.open,false);
  assert.equal(blockedNetwork.execCalls.filter(command=>command.includes('set\\tcom.video.player\\tperformance')).length,1);
});
blockedNetwork.click('.nav-item[data-page=logs]');blockedNetwork.advance(2000);await flush();
check('root fallback subscribes to incremental logs only on the log page',()=>{
  assert.match(blockedNetwork.el('#log-output').textContent,/root 通道日志/);
  assert(blockedNetwork.execCalls.at(-1).includes('"logs":true'));
});
blockedNetwork.click('.nav-item[data-page=home]');blockedNetwork.advance(2000);await flush();
check('root fallback stops log reads when leaving the log page',()=>assert(blockedNetwork.execCalls.at(-1).includes('"logs":false')));
blockedNetwork.window.dispatchEvent(new blockedNetwork.window.Event('pagehide'));const rootCalls=blockedNetwork.execCalls.length;
blockedNetwork.advance(60000);await flush();
check('root fallback does no polling or mutation while the page is in the background',()=>assert.equal(blockedNetwork.execCalls.length,rootCalls));
const opaqueHost=harness({origin:'file://',rpc:true});await flush();
check('root-authorized local file hosts work without allowing Origin:null sockets',()=>{
  assert.equal(opaqueHost.el('#connection-label').textContent,'已连接');
  assert.equal(opaqueHost.sockets.length,0);
  assert(opaqueHost.execCalls[0].includes("--origin 'null'"));
  assert(!opaqueHost.execCalls.some(command=>command.includes(' webui-session ')));
});
const staleRpc=harness({rpc:true,rpcAutoApply:false});staleRpc.live('/modes').fail();await flush();
staleRpc.click('.profile[data-mode=fast]');await flush();
check('saved root request is not equated with successful application',()=>{
  assert.equal(staleRpc.el('#active-mode').textContent,'均衡');
  assert(staleRpc.el('.profile[data-mode=fast]').classList.contains('is-pending'));
});
staleRpc.setRpcState({effective:'fast'});staleRpc.advance(2000);await flush();
check('next live root snapshot confirms profile application',()=>assert.equal(staleRpc.el('#active-mode').textContent,'极速'));
const hiddenRpc=harness({origin:'file://',rpc:true,deferRpc:true});
hiddenRpc.window.dispatchEvent(new hiddenRpc.window.Event('pagehide'));hiddenRpc.resolveRpc();await flush();
check('late root response cannot reconnect a hidden page',()=>assert.notEqual(hiddenRpc.el('#connection-label').textContent,'已连接'));
const expiredRpc=harness({origin:'file://',rpc:true});await flush();expiredRpc.setRpcState({heartbeatMs:'31000'});expiredRpc.advance(2000);await flush();
check('root fallback rejects an expired scheduler heartbeat',()=>{
  assert.equal(expiredRpc.el('#connection-label').textContent,'未连接');
  assert.match(expiredRpc.el('#mode-feedback').textContent,/过期/);
});
const ephemeral=harness({port:45987});ephemeral.connect();
check('system-selected ports come only from the privileged credential response',()=>assert(ephemeral.sockets.every(socket=>socket.url.startsWith('ws://127.0.0.1:45987/'))));
check('every static icon exists and assets resolve locally',()=>{
  const ids=new Set([...h.document.querySelectorAll('symbol')].map(n=>n.id));
  for(const node of h.document.querySelectorAll('use'))assert(ids.has(node.getAttribute('href').slice(1)));
  for(const node of h.document.querySelectorAll('script[src],link[rel=stylesheet]')) { const src=node.getAttribute('src')||node.getAttribute('href');if(src.startsWith('./'))assert(fs.existsSync(path.resolve(web,src))); }
});
check('CSS parses cleanly and covers safe areas, narrow screens and motion',()=>{
  const css=fs.readFileSync(path.join(web,'assets/zen.css'),'utf8');
  cssTree.parse(css,{onParseError:err=>{throw err;}});
  assert.match(css,/minmax\(0,1fr\)/);assert.match(css,/max-width:359px/);assert.match(css,/safe-area-inset-bottom/);assert.match(css,/prefers-reduced-motion/);assert.match(css,/overflow-wrap:anywhere/);
});
check('liquid glass derives blur tint and specular from the single tunable level',()=>{
  const css=fs.readFileSync(path.join(web,'assets/zen.css'),'utf8');
  assert.match(css,/--glass-level:45/);
  assert.match(css,/blur\(var\(--glass-blur\)\)/);
  assert.match(css,/saturate\(var\(--glass-sat\)\)/);
  assert.match(css,/--glass-tint:calc\(0\.54 - var\(--glass-level\)\*0\.0042\)/);
  assert.match(css,/dock-glint/);
  assert.match(css,/glass-slider::-webkit-slider-thumb/);
  assert(css.includes('animation-duration:.001ms!important'));
});
console.log(`Zen UI: ${checks} DOM/protocol/static checks passed. No browser rendering or physical Android device is implied.`);
}
let completed = false;
process.on('beforeExit',()=>{
  if (!completed) {
    console.error('FAIL WebUI test runner exited before all checks completed.');
    process.exitCode = 1;
  }
});
main().then(()=>{completed=true;},error=>{completed=true;console.error(error);process.exitCode=1;});
