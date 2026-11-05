import assert from 'node:assert/strict';
import { createDecipheriv, createHash, createHmac, randomUUID } from 'node:crypto';
import { execFileSync, spawn } from 'node:child_process';
import { once } from 'node:events';
import { open, readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { join } from 'node:path';

const root = process.env.GRENGIN_BACKEND_ROOT;
const scratch = process.env.GRENGIN_PLUGIN_TEST_ROOT;
const catalog = process.env.GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR;
const mock = process.env.MOCK_OAUTH2_BASE_URL;
const database = new URL(process.env.DATABASE_URL);
assert.equal(database.hostname, '127.0.0.1');
assert.equal(database.pathname, '/grengin_plugin_test');
assert.equal(new URL(mock).hostname, '127.0.0.1');
const sql = query => execFileSync('psql', ['-XAt', '-v', 'ON_ERROR_STOP=1', process.env.DATABASE_URL, '-c', query], { encoding:'utf8', stdio:['ignore','pipe','pipe'] }).trim();
const quote = value => `'${String(value).replaceAll("'", "''")}'`;
const admin = randomUUID(), alice = randomUUID(), bob = randomUUID();
const fixtures = JSON.parse(await readFile(join(root, 'connector-server-plugin/tests/fixtures/provider-requests.json'), 'utf8'));
const findings = new Map();
const exchanges = [], calls = [];
const gateReleases = new Set();
const logCanary = 'navikt-fixture-sensitive-value-must-not-be-logged';
let passed = 0, gate, fault;
function check(name, actual, expected = true) {
  assert.deepEqual(actual, expected, name);
  console.log(`PASS ${++passed}: ${name}`);
}
function probe(id, name, actual, expected) {
  if (actual === expected) return check(name, actual, expected);
  findings.set(id, name);
  console.log(`FINDING ${id}: ${name} (expected ${expected}, observed ${actual})`);
}
function bearer(id) {
  const body = [{alg:'HS256',typ:'JWT'}, {user_id:id,sub:`${id}@example.invalid`,name:'Navikt fixture',token_use:'access',exp:Math.floor(Date.now()/1000)+1800}]
    .map(value => Buffer.from(JSON.stringify(value)).toString('base64url')).join('.');
  return `${body}.${createHmac('sha256', process.env.JWT_SECRET).update(body).digest('base64url')}`;
}
function unseal(stored) {
  assert(stored.startsWith('enc:v1:'));
  const raw = Buffer.from(stored.slice(7),'base64');
  const decipher = createDecipheriv('aes-256-gcm',Buffer.from(process.env.APP_KEY,'base64'),raw.subarray(0,12));
  decipher.setAuthTag(raw.subarray(-16));
  return Buffer.concat([decipher.update(raw.subarray(12,-16)),decipher.final()]).toString();
}
const vendor = createServer(async (req, res) => {
  try {
    let raw = ''; for await (const chunk of req) { raw += chunk; assert(raw.length < 1024 * 1024); }
    const url = new URL(req.url, 'http://localhost');
    if (url.pathname.startsWith('/oauth/') && url.pathname.endsWith('/token')) {
      if (fault === 'outage') { req.socket.destroy(); return; }
      if (fault === 'error_body') {
        res.writeHead(400, {'Content-Type':'application/json'});
        res.end(JSON.stringify({error:'invalid_grant',error_description:logCanary})); return;
      }
      const issuer = url.pathname.split('/')[2];
      const form = req.headers['content-type']?.includes('application/json') ? new URLSearchParams(JSON.parse(raw)) : new URLSearchParams(raw);
      const response = await fetch(`${mock}/${issuer}/token`, {
        method:'POST', redirect:'manual', body:form.toString(),
        headers:{'Content-Type':'application/x-www-form-urlencoded', ...(req.headers.authorization ? {Authorization:req.headers.authorization} : {})},
        signal:AbortSignal.timeout(10000),
      });
      const body = JSON.parse(await response.text());
      exchanges.push({issuer, form, authorization:req.headers.authorization, status:response.status, body:structuredClone(body)});
      if (gate && form.get('grant_type') === gate.grant && response.ok) {
        const current = gate; gate = undefined;
        current.ready(); await current.released;
      }
      if (fault === 'omit_refresh' && form.get('grant_type') === 'refresh_token') delete body.refresh_token;
      if (fault === 'empty_access') body.access_token = '';
      if (fault === 'negative_expiry') body.expires_in = -1;
      if (issuer === 'slack' && form.get('grant_type') === 'authorization_code') {
        body.ok = true;
        body.authed_user = {...body};
        body.access_token = 'fixture-bot-must-not-be-used';
      }
      if (issuer === 'slack') body.ok = true;
      res.writeHead(response.status, {'Content-Type':'application/json'}); res.end(JSON.stringify(body));
    } else if (url.pathname.startsWith('/vendor/')) {
      calls.push({path:url.pathname, token:req.headers['x-shopify-access-token'] ? `Bearer ${req.headers['x-shopify-access-token']}` : req.headers.authorization, body:raw});
      res.writeHead(200, {'Content-Type':'application/json'});
      res.end(JSON.stringify({ok:true,errors:[],value:[],items:[],files:[],messages:[],result:'navikt-tool-ok'}));
    } else if (url.pathname.endsWith('/chat/completions')) {
      const body = JSON.parse(raw);
      const tool = body.tools?.find(item => item.function.name.startsWith('mcp__'));
      const complete = body.messages.some(message => message.role === 'tool');
      const events = tool && !complete ? [
        {choices:[{delta:{tool_calls:[{index:0,id:'navikt-call',type:'function',function:{name:tool.function.name,arguments:'{}'}}]}}]},
        {choices:[{delta:{},finish_reason:'tool_calls'}]},
      ] : [{choices:[{delta:{content:'Connector OAuth completed.'}}]}, {choices:[{delta:{},finish_reason:'stop'}],usage:{prompt_tokens:4,completion_tokens:4,total_tokens:8}}];
      res.writeHead(200, {'Content-Type':'text/event-stream'});
      for (const event of events) res.write(`data: ${JSON.stringify(event)}\n\n`);
      res.end('data: [DONE]\n\n');
    } else { res.writeHead(404); res.end(); }
  } catch { if (!res.headersSent) res.writeHead(500); res.end(); }
});
await new Promise(resolve => vendor.listen(0, '127.0.0.1', resolve));
const local = `http://127.0.0.1:${vendor.address().port}`;
const reservation = createServer();
await new Promise(resolve => reservation.listen(0, '127.0.0.1', resolve));
const api = `http://127.0.0.1:${reservation.address().port}`;
await new Promise(resolve => reservation.close(resolve));
const logPath = join(scratch, 'navikt-backend.log');
const log = await open(logPath, 'a');
const backend = spawn(process.env.GRENGIN_BACKEND_BINARY, [], {
  cwd:root, env:{PATH:process.env.PATH,HOME:process.env.HOME,DATABASE_URL:process.env.DATABASE_URL,
    APP_KEY:process.env.APP_KEY,JWT_SECRET:process.env.JWT_SECRET,HOST:'127.0.0.1',PORT:new URL(api).port,
    REDIRECT_URL:'http://localhost:5173',RAG_ENABLED:'false',FILE_STORAGE_ROOT:join(scratch,'files'),
    GRENGIN_METADATA_BASE_URL:local+'/metadata/'}, stdio:['ignore',log.fd,log.fd],
});
async function request(method, path, body, status=200, user=alice) {
  const response = await fetch(api+path, {method,redirect:'manual',headers:{'Content-Type':'application/json',...(user ? {Authorization:`Bearer ${bearer(user)}`} : {})},
    body:body === undefined ? undefined : JSON.stringify(body),signal:AbortSignal.timeout(45000)});
  const text = await response.text();
  if (status !== null) assert.equal(response.status, status, `${method} ${path.split('?')[0]} returned ${response.status}`);
  return {status:response.status,headers:response.headers,text,json:() => JSON.parse(text)};
}
const countStates = state => Number(sql(`SELECT count(*) FROM mcp_oauth_states WHERE state=${quote(state)}`));
const connection = (server, user=alice) => JSON.parse(sql(`SELECT row_to_json(c) FROM mcp_connections c WHERE "serverId"=${quote(server.id)} AND "userId"=${quote(user)}`) || 'null');
async function begin(server, user=alice, redirect='') {
  const started = (await request('POST', `/mcp/connections/${server.id}/authorize${redirect}`, {}, 200, user)).json();
  const url = new URL(started.authorization_url);
  assert.equal(url.origin, new URL(mock).origin);
  const response = await fetch(url, {redirect:'manual',signal:AbortSignal.timeout(10000)});
  if (response.status !== 302) {
    const reason = await response.text();
    throw new Error(`Navikt authorization returned ${response.status}: ${reason.slice(0,400)}`);
  }
  const callback = new URL(response.headers.get('location'));
  assert.equal(callback.origin, 'http://localhost:5173');
  assert.equal(callback.pathname, '/mcp/oauth/callback');
  assert.equal(callback.searchParams.get('state'), url.searchParams.get('state'));
  return {url, state:callback.searchParams.get('state'), code:callback.searchParams.get('code'), query:callback.search};
}
const redeem = (pending, user=alice, status=200) => request('GET','/mcp/oauth/callback'+pending.query,undefined,status,user);
async function chat(server, user=alice) {
  return request('POST','/chat/stream',{provider:'navikt-chat-fixture',model_name:'fixture-model',selected_mcp_servers:[server.id],
    messages:[{role:'user',content:'Use the available connector tool',files:[]}]},200,user);
}
async function install(id, method='client_secret_post', flow='oauth2') {
  const manifest = JSON.parse(await readFile(join(catalog,id,'versions/1.0.0/plugin.json'),'utf8'));
  const example = fixtures.find(item => item.plugin === id && Object.keys(item.args).length === 0);
  assert(example, 'provider must have a no-argument read tool in the reviewed fixture');
  manifest.tools = manifest.tools.filter(tool => tool.name === example.tool);
  manifest.base_url = `${local}/vendor/${id}`;
  const setup = JSON.parse(await readFile(join(catalog,id,'versions/1.0.0/setup.json'),'utf8'));
  // Navikt requires openid; these two fixture-only scopes are not vendor setup instructions.
  const scopes = [...new Set([...manifest.tools.flatMap(tool => tool.required_scopes ?? []), 'openid', 'offline_access'])];
  const oauth = {issuer_url:`${mock}/${id}`,auth_url:`${mock}/${id}/authorize`,token_url:`${local}/oauth/${id}/token`,
    redirect_url:'http://localhost:5173/mcp/oauth/callback',scopes,flow,token_endpoint_auth_method:method,
    ...(setup.authentication.oauth.token_request_format ? {token_request_format:setup.authentication.oauth.token_request_format} : {}),
    ...(setup.authentication.oauth.token_response ? {token_response:setup.authentication.oauth.token_response} : {}),
    ...(setup.authentication.oauth.refresh_token_response ? {refresh_token_response:setup.authentication.oauth.refresh_token_response} : {})};
  return (await request('POST','/admin/mcp-servers',{name:`Navikt ${id} ${flow}`,transport_type:'http',url:manifest.base_url,enabled:true,
    default_access:'all_users',client_id:`fixture-${id}`,...(method !== 'none' ? {client_secret:'fixture-only-secret'} : {}),
    connection_config:{connector_plugin:{manifest,base_url_override:manifest.base_url,configuration:example.configuration ?? {},allow_insecure_http:true,allow_private_network:true},oauth}},201,admin)).json();
}
function pause(grant) {
  let ready, resolveRelease;
  const pending = new Promise(resolve => { ready=resolve; });
  const released = new Promise(resolve => { resolveRelease=resolve; });
  const release = () => { gateReleases.delete(release); resolveRelease(); };
  gateReleases.add(release);
  gate = {grant,ready,released};
  return {pending,release};
}
async function waitForGate(pending, message) {
  let timer;
  try {
    await Promise.race([pending,new Promise((_,reject)=>{timer=setTimeout(()=>reject(new Error(message)),10000);})]);
  } finally { clearTimeout(timer); }
}
const patch = (server, body) => request('PUT', `/admin/mcp-servers/${server.id}`, body, 200, admin);
try {
  let deployment;
  for (let attempt=0; attempt<120; attempt++) {
    if (backend.exitCode !== null) throw new Error('Backend exited before readiness');
    try {
      const response=await fetch(api+'/',{signal:AbortSignal.timeout(1000)});
      if (response.ok) { deployment=await response.json(); break; }
    } catch {}
    await new Promise(resolve => setTimeout(resolve,250));
  }
  assert(deployment,'Backend readiness timed out');
  const sourceVersion=execFileSync('python3',['-c','import sys,tomllib; print(tomllib.load(open(sys.argv[1],"rb"))["package"]["version"])',join(root,'Cargo.toml')],{encoding:'utf8'}).trim();
  const migrationHead=(await readFile(join(root,'migration/HEAD'),'utf8')).trim();
  assert.equal(deployment.version,sourceVersion,'test binary must match the source release');
  assert.equal(deployment.migration_head,migrationHead,'test database must match the source migrations');
  assert.equal(deployment.expected_migration_head,migrationHead,'test binary must match the source migration contract');
  for (const id of [admin,alice,bob]) sql(`INSERT INTO users(id,status,email,"emailVerified",name,"mfaEnabled","createdAt","updatedAt","lastLoginAt","isIndependent") VALUES (${quote(id)},'active',${quote(`${id}@example.invalid`)},true,'Navikt fixture',false,now(),now(),now(),true)`);
  for (const [id,role] of [[admin,'Super Admin'],[alice,'User'],[bob,'User']]) sql(`INSERT INTO user_role_assignments(id,"userId","roleId","assignedBy","createdAt","updatedAt") SELECT gen_random_uuid(),${quote(id)},id,${quote(admin)},now(),now() FROM roles WHERE name=${quote(role)}`);
  const provider = JSON.parse(await readFile(join(root,'llm-plugin/examples/openai-compatible.provider.json'),'utf8'));
  provider.id='navikt-chat-fixture'; provider.name='Navikt fixture'; delete provider.operations.listModels;
  provider.models=[{id:'fixture-model',name:'Fixture model',modelType:'text_generator',capabilities:{chat:provider.capabilities.chat},metadata:{inputTokenRate:0,outputTokenRate:0}}];
  await request('POST','/admin/ai-engines',{api_key:'fixture-only',is_enabled:true,whitelisted_models:['fixture-model'],default_model:'fixture-model',
    plugin_config:{manifest:provider,configuration:{},baseUrlOverride:local+'/',allowInsecureHttp:true,allowPrivateNetwork:true}},201,admin);

  const ids = ['gmail', ...JSON.parse(await readFile(join(catalog,'../connector-plugins.json'),'utf8')).map(item=>item.key).filter(id=>id!=='gmail' && !['mailchimp-transactional','paypal','wix'].includes(id))];
  const methods = ['client_secret_post','none','client_secret_basic'];
  const installed = [];
  for (const [index,id] of ids.entries()) {
    const method = methods[index % methods.length];
    const server = await install(id,method); installed.push(server);
    await request('POST',`/mcp/connections/${server.id}/authorize`,{},401,null);
    const pending = await begin(server);
    check(`${id}: authorization uses PKCE S256 and a fresh user-bound state`, pending.url.searchParams.get('code_challenge_method'), 'S256');
    const verifier = sql(`SELECT "pkceVerifier" FROM mcp_oauth_states WHERE state=${quote(pending.state)}`);
    check(`${id}: stored verifier matches the transmitted challenge`,createHash('sha256').update(verifier).digest('base64url'),pending.url.searchParams.get('code_challenge'));
    await redeem(pending,bob,404); await redeem(pending,null,401);
    check(`${id}: another user or anonymous callback cannot consume the owner's state`,countStates(pending.state),1);
    await redeem(pending);
    const exchange = exchanges.at(-1);
    check(`${id}: Navikt accepted the real code exchange`,exchange.status,200);
    check(`${id}: code exchange pins the exact callback URI`,exchange.form.get('redirect_uri'),'http://localhost:5173/mcp/oauth/callback');
    check(`${id}: token authentication uses ${method}`,method === 'client_secret_basic' ? exchange.authorization?.startsWith('Basic ') : exchange.form.has('client_secret'),method !== 'none');
    const saved = connection(server);
    check(`${id}: connection tokens are encrypted at rest`,saved.connected && saved.accessToken.startsWith('enc:v1:') && !saved.accessToken.includes(exchange.body.access_token));
    const list = (await request('GET','/mcp/connections')).json().connections;
    check(`${id}: user API reports the connector as connected`,list.some(value => value.server_id === server.id && value.connected));
    const before = exchanges.length; await redeem(pending,alice,404);
    check(`${id}: replay is rejected before a second exchange`,exchanges.length,before);
    await chat(server);
    check(`${id}: chat tool call uses this user's Navikt access token`,calls.at(-1)?.token,`Bearer ${exchange.body.access_token}`);
    sql(`UPDATE mcp_connections SET "expiresAt"=now()-interval '1 second' WHERE "serverId"=${quote(server.id)} AND "userId"=${quote(alice)}`);
    const refreshCount=exchanges.length;
    await chat(server);
    check(`${id}: expired token refreshes with the correct response format`,exchanges.length===refreshCount+1 && exchanges.at(-1).form.get('grant_type')==='refresh_token' && calls.at(-1)?.token===`Bearer ${exchanges.at(-1).body.access_token}`);
    const other = await begin(server,bob); await redeem(other,bob);
    const otherToken = exchanges.at(-1).body.access_token;
    assert.notEqual(otherToken,exchange.body.access_token,'mock must produce distinguishable credentials');
    await chat(server,bob);
    check(`${id}: second user's tool call never borrows the first token`,calls.at(-1)?.token,`Bearer ${otherToken}`);
  }

  for (const id of ['mailchimp-transactional','paypal','wix']) {
    const manifest=JSON.parse(await readFile(join(catalog,id,'versions/1.0.0/plugin.json'),'utf8'));
    const example=fixtures.find(item=>item.plugin===id && Object.keys(item.args).length===0);
    manifest.tools=manifest.tools.filter(tool=>tool.name===example.tool);
    manifest.base_url=`${local}/vendor/${id}`;
    const server=(await request('POST','/admin/mcp-servers',{name:`Credential ${id}`,transport_type:'http',url:manifest.base_url,enabled:true,default_access:'all_users',connection_config:{connector_plugin:{manifest,configuration:example.configuration??{},allow_insecure_http:true,allow_private_network:true}}},201,admin)).json();
    await request('POST',`/mcp/connections/${server.id}/credentials`,{credential:'fixture'},401,null);
    await request('POST',`/mcp/connections/${server.id}/credentials`,{credential:'bad key'},400,alice);
    await request('POST',`/mcp/connections/${server.id}/credentials`,{credential:'fixture',expires_at:'2000-01-01T00:00:00Z'},400,alice);
    check(`${id}: invalid/anonymous credentials are rejected`,connection(server),null);
    for(const user of [alice,bob]) {
      const token=`fixture-${id}-${user}`;
      const response=await request('POST',`/mcp/connections/${server.id}/credentials`,{credential:token,expires_at:new Date(Date.now()+3600000).toISOString()},200,user);
      check(`${id}: credential response is private and does not expose the key`,response.headers.get('cache-control').includes('no-store')&&!response.text.includes(token));
      check(`${id}: per-user credential is encrypted`,unseal(connection(server,user).accessToken),token);
      await chat(server,user);
      const call=calls.at(-1);
      check(`${id}: chat uses the caller's credential`,id==='mailchimp-transactional'?JSON.parse(call.body).key:call.token,id==='paypal'?`Bearer ${token}`:token);
      await request('POST',`/mcp/connections/${server.id}/disconnect`,{},200,user);
      check(`${id}: disconnect erases the saved credential`,connection(server,user).accessToken,null);
    }
    await patch(server,{default_access:'admin_only'});
    await request('POST',`/mcp/connections/${server.id}/credentials`,{credential:'fixture'},404,alice);
    check(`${id}: saving credentials enforces current permission`,connection(server).connected,false);
  }

  const server = installed[0];
  let pending = await begin(server);
  sql(`UPDATE mcp_oauth_states SET "expiresAt"=now()-interval '1 second' WHERE state=${quote(pending.state)}`);
  let before = exchanges.length; await redeem(pending,alice,404);
  check('expired state is removed without contacting the token endpoint',countStates(pending.state)===0 && exchanges.length===before);
  pending = await begin(server);
  sql(`UPDATE mcp_oauth_states SET "pkceVerifier"=${quote('x'.repeat(43))} WHERE state=${quote(pending.state)}`);
  await redeem(pending,alice,503);
  check('real Navikt rejects a PKCE mismatch and backend consumes the spent state',exchanges.at(-1).status===400 && countStates(pending.state)===0);
  pending = await begin(server);
  fault='outage'; await redeem(pending,alice,503); fault=undefined;
  check('token endpoint unreachable preserves the pending state',countStates(pending.state),1);
  await redeem(pending);
  check('same callback succeeds after the token endpoint recovers',countStates(pending.state),0);

  const refreshBefore = unseal(connection(server).refreshToken);
  assert(refreshBefore,'Navikt must issue refresh tokens for offline_access');
  sql(`UPDATE mcp_connections SET "expiresAt"=now()-interval '1 second' WHERE "serverId"=${quote(server.id)} AND "userId"=${quote(alice)}`);
  fault='omit_refresh'; before=exchanges.length; await chat(server); fault=undefined;
  check('expired token refreshes through Navikt using the real refresh grant',exchanges.length===before+1 && exchanges.at(-1).form.get('grant_type')==='refresh_token');
  check('omitted refresh_token retains the previous credential after randomized re-encryption',unseal(connection(server).refreshToken)===refreshBefore);
  check('tool execution uses the refreshed access token',calls.at(-1)?.token,`Bearer ${exchanges.at(-1).body.access_token}`);

  sql(`UPDATE mcp_connections SET "expiresAt"=now()-interval '1 second' WHERE "serverId"=${quote(server.id)} AND "userId"=${quote(alice)}`);
  const refreshing=pause('refresh_token');
  const previousCalls=calls.length;
  const duringRefresh=chat(server);
  await waitForGate(refreshing.pending,'refresh gate timed out');
  await request('POST',`/mcp/connections/${server.id}/disconnect`,{});
  refreshing.release(); await duringRefresh;
  check('disconnect during refresh cannot resurrect or return a credential',!connection(server).connected && connection(server).accessToken===null && calls.length===previousCalls);

  pending=await begin(server);
  before=exchanges.length;
  await patch(server,{client_id:'changed-client'});
  await redeem(pending,alice,404);
  check('changing the plugin OAuth binding invalidates pending state before exchange',exchanges.length,before);
  await patch(server,{client_id:'fixture-gmail'});

  pending=await begin(server);
  sql(`CREATE FUNCTION navikt_fail_store() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'fixture-store-failure'; END $$; CREATE TRIGGER navikt_fail_store BEFORE INSERT OR UPDATE ON mcp_connections FOR EACH ROW EXECUTE FUNCTION navikt_fail_store()`);
  await redeem(pending,alice,503);
  check('token persistence failure rolls back state consumption',countStates(pending.state),1);
  sql('DROP TRIGGER navikt_fail_store ON mcp_connections; DROP FUNCTION navikt_fail_store()');
  await redeem(pending,alice,503);
  check('retry of a code spent before failed persistence is rejected and consumes state',countStates(pending.state),0);

  pending=await begin(server);
  await patch(server,{enabled:false});
  const listing=(await request('GET',`/mcp/tools?server_id=${server.id}`)).json();
  probe('OAUTH-01','disabled connector exposes no tools',listing.total,0);
  const disabled=await redeem(pending,alice,null);
  probe('OAUTH-02','callback refuses a connector disabled after consent began',disabled.status,404);
  if (disabled.status===200) assert(connection(server).connected,'finding must persist a connected row');
  before=calls.length; await chat(server);
  check('disabled-server OAuth gap does not bypass the tool-execution guard',calls.length,before);
  await patch(server,{enabled:true,default_access:'admin_only'});
  const blocked=await request('POST',`/mcp/connections/${server.id}/authorize`,{},null);
  probe('OAUTH-03','direct OAuth start enforces current server permission',blocked.status,404);
  await patch(server,{default_access:'all_users'});
  pending=await begin(server);
  await patch(server,{default_access:'admin_only'});
  const revoked=await redeem(pending,alice,null);
  probe('OAUTH-04','callback refuses permission revoked during consent',revoked.status,404);
  if (revoked.status===200) assert(connection(server).connected,'finding must persist a connected row');
  before=calls.length; await chat(server);
  check('revoked-access OAuth gap does not bypass the tool-execution guard',calls.length,before);
  await patch(server,{default_access:'all_users'});

  await request('POST',`/mcp/connections/${server.id}/disconnect`,{});
  pending=await begin(server);
  const waiting=pause('authorization_code');
  const first=redeem(pending,alice,null);
  await waitForGate(waiting.pending,'token exchange gate timed out');
  const second=await redeem(pending,alice,null);
  waiting.release(); const original=await first;
  probe('OAUTH-05','duplicate concurrent callbacks cannot invalidate the first successful redemption',Number(original.status===200)+Number(second.status===200),1);
  if (original.status!==200 && second.status!==200) {
    assert(!connection(server).connected && countStates(pending.state)===0,'race must leave no usable connection or retry state');
    console.log(`RACE EVIDENCE: original callback ${original.status}; duplicate callback ${second.status}; no connected session`);
  }

  pending=await begin(server); fault='empty_access';
  const empty=await redeem(pending,alice,null); fault=undefined;
  probe('OAUTH-06','empty access_token cannot create a connected OAuth session',empty.status,503);
  if (empty.status===200) assert(connection(server).connected && unseal(connection(server).accessToken)==='');
  pending=await begin(server); fault='negative_expiry';
  const negative=await redeem(pending,alice,null); fault=undefined;
  probe('OAUTH-07','negative expires_in must not become an unbounded connected credential',negative.status,503);
  if (negative.status===200) assert(connection(server).connected && connection(server).expiresAt===null);

  pending=await begin(server); fault='error_body';
  await redeem(pending,alice,503); fault=undefined;
  probe('OAUTH-08','upstream OAuth descriptions cannot leak sensitive values to backend logs',(await readFile(logPath,'utf8')).includes(logCanary),false);

  const oidc=await install('gmail','client_secret_basic','oidc');
  pending=await begin(oidc); await redeem(pending);
  check('OIDC-shaped connector configuration exchanges a signed Navikt response',connection(oidc).connected);
  await chat(oidc); check('OIDC-shaped connector token reaches the tool call',calls.at(-1)?.token,`Bearer ${exchanges.at(-1).body.access_token}`);

  pending=await begin(server,alice,'?redirect_uri='+encodeURIComponent('https://attacker.example/collect'));
  const redirected=await redeem(pending);
  check('caller-supplied off-origin post-consent redirect is never followed',redirected.headers.get('location'),null);
  await request('POST',`/mcp/connections/${server.id}/disconnect`,{});
  const disconnected=connection(server);
  check('disconnect removes saved access and refresh tokens',!disconnected.connected && disconnected.accessToken===null && disconnected.refreshToken===null);

  console.log(`Navikt connector OAuth review: ${passed} passed; ${findings.size} findings; ${exchanges.length} real mock token requests; ${calls.length} local tool requests; no paid API calls.`);
  if (findings.size) process.exitCode=1;
} catch (error) {
  console.error(`TEST HARNESS FAILURE: ${error.message}`);
  const lines=(await readFile(logPath,'utf8')).split('\n').filter(line => !line.includes('token exchange error')).slice(-12);
  console.error(lines.join('\n'));
  process.exitCode=1;
} finally {
  for (const release of gateReleases) release();
  if (backend.exitCode===null) { backend.kill('SIGTERM'); await once(backend,'exit'); }
  vendor.closeAllConnections(); await new Promise(resolve => vendor.close(resolve)); await log.close();
}
