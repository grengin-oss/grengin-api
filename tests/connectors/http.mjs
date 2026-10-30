import assert from 'node:assert/strict';
import { createHmac,randomUUID } from 'node:crypto';
import { createServer } from 'node:http';
import { createServer as createHttpsServer } from 'node:https';
import { createRequire } from 'node:module';
import { readFile,open } from 'node:fs/promises';
import { execFileSync,spawn } from 'node:child_process';
import { pathToFileURL } from 'node:url';

const root = process.env.GRENGIN_BACKEND_ROOT;
const website = process.cwd();
const scratch = process.env.GRENGIN_CONNECTOR_TEST_ROOT;
const db = new URL(process.env.DATABASE_URL);
assert.equal(db.hostname,'127.0.0.1'); assert.equal(db.pathname,'/grengin_connectors_test');
const {createDatabase} = await import(pathToFileURL(`${website}/workers/test/helpers/sqlite-d1.mjs`));
const {handleConnectorBroker} = await import(pathToFileURL(`${website}/workers/src/services/connector-broker.ts`));
const require = createRequire(`${website}/workers/package.json`);
const {getPublicKeyAsync} = await import(pathToFileURL(require.resolve('@noble/ed25519')));
const originalFetch = globalThis.fetch;
const {db:workerDb,sqlite} = createDatabase();
const user = randomUUID(); const other = randomUUID();
const installation = `inst_connector_${randomUUID().replaceAll('-','')}`;
const privateKey = crypto.getRandomValues(new Uint8Array(32));
const publicKey = Buffer.from(await getPublicKeyAsync(privateKey)).toString('hex');
sqlite.prepare("INSERT INTO organizations (id,name,created_at,updated_at) VALUES ('org_connector','Connector tests','now','now')").run();
sqlite.prepare(`INSERT INTO installations(id,organization_id,name,public_key,environment,is_revoked,created_at,instance_uuid,connection_status,kind)
  VALUES (?,'org_connector','Local tests',?,'prod',0,'now',?,'connected','standalone')`).run(installation,publicKey,user);
const oauth = {auth_url:'https://vendor.test/authorize',token_url:'https://vendor.test/token',provider:'vendor',type:'oauth',scopes:['read:tools'],token_endpoint_auth_method:'client_secret_post'};
const env = {DB:workerDb,SERVICE_BASE_URL:'https://service.grengin.com',CONNECTOR_BROKER_ENABLED:'true',CONNECTOR_TOKEN_KEYS:JSON.stringify({'1':'b'.repeat(64)}),
  CONNECTOR_TOKEN_KEY_VERSION:'1',CONNECTOR_OAUTH_CLIENTS:JSON.stringify({vendor:{client_id:'connector-client',client_secret:'FAKE_CLIENT_SECRET',auth_url:oauth.auth_url,token_url:oauth.token_url}})};
let vendorUrl; let api; let server; let providerCalls=0; let toolCalls=0; let modelCalls=0; let availability=true;let brokerFault;let redirectVendor=false;let leakUrl;let leakedRequests=0;let revocationAvailable=true;
const mcpRequests=[];
const log = await open(`${scratch}/backend.log`,'a');
const sql = query=>execFileSync('psql',['-X','-A','-t','-v','ON_ERROR_STOP=1',process.env.DATABASE_URL,'-c',query],{encoding:'utf8',stdio:['ignore','pipe','pipe']}).trim();
const quote = value=>`'${String(value).replaceAll("'","''")}'`;
const token = id=>{
  const body=[{alg:'HS256',typ:'JWT'},{user_id:id,sub:'connector-local@example.invalid',name:'Connector test',token_use:'access',exp:Math.floor(Date.now()/1000)+1800}]
    .map(value=>Buffer.from(JSON.stringify(value)).toString('base64url')).join('.');
  return `${body}.${createHmac('sha256',process.env.JWT_SECRET).update(body).digest('base64url')}`;
};
let passed=0;
function check(name,actual,expected){assert.deepEqual(actual,expected,name);console.log(`PASS ${++passed}: ${name}`);}
async function call(method,path,body,bearer=token(user)){
  const response=await originalFetch(`${api}${path}`,{method,headers:{'Content-Type':'application/json',...(bearer?{Authorization:`Bearer ${bearer}`}:{})},
    body:body===undefined?undefined:JSON.stringify(body),signal:AbortSignal.timeout(45000)});
  const text=await response.text();return {status:response.status,text,json:()=>JSON.parse(text),headers:response.headers};
}
async function expect(method,path,body,status,bearer){const response=await call(method,path,body,bearer);assert.equal(response.status,status,`${method} ${path}: ${response.text.slice(0,400)}`);return response;}
async function listen(value){await new Promise(resolve=>value.listen(0,'127.0.0.1',resolve));return value.address().port;}
const vendor=createHttpsServer({key:await readFile(`${scratch}/key.pem`),cert:await readFile(`${scratch}/cert.pem`)},async(req,res)=>{
  if(redirectVendor){res.writeHead(307,{Location:leakUrl});res.end();return;}
  let raw='';for await(const chunk of req)raw+=chunk;
  let message;try{message=JSON.parse(raw);}catch{res.writeHead(400);res.end();return;}
  mcpRequests.push({method:message.method,authorized:req.headers.authorization==='Bearer FAKE_VENDOR_ACCESS'});
  if(req.headers.authorization!=='Bearer FAKE_VENDOR_ACCESS'){res.writeHead(401);res.end();return;}
  if(message.id===undefined||message.id===null){res.writeHead(202);res.end();return;}
  const result=message.method==='initialize'?{protocolVersion:message.params.protocolVersion,capabilities:{tools:{}},serverInfo:{name:'connector-fixture',version:'1.0.0'}}:
    message.method==='tools/list'?{tools:[{name:'audit_echo',description:'Echo local test data',inputSchema:{type:'object',properties:{message:{type:'string'}},required:['message']},annotations:{readOnlyHint:true}}]}:
    message.method==='tools/call'?(toolCalls++,{content:[{type:'text',text:'connector tool completed'}],isError:false}):{};
  res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify({jsonrpc:'2.0',id:message.id,result}));
});
const bridge=createServer(async(req,res)=>{
  try{
    let raw='';for await(const chunk of req)raw+=chunk;
    if(req.url==='/credential-leak'){leakedRequests++;res.writeHead(200);res.end('{}');return;}
    if(req.url.includes('/chat/completions')){
      modelCalls++;const body=JSON.parse(raw); const tool=body.tools?.find(tool=>tool.function?.name.includes('audit_echo'));
      const finished=body.messages.some(message=>message.role==='tool');
      res.writeHead(200,{'Content-Type':'text/event-stream'});
      const events=(!finished&&tool)?[
        {choices:[{delta:{tool_calls:[{index:0,id:'connector-call',type:'function',function:{name:tool.function.name,arguments:'{"message":"hello"}'}}]}}]},
        {choices:[{delta:{},finish_reason:'tool_calls'}],usage:{prompt_tokens:5,completion_tokens:3,total_tokens:8}}
      ]:[{choices:[{delta:{content:'Tool completed.'}}]},{choices:[{delta:{},finish_reason:'stop'}],usage:{prompt_tokens:5,completion_tokens:3,total_tokens:8}}];
      for(const event of events)res.write(`data: ${JSON.stringify(event)}\n\n`);res.end('data: [DONE]\n\n');return;
    }
    if(!availability){res.writeHead(503,{'Content-Type':'application/json'});res.end('{}');return;}
    if(brokerFault){res.writeHead(brokerFault.status,{'Content-Type':brokerFault.html?'text/html':'application/json',...(brokerFault.retry_after?{'Retry-After':brokerFault.retry_after}:{})});res.end(brokerFault.html?'<html>DO_NOT_LEAK_BROKER_PRIVATE</html>':JSON.stringify({error:{type:brokerFault.code,message:'DO_NOT_LEAK_BROKER_PRIVATE'}}));return;}
    const response=await handleConnectorBroker(new Request('https://service.grengin.com'+req.url,{method:req.method,headers:req.headers,body:req.method==='GET'?undefined:raw}),env);
    res.writeHead(response.status,Object.fromEntries(response.headers));res.end(Buffer.from(await response.arrayBuffer()));
  }catch{res.writeHead(500);res.end();}
});
globalThis.fetch=async(url,init={})=>{
  if(String(url)==='https://meta.grengin.com/connectors/github/versions/1.0.0.json')return Response.json({key:'github',version:'1.0.0',protocols:[{transport:'streamable_http'}],connection:{transport_type:'http',url:vendorUrl,auth:oauth}});
  if(String(url)==='https://vendor.test/token'){providerCalls++;return Response.json({access_token:'FAKE_VENDOR_ACCESS',refresh_token:'FAKE_VENDOR_REFRESH',expires_in:3600,token_type:'Bearer'});}
  if(String(url)==='https://vendor.test/revoke')return new Response(null,{status:revocationAvailable?200:503});
  throw new Error('Unexpected broker outbound request');
};
try{
  vendorUrl=`https://localhost:${await listen(vendor)}/mcp`;
  const apps=JSON.parse(env.CONNECTOR_OAUTH_CLIENTS);apps.vendor.mcp_origins=[new URL(vendorUrl).origin];apps.vendor.scopes=oauth.scopes;env.CONNECTOR_OAUTH_CLIENTS=JSON.stringify(apps);
  const broker=`http://127.0.0.1:${await listen(bridge)}`;
  leakUrl=broker+'/credential-leak';
  const reservation=createServer();const port=await listen(reservation);await new Promise(resolve=>reservation.close(resolve));api=`http://127.0.0.1:${port}`;
  const childEnv={PATH:process.env.PATH,HOME:process.env.HOME,DATABASE_URL:process.env.DATABASE_URL,APP_KEY:process.env.APP_KEY,JWT_SECRET:process.env.JWT_SECRET,
    HOST:'127.0.0.1',PORT:String(port),REDIRECT_URL:'http://localhost:5173',RAG_ENABLED:'false',FILE_STORAGE_ROOT:`${scratch}/files`,
    GRENGIN_SERVICE_URL:broker,GRENGIN_CONNECTOR_INSTALLATION_ID:installation,GRENGIN_CONNECTOR_PRIVATE_KEY:Buffer.from(privateKey).toString('hex'),SSL_CERT_FILE:`${scratch}/cert.pem`};
  childEnv.GRENGIN_CONNECTOR_CLEANUP_INTERVAL_SECS='5';
  server=spawn(process.env.GRENGIN_BACKEND_BINARY,[],{cwd:root,env:childEnv,stdio:['ignore',log.fd,log.fd]});
  let ready=false;
  for(let i=0;i<120;i++){
    if(server.exitCode!==null)throw new Error('Backend startup failed');
    try{const response=await originalFetch(api+'/');if(response.ok){ready=true;break;}}catch{}
    await new Promise(resolve=>setTimeout(resolve,500));
  }
  assert(ready,'backend readiness timed out');
  const expectedHead=(await readFile(`${root}/migration/HEAD`,'utf8')).trim();
  const health=await (await originalFetch(api+'/',{signal:AbortSignal.timeout(10000)})).json();
  check('running binary and database match the working-tree migration head',
    [health.expected_migration_head,health.migration_head],[expectedHead,expectedHead]);
  sql(`INSERT INTO users(id,status,email,"emailVerified",name,"mfaEnabled","createdAt","updatedAt","lastLoginAt","isIndependent") VALUES
    (${quote(user)},'active','connector-local@example.invalid',true,'Connector test',false,now(),now(),now(),true),
    (${quote(other)},'active','other-local@example.invalid',true,'Other test',false,now(),now(),now(),true)`);
  sql(`INSERT INTO user_role_assignments(id,"userId","roleId","assignedBy","createdAt","updatedAt") SELECT gen_random_uuid(),${quote(user)},id,${quote(user)},now(),now() FROM roles WHERE name='Super Admin'`);
  check('new database starts with no installed connectors',sql('SELECT count(*) FROM connectors'),'0');
  const spec=(await expect('GET','/openapi.json',undefined,200)).json();
  check('Swagger exposes the new completion endpoint with snake_case confirmation payload',!!spec.paths['/connectors/{connection_id}/complete']?.post&&!!spec.components.schemas.CompleteConnectorRequest?.properties?.confirmation_code,true);
  await expect('GET','/connectors',undefined,401,'');
  check('unauthenticated connector errors are also non-cacheable',(await call('GET','/connectors',undefined,'')).headers.get('cache-control'),'no-store');
  const created=(await expect('POST','/admin/mcp-servers',{name:'Connector fixture',transport_type:'http',url:vendorUrl,enabled:true,default_access:'all_users',client_id:'broker-managed',
    connection_config:{oauth:{auth_url:oauth.auth_url,token_url:oauth.token_url,scopes:oauth.scopes,redirect_url:'http://localhost:5173/mcp/oauth/callback'}}},201)).json();
  const payload={version:'1.0.0',server_id:created.id};
  sql(`CREATE FUNCTION reject_connector_test_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.event LIKE 'connector.%' THEN RAISE EXCEPTION 'forced connector audit failure'; END IF; RETURN NEW; END $$;
    CREATE TRIGGER reject_connector_test_audit BEFORE INSERT ON auth_audit_events FOR EACH ROW EXECUTE FUNCTION reject_connector_test_audit()`);
  await expect('POST','/connectors/github/connect',payload,503);
  check('required audit failure rolls back the local connection and prevents remote creation',[sql('SELECT count(*) FROM connectors'),sqlite.prepare('SELECT count(*) AS n FROM connector_connections').get().n],['0',0]);
  sql('DROP TRIGGER reject_connector_test_audit ON auth_audit_events; DROP FUNCTION reject_connector_test_audit()');
  const start=await expect('POST','/connectors/github/connect',payload,200);const info=start.json();
  await expect('POST','/connectors/another-key/connect',payload,409);
  check('two connector keys cannot silently claim the same user/server binding',sql('SELECT count(*) FROM connectors'),'1');
  check('real backend creates a pending broker connection',info.status,'pending');
  check('private connector response is not cacheable',start.headers.get('cache-control'),'no-store');
  assert(!start.text.includes('connection_grant'));assert(!start.text.includes('FAKE_VENDOR'));
  const encrypted=sql('SELECT "encryptedGrant" FROM connectors');assert(encrypted.startsWith('enc:v1:'));
  const repeated=(await expect('POST','/connectors/github/connect',payload,200)).json();
  check('repeated start reuses the same connection and authorization state',[repeated.id,repeated.authorization_url],[info.id,info.authorization_url]);
  await expect('GET',`/connectors/${info.id}`,undefined,404,token(other));
  await expect('POST',`/connectors/${info.id}/disconnect`,{},404,token(other));
  check('other users cannot read or disconnect a connection',sql('SELECT count(*) FROM connectors'),'1');
  const state=new URL(info.authorization_url).searchParams.get('state');
  const callback=new Request(`https://service.grengin.com/v1/connectors/callback/github?state=${state}&code=test-code`);
  const callbackResponse=await handleConnectorBroker(callback,env);assert.equal(callbackResponse.status,200);
  const confirmation=(await callbackResponse.text()).match(/\n([a-f0-9]{64})\n/)?.[1];assert(confirmation);
  assert.equal((await handleConnectorBroker(callback.clone(),env)).status,409);
  const waiting=await expect('GET',`/connectors/${info.id}`,undefined,200);
  check('provider callback alone leaves the backend connection inactive',waiting.json().status,'awaiting_confirmation');
  assert(!waiting.text.includes(confirmation));assert(!sql('SELECT row_to_json(connectors)::text FROM connectors').includes(confirmation));
  await expect('POST',`/admin/mcp-servers/${created.id}/sync-tools`,{},503);
  await expect('POST',`/connectors/${info.id}/complete`,{confirmation_code:confirmation},404,token(other));
  await expect('POST',`/connectors/${info.id}/complete`,{confirmation_code:'f'.repeat(64)},409);
  await expect('POST',`/connectors/${info.id}/complete`,{confirmationCode:confirmation},422);
  await expect('POST',`/connectors/${info.id}/complete`,{confirmation_code:'a'.repeat(17000)},413);
  sql(`CREATE FUNCTION reject_confirmation_test_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.event='connector.status_changed' THEN RAISE EXCEPTION 'forced confirmation audit failure'; END IF; RETURN NEW; END $$;
    CREATE TRIGGER reject_confirmation_test_audit BEFORE INSERT ON auth_audit_events FOR EACH ROW EXECUTE FUNCTION reject_confirmation_test_audit()`);
  await expect('POST',`/connectors/${info.id}/complete`,{confirmation_code:confirmation},503);
  check('failed local confirmation audit does not partially commit local approval',sql(`SELECT status FROM connectors WHERE id=${quote(info.id)}`),'awaiting_confirmation');
  sql('DROP TRIGGER reject_confirmation_test_audit ON auth_audit_events; DROP FUNCTION reject_confirmation_test_audit()');
  await expect('GET',`/connectors/${info.id}`,undefined,200);
  await expect('POST',`/connectors/${info.id}/complete`,{confirmation_code:confirmation},409);
  check('only the authenticated owner can consume the browser-only receipt',true,true);
  const status=(await expect('GET',`/connectors/${info.id}`,undefined,200)).json();check('approval is verified and persisted by the real backend',status.status,'connected');
  check('provider code is redeemed only once',providerCalls,1);
  assert(!JSON.stringify(status).includes('FAKE_VENDOR'));assert(!sql('SELECT row_to_json(connectors)::text FROM connectors').includes('FAKE_VENDOR'));
  const sealed=sql('SELECT "encryptedGrant" FROM connectors');
  sql(`UPDATE connectors SET "encryptedGrant"='enc:v1:tampered' WHERE id=${quote(info.id)}`);
  await expect('GET',`/connectors/${info.id}`,undefined,503);
  sql(`UPDATE connectors SET "encryptedGrant"=${quote(sealed)} WHERE id=${quote(info.id)}`);
  check('tampered encrypted grants fail closed rather than falling back to plaintext',true,true);
  availability=false;await expect('GET',`/connectors/${info.id}`,undefined,503);availability=true;
  check('broker outage cannot claim a cached live verification',true,true);
  for(const [upstream,code,expected,retry] of [[401,'invalid_connector_proof',403],[403,'installation_disconnected',403],[404,'connector_not_found',404],[409,'connector_reconnect_required',409],[400,'invalid_connector_request',400],[429,'connector_rate_limited',429,'13'],[500,'DO_NOT_LEAK_BROKER_PRIVATE',503]]){
    brokerFault={status:upstream,code,retry_after:retry};const rejected=await expect('GET',`/connectors/${info.id}`,undefined,expected);
    assert(!rejected.text.includes('DO_NOT_LEAK_BROKER_PRIVATE'));assert.equal(rejected.headers.get('cache-control'),'no-store');
    if(retry)assert.equal(rejected.headers.get('retry-after'),retry);
  }
  brokerFault=undefined;
  check('broker errors preserve safe 400/403/404/409/429 status and Retry-After without leaking bodies',true,true);
  brokerFault={status:429,html:true,retry_after:'47'};
  const edgeLimited=await expect('GET',`/connectors/${info.id}`,undefined,429);
  check('HTML edge rate limits preserve Retry-After without exposing upstream content',
    [edgeLimited.headers.get('retry-after'),edgeLimited.text.includes('DO_NOT_LEAK_BROKER_PRIVATE')],['47',false]);
  brokerFault=undefined;
  const firstConnected=status.connected_at;
  assert(firstConnected);await expect('GET',`/connectors/${info.id}`,undefined,200);
  check('connection approval time stays stable across status refreshes',(await expect('GET',`/connectors/${info.id}`,undefined,200)).json().connected_at,firstConnected);
  const synced=await expect('POST',`/admin/mcp-servers/${created.id}/sync-tools`,{},200);
  assert.equal(synced.json().success,true);assert.equal(synced.json().total_tools,1);
  check('MCP initialization and tool listing go directly to the vendor',mcpRequests.some(call=>call.authorized&&call.method==='tools/list'),true);
  const listed=(await expect('GET','/mcp-servers',undefined,200)).json();
  check('existing MCP listing reflects the broker-backed connection',listed.servers.find(item=>item.id===created.id)?.connected,true);
  const connectionList=(await expect('GET','/mcp/connections',undefined,200)).json();
  check('existing user connection listing includes the broker-backed server',connectionList.connections.find(item=>item.server_id===created.id)?.connected,true);
  const manifest=JSON.parse(await readFile(`${root}/llm-plugin/examples/openai-compatible.provider.json`,'utf8'));
  manifest.id='connector-fixture';manifest.name='Connector fixture';delete manifest.operations.listModels;
  manifest.models=[{id:'connector-model',name:'Connector model',modelType:'text_generator',capabilities:{chat:manifest.capabilities.chat},metadata:{inputTokenRate:0,outputTokenRate:0}}];
  await expect('POST','/admin/ai-engines',{api_key:'fixture-only',is_enabled:true,whitelisted_models:['connector-model'],default_model:'connector-model',
    plugin_config:{manifest,configuration:{},baseUrlOverride:broker+'/',allowInsecureHttp:true,allowPrivateNetwork:true}},201);
  const chat=await expect('POST','/chat/stream',{provider:manifest.id,model_name:'connector-model',selected_mcp_servers:[created.id],messages:[{role:'user',content:'Use audit_echo to say hello',files:[]}]},200);
  if(chat.text.includes('ai_error'))throw new Error('Chat fixture failed: '+chat.text.slice(-800));
  check('chat performs a direct authenticated MCP tool call',toolCalls>0,true);
  check('tool output is continued through the provider-neutral chat flow',chat.text.includes('Tool completed.'),true);
  const requestsBefore=mcpRequests.length;
  sql(`UPDATE mcp_servers SET url='https://unapproved.invalid/mcp' WHERE id=${quote(created.id)}`);
  await expect('POST',`/admin/mcp-servers/${created.id}/sync-tools`,{},503);
  sql(`UPDATE mcp_servers SET url=${quote(vendorUrl)} WHERE id=${quote(created.id)}`);
  check('an edited MCP endpoint cannot receive the pinned connector credential',mcpRequests.length,requestsBefore);
  redirectVendor=true;
  await expect('POST',`/admin/mcp-servers/${created.id}/sync-tools`,{},503);redirectVendor=false;
  check('authenticated MCP requests never follow a TLS-to-HTTP redirect',leakedRequests,0);
  sqlite.prepare('UPDATE installations SET is_revoked=1').run();
  await expect('POST',`/admin/mcp-servers/${created.id}/sync-tools`,{},503);
  check('revoked installation cannot retrieve new MCP credentials',true,true);
  sqlite.prepare('UPDATE installations SET is_revoked=0').run();
  availability=false;
  await expect('POST',`/connectors/${info.id}/disconnect`,{},503);
  const pendingRevocation=(await expect('GET',`/connectors/${info.id}`,undefined,200)).json();
  check('disconnect during outage stops local use and retains an encrypted retry handle',[pendingRevocation.status,pendingRevocation.revocation_pending],['revoked',true]);
  availability=true;
  const noResurrection=(await expect('GET',`/connectors/${info.id}`,undefined,200)).json();
  check('broker recovery cannot resurrect a locally revoked connection',noResurrection.status,'revoked');
  const disconnected=(await expect('POST',`/mcp/connections/${created.id}/disconnect`,{},200)).json();assert(disconnected.success);
  check('existing disconnect endpoint removes broker credentials',sql('SELECT count(*) FROM connectors'),'0');
  check('broker disconnect destroys encrypted vendor tokens',sqlite.prepare('SELECT secret_payload FROM connector_connections').get().secret_payload,null);
  await expect('POST','/connectors/unconfigured/connect',{version:'1.0.0'},503);
  const abandoned=(await expect('GET','/connectors',undefined,200)).json().find(item=>item.connector_key==='unconfigured');assert(abandoned);
  assert.equal(sqlite.prepare('SELECT id FROM connector_connections WHERE id=?').get(abandoned.id),undefined);
  await expect('POST',`/connectors/${abandoned.id}/disconnect`,{},200);
  check('a start that never created a broker row can be disconnected cleanly',sql(`SELECT count(*) FROM connectors WHERE id=${quote(abandoned.id)}`),'0');
  const reconnected=(await expect('POST','/connectors/github/connect',payload,200)).json();assert.notEqual(reconnected.id,info.id);
  check('reconnect uses a fresh connection and explicit consent',reconnected.status,'pending');
  const secondState=new URL(reconnected.authorization_url).searchParams.get('state');
  const secondCallback=await handleConnectorBroker(new Request(`https://service.grengin.com/v1/connectors/callback/github?state=${secondState}&code=second-code`),env);
  const secondCode=(await secondCallback.text()).match(/\n([a-f0-9]{64})\n/)?.[1];assert(secondCode);
  await expect('POST',`/connectors/${reconnected.id}/complete?confirmation%5Fcode=${secondCode}&connection_grant=QUERY_GRANT_SENTINEL`,{confirmation_code:secondCode},200);
  const auditPath=`/connectors/${reconnected.id}/complete`;
  for(let i=0;i<30 && sql(`SELECT count(*) FROM audit_logs WHERE details->>'path'=${quote(auditPath)}`)==='0';i++)await new Promise(resolve=>setTimeout(resolve,20));
  const audited=sql(`SELECT details->>'query' FROM audit_logs WHERE details->>'path'=${quote(auditPath)} ORDER BY "createdAt" DESC LIMIT 1`);
  check('encoded receipt and grant query parameters are redacted in actual audit rows',audited,'confirmation%5Fcode=[REDACTED]&connection_grant=[REDACTED]');
  sql(`UPDATE mcp_connections SET connected=true,"accessToken"='LEGACY_TOKEN_MUST_NOT_RETURN',"refreshToken"='LEGACY_REFRESH',"expiresAt"=NULL WHERE "userId"=${quote(user)} AND "serverId"=${quote(created.id)}`);
  const revokeApps=JSON.parse(env.CONNECTOR_OAUTH_CLIENTS);revokeApps.vendor.revocation_url='https://vendor.test/revoke';env.CONNECTOR_OAUTH_CLIENTS=JSON.stringify(revokeApps);
  revocationAvailable=false;
  const deferred=(await expect('POST',`/connectors/${reconnected.id}/disconnect`,{},200)).json();
  check('vendor revocation failure disables local use but retains an encrypted retry handle',[deferred.status,deferred.revocation_pending,deferred.provider_revocation],['revoked',true,'pending']);
  await expect('POST',`/admin/mcp-servers/${created.id}/sync-tools`,{},503);
  assert(sqlite.prepare('SELECT secret_payload FROM connector_connections WHERE id=?').get(reconnected.id).secret_payload);
  revocationAvailable=true;
  const finished=(await expect('POST',`/connectors/${reconnected.id}/disconnect`,{},200)).json();
  check('retry completes vendor revocation and removes local credentials',[finished.revocation_pending,finished.provider_revocation,sql('SELECT count(*) FROM connectors')],[false,'revoked','0']);
  check('successful broker disconnect cannot fall back to a stale legacy OAuth credential',sql(`SELECT count(*) FROM mcp_connections WHERE "userId"=${quote(user)} AND "serverId"=${quote(created.id)} AND (connected OR "accessToken" IS NOT NULL OR "refreshToken" IS NOT NULL)`),'0');
  sql(`INSERT INTO connectors(id,"userId","connectorKey","catalogVersion",status,scopes,"createdAt","updatedAt")
    SELECT gen_random_uuid(),${quote(other)},'seeded-'||n,'1.0.0','failed','[]'::jsonb,now(),now() FROM generate_series(1,64) AS n`);
  await expect('POST','/connectors/capacity-probe/connect',{version:'1.0.0'},409,token(other));
  check('connection capacity is enforced before a new row is persisted',sql(`SELECT count(*) FROM connectors WHERE "userId"=${quote(other)}`),'64');
  sql(`DELETE FROM connectors WHERE "userId"=${quote(other)}`);
  const limited=randomUUID();
  sql(`INSERT INTO users(id,status,email,"emailVerified",name,"mfaEnabled","createdAt","updatedAt","lastLoginAt","isIndependent") VALUES
    (${quote(limited)},'active','limited@example.invalid',true,'Limited test',false,now(),now(),now(),true)`);
  for(let i=0;i<20;i++)await expect('POST',`/connectors/bogus-${i}/connect`,{version:'1.0.0'},503,token(limited));
  const throttled=await expect('POST','/connectors/bogus-extra/connect',{version:'1.0.0'},429,token(limited));
  assert(Number(throttled.headers.get('retry-after'))>0);
  check('local throttling rejects bogus-key floods before additional database writes',sql(`SELECT count(*) FROM connectors WHERE "userId"=${quote(limited)}`),'20');

  const ownerConnection=(await expect('POST','/connectors/github/connect',{version:'1.0.0'},200,token(other))).json();
  const ownerState=new URL(ownerConnection.authorization_url).searchParams.get('state');
  const ownerCallback=await handleConnectorBroker(new Request(`https://service.grengin.com/v1/connectors/callback/github?state=${ownerState}&code=owner-code`),env);
  const ownerCode=(await ownerCallback.text()).match(/\n([a-f0-9]{64})\n/)?.[1];assert(ownerCode);
  await expect('POST',`/connectors/${ownerConnection.id}/complete`,{confirmation_code:ownerCode},200,token(other));
  assert.throws(()=>sql(`DELETE FROM users WHERE id=${quote(other)}`),/fk_connectors_user/);
  check('hard deletion cannot cascade away a grant awaiting broker cleanup',sql(`SELECT count(*) FROM connectors WHERE id=${quote(ownerConnection.id)}`),'1');
  sql(`UPDATE users SET status='deactivated' WHERE id=${quote(other)}`);
  await new Promise(resolve=>setTimeout(resolve,5500));
  check('temporarily deactivated owners retain their encrypted connection',sql(`SELECT status FROM connectors WHERE id=${quote(ownerConnection.id)}`),'connected');
  sql(`UPDATE users SET status='active' WHERE id=${quote(other)}`);
  availability=false;
  await expect('DELETE',`/admin/users/${other}`,undefined,204);
  await expect('GET','/connectors',undefined,401,token(other));
  async function eventually(predicate,label){
    for(let attempt=0;attempt<60;attempt++) {if(predicate())return;await new Promise(resolve=>setTimeout(resolve,250));}
    throw new Error(label);
  }
  await eventually(()=>sql(`SELECT status FROM connectors WHERE id=${quote(ownerConnection.id)}`)==='revoked','deleted-owner cleanup did not disable local use');
  check('deleted-owner cleanup retains its encrypted grant during a broker outage',sql(`SELECT "encryptedGrant" IS NOT NULL FROM connectors WHERE id=${quote(ownerConnection.id)}`),'t');
  revocationAvailable=false;availability=true;
  await eventually(()=>sqlite.prepare('SELECT status FROM connector_connections WHERE id=?').get(ownerConnection.id)?.status==='revoked','broker cleanup was not retried');
  check('vendor revocation outage retains both retry records',[
    sql(`SELECT "encryptedGrant" IS NOT NULL FROM connectors WHERE id=${quote(ownerConnection.id)}`),
    !!sqlite.prepare('SELECT secret_payload FROM connector_connections WHERE id=?').get(ownerConnection.id)?.secret_payload],['t',true]);
  revocationAvailable=true;
  await eventually(()=>sql(`SELECT count(*) FROM connectors WHERE id=${quote(ownerConnection.id)}`)==='0','deleted-owner credentials were not cleaned after recovery');
  check('background cleanup erases broker credentials after provider recovery',sqlite.prepare('SELECT secret_payload FROM connector_connections WHERE id=?').get(ownerConnection.id).secret_payload,null);
  check('automatic cleanup audits a system action rather than impersonating the deleted user',sql(`SELECT count(*) FROM auth_audit_events WHERE event='connector.disconnected' AND "actorId" IS NULL AND payload->>'owner_user_id'=${quote(other)} AND payload->>'system_cleanup'='true'`),'1');
  check('deleted-owner cleanup does not sweep active users connections',sql(`SELECT count(*) FROM connectors WHERE "userId"=${quote(limited)}`),'20');
  console.log(`Connector end-to-end suite: ${passed} checks passed; ${modelCalls} fixture model requests; no paid provider calls.`);
}catch(error){
  const logs=await readFile(`${scratch}/backend.log`,'utf8');console.error('Backend diagnostic tail:',logs.slice(-6000));throw error;
}finally{
  globalThis.fetch=originalFetch;
  if(server&&server.exitCode===null){server.kill('SIGTERM');await new Promise(resolve=>server.once('exit',resolve));}
  bridge.closeAllConnections();vendor.closeAllConnections();
  await Promise.all([new Promise(resolve=>bridge.close(resolve)),new Promise(resolve=>vendor.close(resolve))]);
  sqlite.close();await log.close();
}
