/* WebUI 流体交互录屏：真实 Chromium(Edge) + 真实 DOM/CSS/动画。
   环境：桌面 Edge 内核，不是 Android WebView —— 交付时如实标注。
   宿主桥与 transport 与单测同一套 mock（root 通道）。 */
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
const {chromium} = require('playwright-core');

const WEB = path.join(__dirname, '..', 'module-template/webroot');
const OUT = path.join(__dirname, '..', 'dist', 'webui-evidence');
fs.mkdirSync(OUT, {recursive: true});
const PORT = 8799;

const MIME = {'.html':'text/html','.css':'text/css','.js':'text/javascript','.png':'image/png','.jpg':'image/jpeg','.svg':'image/svg+xml'};

function serve() {
  return new Promise(resolve => {
    const server = http.createServer((req, res) => {
      const url = req.url.split('?')[0];
      const file = path.join(WEB, url === '/' ? 'index.html' : url);
      if (!file.startsWith(WEB) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) {
        res.writeHead(404); res.end('nf'); return;
      }
      res.writeHead(200, {'content-type': MIME[path.extname(file)] || 'application/octet-stream'});
      res.end(fs.readFileSync(file));
    });
    server.listen(PORT, '127.0.0.1', () => resolve(server));
  });
}

const MOCK = `
window.__novaLog = [];
window.WebSocket = class { constructor(){ const self=this; self.readyState=0; setTimeout(()=>{ self.readyState=3; self.onclose && self.onclose({}); },0); } send(){ throw new Error('closed'); } close(){} };
window.ksu = {
  exec(command, settings, callback) {
    window.__novaLog.push(command);
    const reply = (errno, stdout) => { setTimeout(() => window[callback] && window[callback](errno, stdout, ''), 0); return ''; };
    if (command.includes('webui-session')) {
      return reply(0, JSON.stringify({port: 31415, token: 'a'.repeat(64), origin: location.origin, moduleDir: '/data/adb/modules/NovaSched_Zen_Edition'}));
    }
    if (command.includes('graphics-status')) {
      return reply(0, 'angle_driver_available=no\\nangle_package=com.google.android.angle\\nrules_count=1\\nrule_com.example.chat=angle\\n');
    }
    if (command.includes('haptic probe')) return reply(0, 'haptic=system\\nchannel=feedback\\nvibrators=1\\nhaptic_enabled=1\\n');
    if (command.includes('haptic ')) return reply(0, 'haptic=submitted channel=feedback\\n');
    if (command.includes('webui-rpc')) {
      const request = JSON.parse(command.match(/--request '([^']+)'/)[1]);
      if (request.message === 'fast' || request.message === 'balance' || request.message === 'powersave' || request.message === 'performance') {
        window.__mode = request.message;
      }
      const modes = () => 'mode:' + (window.__mode || 'balance') + ',effective:' + (window.__mode || 'balance') + ',package:com.omarea.vtools,controller:WebUI,sceneActive:false,sceneLinked:false,phase:ready,heartbeatMs:20,port:31415,powerSaveProfile:SM8650.json,socName:Snapdragon 8 Gen 3,socId:SM8650,configProfile:SM8650.json,automaticEnabled:false,appKind:daily,classificationSource:fixture,automaticActive:false,fasAvailable:false,sceneInstalled:true,scenePresence:available,fasState:absent,fasSync:unavailable,fasMode:balance,fasEngine:off,fasEngineSrc:none,fasReason:,fasFps:0.0,fasGpu:0,fasBoost:0,fasOverride:false,fasTick:0,freqs:,extremePower:false,extremeStatus:off,extremeError:,version:2.12.1,pendingControls:0,samsungSafe:false';
      const apps = () => JSON.stringify({type:'app-modes', rules:[{package:'com.example.chat',mode:(window.__mode||'balance')}], defaultMode:(window.__mode||'balance'), version:'2.12.1', classificationOverrides:{}});
      return reply(0, JSON.stringify({modes: modes(), apps: apps(), logs: request.logs && !request.cursor ? '2026-10-25 10:00:00 INFO -> 录屏演示日志\\n' : '', cursor: 'rec', logsReset: false}));
    }
    return reply(0, 'scene_presence=available\\n');
  }
};
`;

async function main() {
  const server = await serve();
  const browser = await chromium.launch({channel: 'msedge', headless: true});
  const context = await browser.newContext({
    viewport: {width: 393, height: 852},
    deviceScaleFactor: 2,
  });
  await context.addInitScript(MOCK);
  const page = await context.newPage();
  page.on('pageerror', e => console.log('PAGEERROR', String(e).slice(0, 200)));
  page.on('console', m => { if (m.type() === 'error') console.log('CONSOLE-ERR', m.text().slice(0, 160)); });
  await page.goto(`http://127.0.0.1:${PORT}/`);
  await page.waitForFunction(() => document.querySelector('#nav-cta-text')?.textContent === '已连接', null, {timeout: 20000})
    .catch(async () => {
      console.log('NAV:', await page.evaluate(() => document.querySelector('#nav-cta-text')?.textContent));
      console.log('BODY:', await page.evaluate(() => document.body.textContent.replace(/\s+/g, ' ').slice(0, 260)));
      await page.screenshot({path: path.join(OUT, 'debug-stuck.png')});
      throw new Error('页面未进入已连接状态');
    });

  // 1) 概览 → 打开个性化弹层（morph 从来源锚点展开）→ 截图 → 关闭半途 → 重新打开（中断续接）
  await page.click('#personalize-open');
  await page.waitForTimeout(200); // 展开中段
  await page.screenshot({path: path.join(OUT, 'panel-opening-mid.png')});
  await page.click('#personalize-close');
  await page.waitForTimeout(120); // 关闭中段
  await page.click('#personalize-open'); // 半途反向
  await page.waitForTimeout(600);
  await page.screenshot({path: path.join(OUT, 'panel-reopened.png')});
  await page.click('#personalize-close');
  await page.waitForTimeout(600);

  // 2) 规则页：规则卡片 → 编辑（从卡片展开，圆角连续）→ 截图 → 关闭回到卡片
  await page.click('.dock [data-page="rules"]');
  await page.waitForTimeout(300);
  await page.screenshot({path: path.join(OUT, 'rules-list.png')});
  await page.click('[data-edit="com.example.chat"]');
  await page.waitForTimeout(90);await page.screenshot({path: path.join(OUT, 'rule-dialog-open-f1.png')});
  await page.waitForTimeout(90);await page.screenshot({path: path.join(OUT, 'rule-dialog-opening-mid.png')});
  await page.waitForTimeout(90);await page.screenshot({path: path.join(OUT, 'rule-dialog-open-f3.png')});
  await page.waitForTimeout(400);
  await page.screenshot({path: path.join(OUT, 'rule-dialog-open.png')});
  // 半途关闭再反向打开
  await page.click('#dialog-close');
  await page.waitForTimeout(100);
  await page.click('[data-edit="com.example.chat"]');
  await page.waitForTimeout(600);
  await page.screenshot({path: path.join(OUT, 'rule-dialog-reopened.png')});
  // 快速连续开关
  for (let i = 0; i < 3; i++) {
    await page.click('#dialog-close');
    await page.waitForTimeout(60);
    await page.click('[data-edit="com.example.chat"]');
    await page.waitForTimeout(60);
  }
  await page.waitForTimeout(600);
  // 拖拽关闭（标题区下拉，中途取消回弹一次，然后甩动关闭）
  const title = await page.locator('#dialog-title').boundingBox();
  if (title) {
    const cx = title.x + title.width / 2, cy = title.y + title.height / 2;
    await page.mouse.move(cx, cy);
    await page.mouse.down();
    for (let step = 1; step <= 8; step++) { await page.mouse.move(cx, cy + step * 8); await page.waitForTimeout(16); }
    await page.mouse.up(); // dy≈64 可关闭；先做一次取消路径
    await page.waitForTimeout(120);
    await page.mouse.move(cx, cy + 30);
    await page.mouse.down();
    for (let step = 1; step <= 4; step++) { await page.mouse.move(cx, cy + 30 + step * 5); await page.waitForTimeout(16); }
    await page.mouse.up(); // 小位移 → 回弹
    await page.waitForTimeout(500);
  }
  await page.click('#dialog-close');
  await page.waitForTimeout(600);
  await page.screenshot({path: path.join(OUT, 'rules-after-close.png')});

  /* msedge 通道的 ffmpeg 落盘不可用（playwright 缓存版本错位）：
     过程证据改用连拍序列（展开/关闭中段各帧）。 */
  server.close();
  console.log('screenshots:', fs.readdirSync(OUT).join(', '));
  await browser.close();
}

main().catch(error => { console.error(error); process.exit(1); });
