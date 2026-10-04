/* Optional desktop rendering check. This uses mock state, never a phone session. */
const fs = require('node:fs');
const path = require('node:path');
const http = require('node:http');
const {chromium} = require('playwright-core');
const root = path.resolve(__dirname, '..');
const web = path.join(root, 'module-template/webroot');
const output = path.join(root, 'ui-preview');

async function main() {
  fs.mkdirSync(output, {recursive:true});
  const server = http.createServer((request, response) => {
    const pathname = new URL(request.url, 'http://localhost').pathname;
    const filename = path.resolve(web, '.' + (pathname === '/' ? '/index.html' : pathname));
    if (!filename.startsWith(web + path.sep)) { response.writeHead(403).end(); return; }
    const type = {'.html':'text/html','.js':'text/javascript','.css':'text/css','.svg':'image/svg+xml'}[path.extname(filename)];
    try { response.setHeader('Content-Type',type || 'application/octet-stream'); response.end(fs.readFileSync(filename)); }
    catch { response.writeHead(404).end(); }
  });
  await new Promise(resolve => server.listen(0,'127.0.0.1',resolve));
  let browser;
  try {
    browser = await chromium.launch({executablePath:process.env.CHROME,headless:true,args:['--no-sandbox','--disable-dev-shm-usage']});
    for (const width of [320,393,768]) {
      const page = await browser.newPage({viewport:{width,height:852},deviceScaleFactor:1});
      await page.goto(`http://127.0.0.1:${server.address().port}/`);
      for (const theme of ['light','dark']) {
        await page.evaluate(theme => document.documentElement.dataset.theme=theme,theme);
        for (const section of ['home','apps','logs','settings']) {
          await page.locator(`.nav-item[data-page=${section}]`).click();
          await page.screenshot({path:path.join(output,`${width}-${theme}-${section}.png`),fullPage:true});
          const overflow = await page.evaluate(() => document.documentElement.scrollWidth > innerWidth);
          if (overflow) throw new Error(`Horizontal overflow: ${width} ${theme} ${section}`);
        }
      }
      await page.close();
    }
    console.log('PASS: desktop offline rendering at 320/393/768px; no horizontal overflow');
  } finally {
    await browser?.close();
    await new Promise(resolve => server.close(resolve));
  }
}
main().catch(error => { console.error('BLOCKED:',error.message); process.exitCode=1; });
