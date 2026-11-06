import assert from 'node:assert/strict';
import {createHmac} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {readFile} from 'node:fs/promises';
import {pathToFileURL} from 'node:url';

const {chromium}=await import(pathToFileURL(process.env.GRENGIN_PLAYWRIGHT_MODULE||
  `${process.env.HOME}/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/playwright/index.mjs`));
const pid=execFileSync('ss',['-ltnpH','sport = :8082'],{encoding:'utf8'}).match(/pid=(\d+)/)?.[1];
assert(pid,'Google local API must be running on 8082');
const env=Object.fromEntries((await readFile(`/proc/${pid}/environ`,'utf8')).split('\0').filter(Boolean)
  .map(value=>{const i=value.indexOf('=');return[value.slice(0,i),value.slice(i+1)];}));
const database=new URL(env.DATABASE_URL);assert.equal(database.pathname,'/grengin_google_local');
const pg={...process.env,PGHOST:database.hostname,PGPORT:database.port,PGUSER:decodeURIComponent(database.username),
  PGPASSWORD:decodeURIComponent(database.password),PGDATABASE:database.pathname.slice(1)};
const user=JSON.parse(execFileSync('psql',['-XAt','-c',`SELECT row_to_json(u) FROM users u JOIN user_role_assignments a ON a."userId"=u.id
  JOIN roles r ON r.id=a."roleId" WHERE u.status='active' AND r.name='Super Admin' ORDER BY u."createdAt" LIMIT 1`],
  {env:pg,encoding:'utf8',stdio:['ignore','pipe','pipe']}).trim());
const data=[{alg:'HS256',typ:'JWT'},{user_id:user.id,sub:user.email,name:user.name,token_use:'access',exp:Math.floor(Date.now()/1000)+3600}]
  .map(value=>Buffer.from(JSON.stringify(value)).toString('base64url')).join('.');
const token=`${data}.${createHmac('sha256',env.JWT_SECRET).update(data).digest('base64url')}`;
const origin='http://localhost:5173';
async function api(path,body){
  const response=await fetch(origin+'/api'+path,{method:body?'POST':'GET',headers:{Authorization:`Bearer ${token}`,'Content-Type':'application/json'},
    ...(body?{body:JSON.stringify(body)}:{}),signal:AbortSignal.timeout(20000)});
  assert(response.ok,`API ${path}: unexpected HTTP ${response.status}`);return response.json();
}
const browser=await chromium.launch({headless:true});
const context=await browser.newContext({viewport:{width:1366,height:1000}});
await context.addInitScript(({token,user})=>{
  localStorage.setItem('grengin_access_token',token);localStorage.setItem('grengin_user',JSON.stringify(user));
},{token,user});
const active=new Set();
try{
  const page=await context.newPage();let intercepted='';
  await context.route('https://accounts.google.com/**',async route=>{
    intercepted=route.request().url();
    await route.fulfill({status:200,contentType:'text/plain',body:'Authorization navigation verified by local test. Actual consent is not performed.'});
  });
  await page.goto(origin+'/settings?tab=integrations');
  for(const [key,name] of [['gmail','Gmail'],['google-calendar','Google Calendar'],['google-drive','Google Drive']]){
    const section=page.getByRole('region',{name,exact:true});
    const button=section.getByRole('button',{name:`Connect ${name}`,exact:true});
    await button.waitFor({state:'visible',timeout:30000});assert(await button.isEnabled(),`${name} connect button must be enabled`);
    intercepted='';
    const start=page.waitForResponse(response=>response.url().endsWith(`/api/connectors/${key}/connect`));
    const popup=context.waitForEvent('page');
    await button.click();
    const result=await start;assert.equal(result.status(),200);
    const connection=await result.json();active.add(connection.id);
    const payload=result.request().postDataJSON();assert.match(payload.server_id,/^[a-f0-9-]{36}$/);
    assert.equal(payload.version,'1.0.0');
    const approval=await popup;
    await approval.waitForURL('https://accounts.google.com/**',{timeout:15000});
    assert(intercepted,'Google navigation should reach the browser');
    const url=new URL(intercepted);
    assert.equal(url.searchParams.get('redirect_uri'),origin+'/__local/google-broker/callback');
    assert.equal(url.searchParams.get('client_id'),env.GOOGLE_CLIENT_ID);
    assert.equal(url.searchParams.get('access_type'),'offline');assert.equal(url.searchParams.get('prompt'),'consent');
    assert.equal(url.searchParams.get('code_challenge_method'),'S256');assert.match(url.searchParams.get('state'),/^c2_[a-f0-9]{64}$/);
    assert(!url.href.includes(env.GOOGLE_CLIENT_SECRET));
    await approval.close();
    const disconnected=await api(`/connectors/${connection.id}/disconnect`,{});assert.equal(disconnected.status,'revoked');active.delete(connection.id);
    console.log(`PASS: ${name} enabled in real UI; actual API, MCP UUID and local broker start; browser navigates to the expected Google consent URL`);
  }
  await page.reload();
  await page.getByRole('region',{name:'Google Drive',exact:true}).getByRole('button',{name:/Connect|Reconnect/}).waitFor({state:'visible'});
  await page.screenshot({path:process.env.HOME+'/.local/state/grengin/google-local/desktop.png',fullPage:true});
  await page.setViewportSize({width:390,height:844});
  await page.reload();
  await page.getByRole('region',{name:'Google Drive',exact:true}).getByRole('button',{name:/Connect|Reconnect/}).waitFor({state:'visible'});
  await page.screenshot({path:process.env.HOME+'/.local/state/grengin/google-local/mobile.png',fullPage:true});
  assert(await page.evaluate(()=>document.documentElement.scrollWidth<=window.innerWidth),'Mobile view must not overflow horizontally');
  const bad=await fetch(origin+'/__local/google-broker/callback');assert.equal(bad.status,400);
  console.log('PASS: desktop/mobile screenshots, mobile horizontal containment, and invalid callback-state refusal');
  console.log('Browser requests were intercepted before Google; actual consent, token exchange and tools still require manual approval.');
}finally{
  for(const id of active)await api(`/connectors/${id}/disconnect`,{});
  await browser.close();
}
