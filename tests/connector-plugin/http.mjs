import assert from 'node:assert/strict';
import { createHash, createHmac, randomUUID } from 'node:crypto';
import { execFileSync, spawn } from 'node:child_process';
import { once } from 'node:events';
import { open, readFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { createInterface } from 'node:readline';
import { dirname, join } from 'node:path';

const root = process.env.GRENGIN_BACKEND_ROOT;
const scratch = process.env.GRENGIN_PLUGIN_TEST_ROOT;
const database = new URL(process.env.DATABASE_URL);
assert.equal(database.hostname, '127.0.0.1');
assert.equal(database.pathname, '/grengin_plugin_test');
const sql = query => execFileSync('psql', ['-XAt', '-v', 'ON_ERROR_STOP=1', process.env.DATABASE_URL, '-c', query], {encoding:'utf8', stdio:['ignore','pipe','pipe']}).trim();
const quote = value => `'${String(value).replaceAll("'", "''")}'`;
const admin = randomUUID();
const member = randomUUID();
let checks = 0;
function check(name, actual, expected) { assert.deepEqual(actual, expected, name); console.log(`PASS ${++checks}: ${name}`); }
function bearer(id) {
  const parts = [{alg:'HS256',typ:'JWT'}, {user_id:id,sub:`${id}@example.invalid`,name:'Plugin test',token_use:'access',exp:Math.floor(Date.now()/1000)+1800}].map(value=>Buffer.from(JSON.stringify(value)).toString('base64url')).join('.');
  return `${parts}.${createHmac('sha256',process.env.JWT_SECRET).update(parts).digest('base64url')}`;
}
const vendorCalls = [];
let modelGate;
const fixture = createServer(async (req,res) => {
  let raw = ''; for await (const chunk of req) raw += chunk;
  const distribution=/^\/metadata\/distributions\/grengin-api\/(\d+\.\d+\.\d+)\/index.json$/.exec(req.url);
  if (distribution) {
    const entry=(id,contract)=>({catalogVersion:'1.0.0',providers:[{id,name:id,status:'preview',defaultVersion:'1.0.0',versions:[{version:'1.0.0',schemaVersion:'1.0',...contract,sha256:'0'.repeat(64)}]}]});
    const index={formatVersion:'1.0',grenginApiVersion:distribution[1],grenginApiCommit:'a'.repeat(40),catalogCommit:'b'.repeat(40),
      authProviders:entry('fixture-auth',{configurationVersion:'1.0'}),aiProviders:entry('fixture-ai',{manifestVersion:'1.0'}),connectorPlugins:entry(catalogManifest.id,{manifestVersion:'1.0'})};
    index.connectorPlugins.providers[0].versions[0].sha256=createHash('sha256').update(catalogBytes).digest('hex');
    res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify(index));
  } else if(req.url==='/metadata/connector-plugins/fixture-connector/versions/1.0.0/plugin.json') {
    res.writeHead(200,{'Content-Type':'application/json'});res.end(catalogBytes);
  } else if (req.url === '/token') {
    const code = new URLSearchParams(raw).get('code');
    res.writeHead(200,{'Content-Type':'application/json'});
    res.end(JSON.stringify({access_token:`fixture-${code}`,refresh_token:`refresh-${code}`,expires_in:3600,token_type:'Bearer',scope:'read'}));
  } else if (req.url === '/v1/echo') {
    vendorCalls.push(req.headers.authorization);
    res.writeHead(200,{'Content-Type':'application/json'}); res.end(JSON.stringify({message:'plugin-result'}));
  } else if (req.url.includes('/chat/completions')) {
    const body = JSON.parse(raw);
    const tool = body.tools?.find(tool=>tool.function?.name.includes('plugin_echo'));
    const finished = body.messages.some(message=>message.role==='tool');
    if (tool && !finished && modelGate) {
      const gate = modelGate; modelGate = undefined;
      gate.ready(); await gate.released;
    }
    res.writeHead(200,{'Content-Type':'text/event-stream'});
    const events = tool && !finished ? [
      {choices:[{delta:{tool_calls:[{index:0,id:'plugin-call',type:'function',function:{name:tool.function.name,arguments:'{}'}}]}}]},
      {choices:[{delta:{},finish_reason:'tool_calls'}],usage:{prompt_tokens:5,completion_tokens:3,total_tokens:8}},
    ] : [{choices:[{delta:{content:'Plugin completed.'}}]},{choices:[{delta:{},finish_reason:'stop'}],usage:{prompt_tokens:5,completion_tokens:3,total_tokens:8}}];
    for (const event of events) res.write(`data: ${JSON.stringify(event)}\n\n`);
    res.end('data: [DONE]\n\n');
  } else { res.writeHead(404); res.end(); }
});
await new Promise(resolve=>fixture.listen(0,'127.0.0.1',resolve));
const vendor = `http://127.0.0.1:${fixture.address().port}`;
const reservation = createServer();
await new Promise(resolve=>reservation.listen(0,'127.0.0.1',resolve));
const port = reservation.address().port;
await new Promise(resolve=>reservation.close(resolve));
const api = `http://127.0.0.1:${port}`;
const log = await open(`${scratch}/backend.log`,'a');
let backend;
async function stop() {
  if (backend && backend.exitCode===null) { backend.kill('SIGTERM'); await once(backend,'exit'); }
}
async function start() {
  backend = spawn(process.env.GRENGIN_BACKEND_BINARY,[], {cwd:root, env:{
    PATH:process.env.PATH,HOME:process.env.HOME,DATABASE_URL:process.env.DATABASE_URL,
    APP_KEY:process.env.APP_KEY,JWT_SECRET:process.env.JWT_SECRET,
    HOST:'127.0.0.1',PORT:String(port),REDIRECT_URL:'http://localhost:5173',
    GRENGIN_METADATA_BASE_URL:vendor+'/metadata/',
    RAG_ENABLED:'false',FILE_STORAGE_ROOT:`${scratch}/files`,
  },stdio:['ignore',log.fd,log.fd]});
  for(let attempt=0;attempt<120;attempt++) {
    if(backend.exitCode!==null) throw new Error('Backend exited during startup');
    try { if((await fetch(api+'/',{signal:AbortSignal.timeout(1000)})).ok) return; } catch {}
    await new Promise(resolve=>setTimeout(resolve,250));
  }
  throw new Error('Backend readiness timed out');
}
async function call(method,path,body,status=200,id=admin) {
  const response=await fetch(api+path,{method,redirect:'manual',headers:{'Content-Type':'application/json',...(id?{Authorization:`Bearer ${bearer(id)}`}:{})},body:body===undefined?undefined:JSON.stringify(body),signal:AbortSignal.timeout(45000)});
  const text=await response.text();
  assert.equal(response.status,status,`${method} ${path}: ${text.slice(-1000)}`);
  return {text,headers:response.headers,json:()=>JSON.parse(text)};
}
const manifest={schema_version:1,id:'fixture-connector',version:'1.0.0',name:'Fixture connector',base_url:vendor+'/v1',auth:'oauth2',tools:[{
  name:'plugin_echo',description:'Read fixture data',input_schema:{type:'object',additionalProperties:false},read_only:true,
  request:{method:'GET',path:'/echo'},response:{result:{$get:'session.response'}},
}]};
const catalogManifest={...manifest,base_url:'https://vendor.example/v1'};
const catalogBytes=JSON.stringify(catalogManifest);
const connection_config={connector_plugin:{manifest,configuration:{},allow_private_network:true,allow_insecure_http:true},oauth:{auth_url:vendor+'/authorize',token_url:vendor+'/token',scopes:['read'],redirect_url:'http://localhost:5173/mcp/oauth/callback'}};
const request={name:'Fixture plugin',transport_type:'http',url:manifest.base_url,enabled:true,default_access:'all_users',client_id:'fixture-client',client_secret:'fixture-secret',connection_config};
let server;
async function connect(id,code) {
  const response=(await call('POST',`/mcp/connections/${server.id}/authorize`,{},200,id)).json();
  const state=new URL(response.authorization_url).searchParams.get('state');
  await call('GET',`/mcp/oauth/callback?state=${encodeURIComponent(state)}&code=${code}`,undefined,200,id);
}
async function chat(id=admin) {
  const result=await call('POST','/chat/stream',{provider:'plugin-chat-fixture',model_name:'fixture-model',selected_mcp_servers:[server.id],messages:[{role:'user',content:'Use plugin_echo',files:[]}]},200,id);
  assert(!result.text.includes('ai_error'),result.text.slice(-1000));
  return result.text;
}
async function changeDuringModelReply(name,change,restore) {
  let ready,release;
  const waiting=new Promise(resolve=>{ready=resolve;});
  const released=new Promise(resolve=>{release=resolve;});
  modelGate={ready,released};
  const count=vendorCalls.length;
  const running=chat(member);
  try {
    await Promise.race([waiting,new Promise((_,reject)=>setTimeout(()=>reject(new Error('Model gate timed out')),10000))]);
    await change();
  } finally {release();}
  try {await running;check(name,vendorCalls.length,count);} finally {await restore();}
}
async function sqlxStdio(version) {
  const child=spawn(join(dirname(process.env.GRENGIN_BACKEND_BINARY),'sqlx-mcp'),['--database-url',process.env.DATABASE_URL,'--log-level','error'],{stdio:['pipe','pipe','ignore']});
  const lines=createInterface({input:child.stdout});
  const pending=new Map();
  lines.on('line',line=>{const reply=JSON.parse(line);pending.get(reply.id)?.(reply);});
  const rpc=(id,method,params)=>new Promise((resolve,reject)=>{
    const timer=setTimeout(()=>{pending.delete(id);reject(new Error(`SQLx MCP ${method} timed out`));},10000);
    pending.set(id,reply=>{clearTimeout(timer);pending.delete(id);resolve(reply);});
    child.stdin.write(JSON.stringify({jsonrpc:'2.0',id,method,params})+'\n');
  });
  try {
    const initialized=await rpc(1,'initialize',{protocolVersion:version,capabilities:{},clientInfo:{name:'plugin-regression',version:'1.0.0'}});
    assert.equal(initialized.result.protocolVersion,version);
    child.stdin.write(JSON.stringify({jsonrpc:'2.0',method:'notifications/initialized'})+'\n');
    const listed=await rpc(2,'tools/list',{});assert(listed.result.tools.some(tool=>tool.name==='sql_query'));
    const query=await rpc(3,'tools/call',{name:'sql_query',arguments:{sql:'SELECT 42 AS answer'}});
    assert(!query.error,JSON.stringify(query));assert(JSON.stringify(query.result).includes('42'));
    check(`updated sqlx-mcp initializes, lists tools and queries over stdio (${version})`,true,true);
  } finally {lines.close();child.kill('SIGTERM');if(child.exitCode===null)await once(child,'exit');}
}
try {
  await start();
  await sqlxStdio('2024-11-05');await sqlxStdio('2025-11-25');
  for(const id of [admin,member]) sql(`INSERT INTO users(id,status,email,"emailVerified",name,"mfaEnabled","createdAt","updatedAt","lastLoginAt","isIndependent") VALUES (${quote(id)},'active',${quote(`${id}@example.invalid`)},true,'Plugin test',false,now(),now(),now(),true)`);
  for(const [id,role] of [[admin,'Super Admin'],[member,'User']]) sql(`INSERT INTO user_role_assignments(id,"userId","roleId","assignedBy","createdAt","updatedAt") SELECT gen_random_uuid(),${quote(id)},id,${quote(admin)},now(),now() FROM roles WHERE name=${quote(role)}`);
  await call('POST','/admin/mcp-servers',request,401,null);
  await call('POST','/admin/mcp-servers',request,403,member);
  check('only an authenticated authorized admin can install plugins',true,true);
  const initial=Number(sql('SELECT count(*) FROM mcp_servers'));
  const discovered=(await call('GET','/discovery/connector-plugins',undefined,200,null)).json();
  check('public discovery lists only pinned connector packages',discovered.providers.map(value=>[value.id,value.selected_version]),[['fixture-connector','1.0.0']]);
  const detail=await call('GET','/discovery/connector-plugins/fixture-connector?version=1',undefined,200,null);
  check('discovery returns a hash-verified package and derived tool categories',detail.json().tool_groups,{read_only:['plugin_echo'],read_write:[]});
  check('discovery package hash matches exact catalog bytes',detail.json().sha256,createHash('sha256').update(catalogBytes).digest('hex'));
  const conditional=await fetch(api+'/discovery/connector-plugins/fixture-connector?version=1',{headers:{'If-None-Match':detail.headers.get('etag')}});
  check('connector discovery supports conditional requests without authentication',conditional.status,304);
  check('discovery never installs catalog plugins into the database',Number(sql('SELECT count(*) FROM mcp_servers')),initial);
  await call('GET','/discovery/connector-plugins/not-listed',undefined,404,null);
  await call('GET','/discovery/connector-plugins/fixture-connector?version=2',undefined,404,null);
  await call('GET','/discovery/connector-plugins?version=invalid',undefined,400,null);
  check('discovery refuses unlisted providers and unsupported version selectors',true,true);
  for (const oauth of [{},{...connection_config.oauth,scopes:['bad scope']},{...connection_config.oauth,token_url:'http://169.254.169.254/token'}]) {
    const invalid=structuredClone(request);invalid.connection_config.oauth=oauth;
    await call('POST','/admin/mcp-servers',invalid,400);
  }
  await call('POST','/admin/mcp-servers',{...request,client_id:''},400);
  const badFlow=structuredClone(request);badFlow.connection_config.oauth.flow='unsupported';
  await call('POST','/admin/mcp-servers',badFlow,400);
  check('invalid OAuth installs write no server rows',Number(sql('SELECT count(*) FROM mcp_servers')),initial);
  server=(await call('POST','/admin/mcp-servers',request,201)).json();
  check('installation persists exactly one explicit server',Number(sql('SELECT count(*) FROM mcp_servers')),initial+1);
  check('manifest tools are synchronized without an OAuth login or manual sync',sql(`SELECT count(*) FROM mcp_tools WHERE "serverId"=${quote(server.id)}`),'1');
  check('manifest is persisted in the existing server JSON field',JSON.parse(sql(`SELECT "connectionConfig" FROM mcp_servers WHERE id=${quote(server.id)}`)).connector_plugin.manifest.version,'1.0.0');
  check('OAuth client secret is encrypted at rest',sql(`SELECT "clientSecret" FROM mcp_servers WHERE id=${quote(server.id)}`).includes('fixture-secret'),false);
  await call('POST','/connectors/slack/connect',{version:'1.0.0',server_id:server.id},404,member);
  check('custom plugin servers cannot enter the separate hosted OAuth broker flow',true,true);
  check('rejected plugin broker bindings write no connector grants',Number(sql('SELECT count(*) FROM connectors')),0);
  const tools=(await call('GET','/mcp/tools',undefined,200,member)).json();
  check('installed tools reach the existing user tool listing',JSON.stringify(tools).includes('plugin_echo'),true);
  const provider=JSON.parse(await readFile(`${root}/llm-plugin/examples/openai-compatible.provider.json`,'utf8'));
  provider.id='plugin-chat-fixture';provider.name='Plugin fixture';delete provider.operations.listModels;
  provider.models=[{id:'fixture-model',name:'Fixture model',modelType:'text_generator',capabilities:{chat:provider.capabilities.chat},metadata:{inputTokenRate:0,outputTokenRate:0}}];
  await call('POST','/admin/ai-engines',{api_key:'fixture-only',is_enabled:true,whitelisted_models:['fixture-model'],default_model:'fixture-model',plugin_config:{manifest:provider,configuration:{},baseUrlOverride:vendor+'/',allowInsecureHttp:true,allowPrivateNetwork:true}},201);
  const before=vendorCalls.length;
  await chat();
  check('missing per-user authorization never reaches vendor API',vendorCalls.length,before);
  await connect(admin,'alice');await connect(member,'bob');
  check('stored OAuth access tokens are encrypted',sql('SELECT string_agg("accessToken",\',\') FROM mcp_connections').includes('fixture-alice'),false);
  check('chat consumes the plugin result through the existing tool loop',(await chat()).includes('Plugin completed.'),true);
  check('admin call uses only the admin connection token',vendorCalls.at(-1),'Bearer fixture-alice');
  await chat(member);
  check('member call uses only the member connection token',vendorCalls.at(-1),'Bearer fixture-bob');
  const toolId=sql(`SELECT id FROM mcp_tools WHERE "serverId"=${quote(server.id)}`);
  await changeDuringModelReply('revoking server access during generation prevents vendor execution',
    ()=>call('PUT',`/admin/mcp-servers/${server.id}`,{default_access:'admin_only'}),
    ()=>call('PUT',`/admin/mcp-servers/${server.id}`,{default_access:'all_users'}));
  await changeDuringModelReply('disabling a tool during generation prevents vendor execution',
    async()=>sql(`UPDATE mcp_tools SET enabled=false WHERE id=${quote(toolId)}`),
    async()=>sql(`UPDATE mcp_tools SET enabled=true WHERE id=${quote(toolId)}`));
  await changeDuringModelReply('deactivating the caller during generation prevents vendor execution',
    async()=>sql(`UPDATE users SET status='deactivated' WHERE id=${quote(member)}`),
    async()=>sql(`UPDATE users SET status='active' WHERE id=${quote(member)}`));
  await call('PUT',`/admin/mcp-servers/${server.id}`,{default_access:'admin_only'});
  const deniedCount=vendorCalls.length;await chat(member);
  check('existing MCP access policies still block unauthorized chat calls',vendorCalls.length,deniedCount);
  await call('PUT',`/admin/mcp-servers/${server.id}`,{default_access:'all_users'});
  const oldTool=sql(`SELECT id FROM mcp_tools WHERE "serverId"=${quote(server.id)}`);
  sql(`UPDATE mcp_tools SET enabled=false,"inheritAccessFromServer"=false WHERE id=${quote(oldTool)}`);
  await call('POST',`/admin/mcp-servers/${server.id}/sync-tools`,{});
  check('explicit tool sync preserves disabled tools and access overrides',sql(`SELECT id::text||':'||enabled::text||':'||"inheritAccessFromServer"::text FROM mcp_tools WHERE "serverId"=${quote(server.id)}`),`${oldTool}:false:false`);
  const revision=structuredClone(connection_config);revision.connector_plugin.manifest.version='1.0.1';
  await call('PUT',`/admin/mcp-servers/${server.id}`,{connection_config:revision});
  check('updates preserve tool IDs and admin access/enablement choices',sql(`SELECT id::text||':'||enabled::text||':'||"inheritAccessFromServer"::text FROM mcp_tools WHERE "serverId"=${quote(server.id)}`),`${oldTool}:false:false`);
  sql(`UPDATE mcp_tools SET enabled=true,"inheritAccessFromServer"=true WHERE id=${quote(oldTool)}`);
  const invalid=structuredClone(revision);invalid.connector_plugin.manifest.tools[0].request.headers={Authorization:'bad'};
  await call('PUT',`/admin/mcp-servers/${server.id}`,{connection_config:invalid},400);
  check('invalid update leaves the previous saved version intact',JSON.parse(sql(`SELECT "connectionConfig" FROM mcp_servers WHERE id=${quote(server.id)}`)).connector_plugin.manifest.version,'1.0.1');
  await chat();check('invalid update leaves the previous runtime usable',vendorCalls.at(-1),'Bearer fixture-alice');
  sql(`CREATE FUNCTION reject_plugin_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.event='mcp.plugin_saved' THEN RAISE EXCEPTION 'fixture audit failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_plugin_audit BEFORE INSERT ON auth_audit_events FOR EACH ROW EXECUTE FUNCTION reject_plugin_audit()`);
  const rejected=structuredClone(revision);rejected.connector_plugin.manifest.version='1.0.2';
  await call('PUT',`/admin/mcp-servers/${server.id}`,{name:'Should rollback',connection_config:rejected},503);
  await call('POST','/admin/mcp-servers',{...request,name:'Should not exist'},503);
  check('audit failure rolls back server and tool creation',sql("SELECT count(*) FROM mcp_servers WHERE name='Should not exist'"),'0');
  check('audit failure rolls back updates',sql(`SELECT name FROM mcp_servers WHERE id=${quote(server.id)}`),'Fixture plugin');
  sql('DROP TRIGGER reject_plugin_audit ON auth_audit_events; DROP FUNCTION reject_plugin_audit()');
  sql(`CREATE FUNCTION delay_plugin_update() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.name='Concurrent A' THEN PERFORM pg_sleep(0.5); END IF; RETURN NEW; END $$; CREATE TRIGGER delay_plugin_update BEFORE UPDATE ON mcp_servers FOR EACH ROW EXECUTE FUNCTION delay_plugin_update()`);
  const concurrent=structuredClone(revision);concurrent.connector_plugin.manifest.version='1.0.3';
  const first=call('PUT',`/admin/mcp-servers/${server.id}`,{name:'Concurrent A',connection_config:concurrent});
  await new Promise(resolve=>setTimeout(resolve,100));
  const second=call('PUT',`/admin/mcp-servers/${server.id}`,{name:'Concurrent B'});
  await Promise.all([first,second]);
  check('concurrent partial admin updates preserve the latest manifest',JSON.parse(sql(`SELECT "connectionConfig" FROM mcp_servers WHERE id=${quote(server.id)}`)).connector_plugin.manifest.version,'1.0.3');
  sql('DROP TRIGGER delay_plugin_update ON mcp_servers; DROP FUNCTION delay_plugin_update()');
  await stop();await start();
  await chat();check('restart reloads installed manifest and per-user encrypted credentials',vendorCalls.at(-1),'Bearer fixture-alice');
  await call('PUT',`/admin/mcp-servers/${server.id}`,{client_id:'replacement-client'});
  check('changing the OAuth binding invalidates all old saved tokens',sql(`SELECT count(*) FROM mcp_connections WHERE "serverId"=${quote(server.id)} AND (connected=true OR "accessToken" IS NOT NULL OR "refreshToken" IS NOT NULL)`),'0');
  const count=vendorCalls.length;await chat();check('changed OAuth identity cannot reuse an earlier token',vendorCalls.length,count);
  const mixed=structuredClone(request);mixed.name='Mixed';mixed.connection_config.grengin_connector_key='slack';
  await call('POST','/admin/mcp-servers',mixed,400);
  await call('POST','/admin/mcp-servers',{...request,name:'Wrong base',url:vendor+'/elsewhere'},400);
  check('custom manifests cannot borrow centrally approved broker identities or a mismatched URL',true,true);
  sql(`CREATE FUNCTION reject_plugin_delete_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.event='mcp.plugin_deleted' THEN RAISE EXCEPTION 'fixture delete audit failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER reject_plugin_delete_audit BEFORE INSERT ON auth_audit_events FOR EACH ROW EXECUTE FUNCTION reject_plugin_delete_audit()`);
  await call('DELETE',`/admin/mcp-servers/${server.id}`,undefined,503);
  check('delete audit failure retains the server and its tool metadata',sql(`SELECT count(*) FROM mcp_tools WHERE "serverId"=${quote(server.id)}`),'1');
  sql('DROP TRIGGER reject_plugin_delete_audit ON auth_audit_events; DROP FUNCTION reject_plugin_delete_audit()');
  await call('DELETE',`/admin/mcp-servers/${server.id}`,undefined,204);
  check('deleting a plugin removes its tool metadata',sql(`SELECT count(*) FROM mcp_tools WHERE "serverId"=${quote(server.id)}`),'0');
  check('deleting a plugin removes saved OAuth sessions',sql(`SELECT count(*) FROM mcp_connections WHERE "serverId"=${quote(server.id)}`),'0');
  console.log(`Plugin HTTP checks passed: ${checks}. All credentials and vendor calls were local fixtures.`);
} catch(error) {
  console.error((await readFile(`${scratch}/backend.log`,'utf8')).slice(-5000));
  throw error;
} finally {
  await stop();fixture.closeAllConnections();await new Promise(resolve=>fixture.close(resolve));await log.close();
}
