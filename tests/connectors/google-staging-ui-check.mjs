import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {pathToFileURL} from 'node:url';

const {chromium}=await import(pathToFileURL(process.env.GRENGIN_PLAYWRIGHT_MODULE||
  `${process.env.HOME}/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/playwright/index.mjs`));
const origin='http://localhost:5173',broker='https://service-staging.grengin.com';
const definitions=[['gmail','Gmail','https://gmailmcp.googleapis.com/mcp/v1'],
  ['google-calendar','Google Calendar','https://calendarmcp.googleapis.com/mcp/v1'],
  ['google-drive','Google Drive','https://drivemcp.googleapis.com/mcp/v1']];
const catalogs=new Map();
for(const [key] of definitions){
  const response=await fetch(`https://meta.grengin.com/connectors/${key}/versions/1.0.0.json`,{redirect:'error',signal:AbortSignal.timeout(15000)});
  assert(response.ok);catalogs.set(key,await response.json());
}
const user={id:randomUUID(),name:'Isolated connector UI test',email:'fixture@example.invalid'};
const servers=[],connections=new Map(),errors=[];
let creates=0,completes=0,starts=0;
let permissions={'mcp_servers:admin':'*','mcp_servers:view':'*'};
let stalledStatusId=null;
const browser=await chromium.launch({headless:true});
const context=await browser.newContext({viewport:{width:1366,height:1000}});
await context.addInitScript(user=>{
  localStorage.setItem('grengin_access_token','UI_FIXTURE_NOT_A_REAL_TOKEN');localStorage.setItem('grengin_user',JSON.stringify(user));
  if(sessionStorage.getItem('connector-test-short-timeouts')==='1'){
    const timeout=AbortSignal.timeout.bind(AbortSignal);
    AbortSignal.timeout=milliseconds=>timeout(Math.min(milliseconds,150));
  }
},user);
await context.route('**/*',async route=>{
  const req=route.request(),url=new URL(req.url());
  const json=value=>route.fulfill({status:200,contentType:'application/json',body:JSON.stringify(value)});
  if(url.origin==='https://accounts.google.com')return route.fulfill({status:200,contentType:'text/plain',body:'Google consent is intercepted in this isolated UI test.'});
  if(url.origin==='https://meta.grengin.com')return route.continue();
  if(url.origin!==origin)return route.abort();
  if(!url.pathname.startsWith('/api/'))return route.continue();
  const path=url.pathname.slice(4);
  if(path==='/me/permissions')return json({permissions});
  if(path==='/me/metadata')return json({metadata:{tourGuide:{firstLogin:false}}});
  if(path==='/me/notifications/stream')return route.fulfill({status:200,contentType:'text/event-stream',body:': isolated fixture\n\n'});
  if(path==='/mcp-servers')return json({servers});
  if(path==='/mcp/connections')return json({connections:[]});
  if(path==='/connectors')return json([...connections.values()]);
  if(path==='/admin/mcp-servers'&&req.method()==='GET')return json({servers,total:servers.length});
  if(path==='/admin/mcp-servers'&&req.method()==='POST'){
    const body=req.postDataJSON();assert.equal(body.client_secret,undefined);assert.equal(body.default_access,'admin_only');
    assert(!servers.some(value=>value.connection_config.grengin_connector_key===body.connection_config.grengin_connector_key));
    const server={...body,id:randomUUID(),connected:false,tools:[]};servers.push(server);creates++;return json(server);
  }
  const start=/^\/connectors\/(gmail|google-calendar|google-drive)\/connect$/.exec(path);
  if(start){
    const key=start[1],body=req.postDataJSON(),server=servers.find(value=>value.connection_config.grengin_connector_key===key);
    assert(server);assert.deepEqual(body,{version:'1.0.0',server_id:server.id});starts++;
    const url=new URL('https://accounts.google.com/o/oauth2/v2/auth');
    for(const [k,v] of Object.entries({client_id:'UI_FIXTURE_ONLY',response_type:'code',state:'c2_'+'a'.repeat(64),
      redirect_uri:`${broker}/v1/connectors/callback/${key}`,code_challenge_method:'S256',code_challenge:'b'.repeat(43),
      access_type:'offline',prompt:'consent'}))url.searchParams.set(k,v);
    const connection={id:randomUUID(),connector_key:key,catalog_version:'1.0.0',server_id:server.id,status:'pending',
      revocation_pending:false,authorization_url:url.href};connections.set(connection.id,connection);return json(connection);
  }
  const operation=/^\/connectors\/([^/]+)(?:\/(complete|disconnect))?$/.exec(path);
  if(operation){
    const connection=connections.get(operation[1]);assert(connection);
    if(!operation[2]&&connection.id===stalledStatusId){
      await new Promise(resolve=>setTimeout(resolve,750));
      try{return await json(connection);}catch{return;}
    }
    if(operation[2]==='complete'){
      assert.equal(req.postDataJSON().confirmation_code,'c'.repeat(64));
      assert(!req.frame().url().includes('c'.repeat(64)),'Receipt must be scrubbed before completion');
      completes++;connection.status='connected';delete connection.authorization_url;
    }else if(operation[2]==='disconnect'){
      connection.status='revoked';connections.delete(connection.id);
    }
    return json(connection);
  }
  if(path.endsWith('/sync-tools'))return json({success:true,total_tools:1});
  if(path==='/me')return json(user);
  if(path==='/branding')return json({});
  return json({items:[],notifications:[],chats:[],conversations:[],projects:[],total:0,unread_count:0});
});
try{
  const page=await context.newPage();page.on('pageerror',error=>errors.push(error.message));
  await page.goto(origin+'/settings?tab=integrations');
  await page.getByRole('region',{name:'Slack',exact:true}).waitFor();
  for(const [key,name] of definitions){
    const section=page.getByRole('region',{name,exact:true});
    const button=section.getByRole('button',{name:`Connect ${name}`,exact:true});
    await button.waitFor();assert(await button.isDisabled(),'Admin setup must precede chat-linked connection');
    const setup=section.getByRole('button',{name:`Set up ${name}`,exact:true});
    await setup.waitFor({state:'visible',timeout:5000});
    await setup.click();
    const dialog=page.getByRole('dialog');await dialog.getByRole('button',{name:'Add server',exact:true}).click();
    await dialog.waitFor({state:'hidden'});assert(await button.isEnabled());
    const popupEvent=context.waitForEvent('page');await button.click();const popup=await popupEvent;
    await popup.waitForURL('https://accounts.google.com/**');
    const auth=new URL(popup.url());assert.equal(auth.searchParams.get('redirect_uri'),`${broker}/v1/connectors/callback/${key}`);
    const connection=[...connections.values()].find(value=>value.connector_key===key);assert(connection);
    const closed=popup.waitForEvent('close');
    await popup.goto(origin+'/mcp/oauth/callback#'+new URLSearchParams({connection_id:connection.id,
      state:auth.searchParams.get('state'),connector_approval:'c'.repeat(64)}));await closed;
    await page.bringToFront();await section.getByRole('button',{name:'Refresh status',exact:true}).click();
    await section.getByRole('button',{name:'Sync tools',exact:true}).waitFor();
    console.log(`PASS: ${name}: explicit setup, UUID binding, staging consent callback, owner-bound popup completion and tool-sync controls (isolated API fixtures)`);
  }
  assert.equal(creates,3);assert.equal(starts,3);assert.equal(completes,3);assert.deepEqual(errors,[]);
  await page.setViewportSize({width:390,height:844});await page.reload();
  await page.getByRole('region',{name:'Google Drive',exact:true}).waitFor();
  assert(await page.evaluate(()=>document.documentElement.scrollWidth<=innerWidth));
  assert.equal(creates,3,'Reload must never duplicate admin configuration');
  console.log('PASS: Slack remains available; reload creates no extra servers; mobile has no horizontal overflow; no browser runtime errors.');
  for(const status of ['failed','expired','revoked','pending','connected']){
    servers.length=0;connections.clear();
    const old={id:randomUUID(),connector_key:'gmail',catalog_version:'1.0.0',status,revocation_pending:false};
    connections.set(old.id,old);
    await page.reload();
    const region=page.getByRole('region',{name:'Gmail',exact:true});
    const setup=region.getByRole('button',{name:'Set up Gmail',exact:true});
    await setup.waitFor({state:'visible'});
    await setup.click({timeout:5000});
    const dialog=page.getByRole('dialog');await dialog.getByRole('button',{name:'Add server',exact:true}).click();
    await dialog.waitFor({state:'hidden'});
    const reconnect=region.getByRole('button',{name:'Reconnect Gmail',exact:true});
    await reconnect.waitFor();assert(await reconnect.isEnabled(),'Saved account state must not deadlock missing server setup');
    const popupEvent=context.waitForEvent('page');await reconnect.click();const popup=await popupEvent;
    await popup.waitForURL('https://accounts.google.com/**');await popup.close();
    assert(!connections.has(old.id),'Reconnect must retire the old connection before changing its server binding');
    const current=[...connections.values()].find(value=>value.connector_key==='gmail');
    assert.equal(current.server_id,servers[0].id);
    console.log(`PASS: existing ${status} account without a chat server can be explicitly configured and reconnected`);
  }
  const expectedCreates=creates,expectedStarts=starts;
  servers.length=0;connections.clear();
  const stalled={id:randomUUID(),connector_key:'gmail',catalog_version:'1.0.0',status:'failed',revocation_pending:false};
  connections.set(stalled.id,stalled);stalledStatusId=stalled.id;
  await page.evaluate(()=>sessionStorage.setItem('connector-test-short-timeouts','1'));
  await page.reload();
  const stalledRegion=page.getByRole('region',{name:'Gmail',exact:true});
  await stalledRegion.getByRole('alert').waitFor({state:'visible',timeout:5000});
  await stalledRegion.getByRole('button',{name:'Set up Gmail',exact:true}).click({timeout:5000});
  await page.getByRole('dialog').waitFor();
  await page.getByRole('dialog').getByRole('button',{name:'Add server',exact:true}).click({trial:true});
  await page.keyboard.press('Escape');
  await page.evaluate(()=>sessionStorage.removeItem('connector-test-short-timeouts'));
  await page.waitForTimeout(800);stalledStatusId=null;
  assert.equal(creates,expectedCreates);assert.equal(starts,expectedStarts);
  console.log('PASS: a stalled broker status request times out, displays an error, and releases setup controls');
  servers.length=0;connections.clear();
  for(const [label,grants] of [
    ['regular user',{'mcp_servers:view':'*'}],
    ['obsolete permission key',{'mcp_servers:manage':'*','mcp_servers:view':'*'}],
    ['scoped administrator',{'mcp_servers:admin':[randomUUID()],'mcp_servers:view':'*'}],
  ]){
    permissions=grants;
    const permissionResponse=page.waitForResponse(response=>response.url().endsWith('/api/me/permissions'));
    await page.reload();await permissionResponse;
    for(const [,name] of definitions){
      const region=page.getByRole('region',{name,exact:true});
      const button=region.getByRole('button',{name:`Connect ${name}`,exact:true});
      await button.waitFor();
      assert(await button.isDisabled(),'A missing administrator-configured server cannot be bypassed');
      assert.equal(await region.getByRole('button',{name:`Set up ${name}`,exact:true}).count(),0);
    }
    assert.equal(creates,expectedCreates);assert.equal(starts,expectedStarts);
    console.log(`PASS: ${label} cannot configure servers or bypass required setup`);
  }
  console.log('All API requests were intercepted. No VM writes, real Google consent or broker secret changes were made by this UI test.');
}finally{await browser.close();}
