// Synthetic local service for UI regression tests.
function mockHost(options){
  window.fixture={mode:'balance',effective:'balance',pkg:'com.omarea.vtools',linked:!!options.linked,external:!!options.external,
    installed:options.presence==='available',presence:options.presence||'available',automatic:!!options.automatic,
    pendingControls:options.pendingControls||0,phase:options.phase||'ready',legacyMode:options.legacyMode||'balance',
    version:options.version||'2.11.0',fasState:options.fasState||'ready',fasSync:options.fasSync||'synced',fasMode:options.fasMode||'balance',
    fasEngine:options.fasEngine!==undefined?options.fasEngine:'idle',fasEngineSrc:options.fasEngineSrc||'none',
    fasFps:options.fasFps||0,fasBoost:options.fasBoost||0,fasOverride:!!options.fasOverride,samsungSafe:!!options.samsung,
    fasGpu:options.fasGpu||0,fasTick:options.fasTick||0,freqs:options.freqs||'',
    rules:[{package:'com.example.chat',mode:'balance'}],sockets:[],messages:[],commands:[]};
  const f=window.fixture;
  const labels={'com.omarea.vtools':'Scene','com.example.chat':'聊天'};
  const modes=()=>`mode:${f.mode},effective:${f.external?'':f.effective},package:${f.pkg},controller:${f.external?'Scene（其它调度器）':f.linked?'Scene（NovaSched Zen Edition）':'WebUI'},sceneActive:${f.external},sceneLinked:${f.linked},phase:${f.external?'suspended':f.phase},heartbeatMs:20,port:31415,socName:Snapdragon 8 Gen 3,socId:SM8650,configProfile:SM8650.json,automaticEnabled:${f.automatic},automaticActive:${f.automatic&&!f.linked&&!f.external},appKind:daily,classificationSource:fixture,fasAvailable:${['ready','running'].includes(f.fasState)},sceneInstalled:${f.installed},scenePresence:${f.presence}` + (options.legacy ? "" : `,fasState:${f.fasState},fasSync:${f.fasSync},fasMode:${f.fasMode},fasEngine:${f.fasEngine},fasEngineSrc:${f.fasEngineSrc},fasFps:${f.fasFps.toFixed(1)},fasGpu:${f.fasGpu.toFixed(0)},fasBoost:${f.fasBoost},fasOverride:${f.fasOverride},fasTick:${f.fasTick},freqs:${f.freqs},version:${f.version},pendingControls:${f.pendingControls},samsungSafe:${f.samsungSafe}`);
  const apps=()=>JSON.stringify({type:'app-modes',rules:f.rules,defaultMode:f.mode,version:f.version,classificationOverrides:{}});
  const logs='2026-10-13 03:00:00 INFO -> Scene 回调已注册\n2026-10-13 03:00:01 WARN -> 等待节点读回\n';
  function update(message){
    f.messages.push(message);
    if(['powersave','balance','performance','fast'].includes(message)){f.mode=f.effective=f.fasMode=message;f.automatic=false;}
    else if(message==='auto\ton'){f.automatic=true;f.effective=f.fasMode='powersave';}
    else if(message==='auto\toff'){f.automatic=false;f.effective=f.mode;}
    else if(message.startsWith('set\t')){const[,pkg,mode]=message.split('\t');f.rules=f.rules.filter(r=>r.package!==pkg);f.rules.push({package:pkg,mode});}
    else if(message.startsWith('delete\t'))f.rules=f.rules.filter(r=>r.package!==message.split('\t')[1]);
    for(const socket of f.sockets)socket.push();
  }
  window.ksu={exec(command,settings,callback){
    f.commands.push(command);
    if(options.authError && command.includes('webui-session')) {
      setTimeout(()=>window[callback]?.(1,'',options.authError),0);
      return options.emptyAsync ? '' : undefined;
    }
    let stdout;
    if(command.includes('webui-session'))stdout=JSON.stringify({port:31415,token:'a'.repeat(64),origin:location.origin,moduleDir:'/data/adb/modules/NovaSched_Zen_Edition'});
    else if(command.includes('webui-rpc')){
      const raw=command.match(/--request '([^']+)'/)[1],request=JSON.parse(raw);
      if(request.message)update(request.message);
      stdout=JSON.stringify({modes:modes(),apps:apps(),logs:request.logs&&!request.cursor?logs:'',cursor:'test',logsReset:false});
    }else if(command.includes('__NOVA_FAS_ABSENT__'))stdout=f.legacyMode;
    else stdout='scene_presence=available\nscene_detection=fixture PackageManager\nscene_linked=true\n';
    setTimeout(()=>window[callback]?.(0,stdout,''),0);
    return options.emptyAsync ? '' : undefined;
  }};
  if(options.bridgeStyle) {
    const nativeExec=window.ksu.exec;
    let counter=0;
    function reply(command, callback) {
      nativeExec(command, '{}', callback);
    }
    if(options.bridgeStyle==='native0')window.ksu.exec=(...args)=>reply(args[0],args[2]);
    if(options.bridgeStyle==='native2')window.ksu.exec=(...args)=>{
      if(args.length!==2)throw new Error("Java bridge method can't be invoked with these arguments");
      reply(args[0],args[1]);
    };
    if(options.bridgeStyle==='callback2')window.ksu.exec=(command, callback)=>reply(command, callback);
    if(options.bridgeStyle==='promise1')window.ksu.exec=command=>new Promise(resolve=>{
      const name='fixture_reply_'+(++counter);
      window[name]=(errno, stdout, stderr)=>{delete window[name];resolve({errno,stdout,stderr});};
      reply(command, name);
    });
    if(options.bridgeStyle==='sync1')window.ksu.exec=command=>{
      f.commands.push(command);
      if(command.includes('webui-session'))return {errno:0,stdout:JSON.stringify({port:31415,token:'a'.repeat(64),origin:location.origin,moduleDir:'/data/adb/modules/NovaSched_Zen_Edition'})};
      const request=JSON.parse(command.match(/--request '([^']+)'/)[1]);
      if(request.message)update(request.message);
      return JSON.stringify({errno:0,stdout:JSON.stringify({modes:modes(),apps:apps(),logs:'',cursor:'test'})});
    };
  }
  if(options.insets==='modern')window.ksu.enableEdgeToEdge=enabled=>{f.edgeToEdge=enabled;};
  if(options.insets==='legacy')window.ksu.enableInsets=enabled=>{f.edgeToEdge=enabled;};
  if(options.labels!==false)window.ksu.getPackagesInfo=packages=>JSON.stringify(JSON.parse(packages).map(packageName=>({packageName,appLabel:labels[packageName]||packageName})));
  if(options.bridgeDelay) {
    const host=window.ksu; delete window.ksu;
    setTimeout(()=>{window.ksu=host;},options.bridgeDelay);
  }
  class MockSocket{
    static OPEN=1;static CONNECTING=0;
    constructor(url,protocols){
      if(options.root)throw new Error('WebSocket unavailable');
      this.url=url;this.protocols=protocols;this.readyState=0;f.sockets.push(this);
      setTimeout(()=>{if(this.readyState===3)return;this.readyState=1;this.onopen?.({});this.push();},15);
    }
    push(){if(this.readyState!==1)return;const data=this.url.endsWith('/modes')?modes():this.url.endsWith('/app-modes')?apps():logs;setTimeout(()=>this.onmessage?.({data}),5);}
    send(message){if(options.ackDelay)setTimeout(()=>update(message),options.ackDelay);else update(message);}
    close(){this.readyState=3;this.onclose?.({});}
  }
  window.WebSocket=MockSocket;
  window.fixturePush=()=>{for(const socket of f.sockets)socket.push();};
}
module.exports = mockHost;
