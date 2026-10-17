/* Real Chromium checks; geometry and screenshots require a working browser. */
'use strict';
const fs=require('node:fs'),path=require('node:path'),http=require('node:http'),assert=require('node:assert/strict');
const {chromium}=require('playwright-core');
const root=path.resolve(__dirname,'..'),web=path.join(root,'module-template/webroot'),output=path.join(root,'ui-preview');
const mockHost=require('./webui-fixture.cjs');
async function main(){
 fs.mkdirSync(output,{recursive:true});
 const server=http.createServer((req,res)=>{
  const pathname=new URL(req.url,'http://localhost').pathname,file=path.resolve(web,'.'+(pathname==='/'?'/index.html':pathname));
  if(!file.startsWith(web+path.sep))return res.writeHead(403).end();
  const mime={'.html':'text/html','.css':'text/css','.js':'application/javascript','.jpg':'image/jpeg','.json':'application/json'}[path.extname(file)]||'application/octet-stream';
  try{res.setHeader('Content-Type',mime);res.end(fs.readFileSync(file));}catch{res.writeHead(404).end();}
 });
 await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));let browser;
 try{
  browser=await chromium.launch({executablePath:process.env.CHROME,headless:true,args:['--no-sandbox','--disable-dev-shm-usage','--use-angle=swiftshader','--enable-unsafe-swiftshader']});
  for(const [width,height] of [[320,740],[393,852],[852,393],[768,1024]]){
   const page=await browser.newPage({viewport:{width,height}}),errors=[];page.on('pageerror',e=>errors.push(e.message));
   await page.addInitScript(mockHost,{linked:true,presence:'available',automatic:true});await page.goto(`http://127.0.0.1:${server.address().port}/`);
   await page.waitForFunction(()=>document.querySelector('#nav-cta-text').textContent==='已连接');
   for(const target of ['overview','rules','logs','about']){
    await page.locator(`.dock [data-page="${target}"]`).click();await page.waitForTimeout(500);
    const g=await page.evaluate(()=>{const d=document.querySelector('.dock').getBoundingClientRect(),m=document.querySelector('#main');return{tabs:document.querySelectorAll('.dock [data-page]').length,active:document.querySelectorAll('.page.active').length,leaving:document.querySelectorAll('.page.leaving').length,left:d.left,right:d.right,bottom:d.bottom,overflow:m.scrollWidth>m.clientWidth+1,docScroll:scrollY};});
    assert.equal(g.tabs,4);assert.equal(g.active,1);assert.equal(g.leaving,0);assert(g.left>=0&&g.right<=width&&g.bottom<=height);assert.equal(g.overflow,false);assert.equal(g.docScroll,0);
    await page.screenshot({path:path.join(output,`${width}x${height}-${target}.png`)});
   }
   await page.locator('#appearance-settings').click();await page.waitForTimeout(500);const panel=await page.locator('#personal-panel').boundingBox();assert(panel&&panel.x>=0&&panel.y>=0&&panel.x+panel.width<=width+1&&panel.y+panel.height<=height+1);
   await page.locator('#personalize-close').click();await page.waitForTimeout(450);await page.locator('#about-license').click();await page.waitForTimeout(500);const dialog=await page.locator('.dialog').boundingBox();assert(dialog&&dialog.y>=0&&dialog.y+dialog.height<=height+1);
   await page.keyboard.press('Escape');await page.waitForTimeout(450);assert.equal(await page.locator('main').evaluate(n=>n.inert),false);
   await page.evaluate(()=>{for(const id of ['overview','rules','logs','about','overview'])document.querySelector(`.dock [data-page="${id}"]`).click();});await page.waitForTimeout(550);assert.equal(await page.locator('.page.leaving').count(),0);assert.deepEqual(errors,[]);
   console.log(`PASS Chromium ${width}x${height}: navigation, bounds, overlays and motion cleanup`);await page.close();
  }
 }finally{await browser?.close();await new Promise(resolve=>server.close(resolve));}
}
main().catch(error=>{console.error(error.message);process.exitCode=1;});
