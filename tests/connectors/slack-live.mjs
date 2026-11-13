import assert from 'node:assert/strict';
import {createHmac, randomBytes, randomUUID} from 'node:crypto';
import {execFileSync, spawn} from 'node:child_process';
import {createServer, request as httpsRequest} from 'node:https';
import {request as httpRequest} from 'node:http';
import {createRequire} from 'node:module';
import {readFile, writeFile, mkdir, open} from 'node:fs/promises';
import {readdirSync, readFileSync} from 'node:fs';
import {DatabaseSync} from 'node:sqlite';
import {pathToFileURL} from 'node:url';

process.umask(0o077);
const root=process.env.GRENGIN_BACKEND_ROOT;
const website=process.env.GRENGIN_WEBSITE_ROOT;
const directory=process.env.GRENGIN_SLACK_STATE;
const origin='https://localhost:8787';
const localFrontend=process.env.GRENGIN_LOCAL_SLACK_FRONTEND==='1';
const realApp=process.env.GRENGIN_SLACK_REAL_APP==='1';
const browserOrigin=localFrontend?'http://localhost:5173':origin;
const browserCallback=localFrontend?'http://localhost:5173/mcp/oauth/callback':`${origin}/v1/connectors/callback/slack`;
const api='http://127.0.0.1:8080';
const require=createRequire(`${website}/workers/package.json`);
const {getPublicKeyAsync}=await import(pathToFileURL(require.resolve('@noble/ed25519')));
let handleConnectorBroker;
if(localFrontend){
  // Local-only shim: preserve the production HTTPS callback rules and source file.
  const source=await readFile(`${website}/workers/src/services/connector-broker.ts`,'utf8');
  const statement='const callback = `${secureUrl(env.SERVICE_BASE_URL).origin}/v1/connectors/callback/${key}`;';
  assert.equal(source.split(statement).length,2,'Broker callback changed; review the local shim');
  const esbuild=createRequire(require.resolve('tsx/package.json'))('esbuild');
  const result=await esbuild.build({stdin:{contents:source.replace(statement,`const callback = ${JSON.stringify(browserCallback)};`),
    resolveDir:`${website}/workers/src/services`,sourcefile:'connector-broker.local.ts',loader:'ts'},
    bundle:true,write:false,format:'esm',platform:'node',target:'es2022',logLevel:'silent'});
  ({handleConnectorBroker}=await import(`data:text/javascript;base64,${Buffer.from(result.outputFiles[0].contents).toString('base64')}`));
} else ({handleConnectorBroker}=await import(pathToFileURL(`${website}/workers/src/services/connector-broker.ts`)));
let config;
try {config=JSON.parse(await readFile(`${directory}/session.json`,'utf8'));}
catch(error) {
  if(error.code!=='ENOENT') throw error;
  config={user:randomUUID(),installation:`local_slack_${randomUUID().replaceAll('-','')}`,
    privateKey:randomBytes(32).toString('hex'),encryptionKey:randomBytes(32).toString('hex'),
    session:randomBytes(24).toString('hex'),password:randomBytes(24).toString('hex')};
}
const save=()=>writeFile(`${directory}/session.json`,JSON.stringify(config),{mode:0o600});
const base=`/${config.session}`;
const browserBase=localFrontend?`/__local/slack${base}`:base;
const browserHome=`${browserOrigin}${browserBase}/`;
if(process.argv.includes('--check-local')){
  const ca=await readFile(`${directory}/tls.crt`);
  function request(path,method='GET',headers={}){
    return new Promise((resolve,reject)=>{
      const req=(localFrontend?httpRequest:httpsRequest)(browserOrigin+path,{ca,family:4,method,headers,timeout:15000},res=>{
        let text='';res.on('data',data=>{text+=data;});res.on('end',()=>resolve({status:res.statusCode,headers:res.headers,text}));
      });req.on('error',reject);req.on('timeout',()=>req.destroy(new Error('Local check timeout')));req.end();
    });
  }
  const health=await (await fetch(api+'/')).json();
  assert.equal(health.migration_head,(await readFile(`${root}/migration/HEAD`,'utf8')).trim());
  assert.equal(health.expected_migration_head,health.migration_head);
  const home=await request(`${browserBase}/`);assert.equal(home.status,200);assert(home.text.includes('Connect Slack'));
  assert.equal(home.headers['cache-control'],'no-store');
  assert.equal((await request(`${browserBase}/connect`,'POST')).status,404);
  assert.equal((await request(`${browserBase}/connect`,'POST',{Origin:'https://untrusted.example'})).status,404);
  const start=await request(`${browserBase}/connect`,'POST',{Origin:browserOrigin});assert.equal(start.status,303);
  const url=new URL(start.headers.location);assert.equal(url.origin+url.pathname,'https://slack.com/oauth/v2_user/authorize');
  assert.equal(url.searchParams.get('redirect_uri'),browserCallback);
  assert.equal(url.searchParams.get('client_id'),process.env.SLACK_CLIENT_ID);
  assert.equal(url.searchParams.get('code_challenge_method'),'S256');
  assert.equal(url.searchParams.get('code_challenge').length,43);
  assert.match(url.searchParams.get('state'),/^[a-f0-9]{64}$/);
  assert(!(start.headers.location).includes(process.env.SLACK_CLIENT_SECRET));
  const stored=new DatabaseSync(`${directory}/broker.sqlite`,{readOnly:true});
  try{
    const row=stored.prepare("SELECT * FROM connector_connections WHERE status='pending' AND connector_key='slack' ORDER BY created_at DESC LIMIT 1").get();
    assert(row);
    const [version,iv,ciphertext]=row.secret_payload.split('.');assert.equal(version,'1');
    const key=await crypto.subtle.importKey('raw',Buffer.from(config.encryptionKey,'hex'),'AES-GCM',false,['decrypt']);
    const aad=[row.id,row.installation_id,row.local_user_id,row.connector_key,row.catalog_version].join('\n');
    const pending=JSON.parse(new TextDecoder().decode(await crypto.subtle.decrypt({name:'AES-GCM',iv:Buffer.from(iv,'hex'),
      additionalData:new TextEncoder().encode(aad)},key,Buffer.from(ciphertext,'hex'))));
    assert.equal(pending.callback,browserCallback);
    assert.equal(pending.client_id,process.env.SLACK_CLIENT_ID);
    assert.equal(Buffer.from(await crypto.subtle.digest('SHA-256',new TextEncoder().encode(pending.pkce))).toString('base64url'),url.searchParams.get('code_challenge'));
  } finally{stored.close();}
  assert.equal((await request((localFrontend?'/mcp/oauth/callback':'/v1/connectors/callback/slack')+'?state=invalid')).status,400);
  assert((await request(`${browserBase}/`)).text.includes('pending'));
  console.log('PASS: backend migration health; private local page; cross-origin request rejection; actual backend/broker start; Slack endpoint/client/callback/PKCE; encrypted exchange callback and verifier binding; invalid callback rejection; persisted pending status.');
  console.log('Real Slack consent and token redemption still require browser approval.');
  process.exit(0);
}
await save();
const sqlite=new DatabaseSync(`${directory}/broker.sqlite`);
sqlite.exec('PRAGMA foreign_keys=ON; CREATE TABLE IF NOT EXISTS local_schema(name TEXT PRIMARY KEY)');
for(const name of readdirSync(`${website}/workers/migrations`).filter(name=>name.endsWith('.sql')).sort()) {
  if(sqlite.prepare('SELECT name FROM local_schema WHERE name=?').get(name)) continue;
  sqlite.exec('BEGIN');
  try {sqlite.exec(readFileSync(`${website}/workers/migrations/${name}`,'utf8'));
    sqlite.prepare('INSERT INTO local_schema(name) VALUES(?)').run(name);sqlite.exec('COMMIT');}
  catch(error){sqlite.exec('ROLLBACK');throw error;}
}
const db={
  async batch(statements){sqlite.exec('BEGIN IMMEDIATE');try {const result=statements.map(s=>s.execute());sqlite.exec('COMMIT');return result;}
    catch(error){sqlite.exec('ROLLBACK');throw error;}},
  withSession(mode){assert.equal(mode,'first-primary');return db;},
  prepare(sql){const statement=sqlite.prepare(sql);const bound=(args=[])=>({
    execute:()=>({success:true,meta:{changes:Number(statement.run(...args).changes)}}),
    bind:(...args)=>bound(args),first:async()=>statement.get(...args)??null,
    all:async()=>({results:statement.all(...args),success:true}),
    run:async()=>({success:true,meta:{changes:Number(statement.run(...args).changes)}})});return bound();},
};
sqlite.prepare("INSERT OR IGNORE INTO organizations(id,name,created_at,updated_at) VALUES('slack_local_demo','Local Slack consent',datetime('now'),datetime('now'))").run();
sqlite.prepare(`INSERT OR IGNORE INTO installations(id,organization_id,name,public_key,environment,is_revoked,created_at,instance_uuid,connection_status,kind)
  VALUES(?,'slack_local_demo','Local Slack consent',?,'prod',0,datetime('now'),?,'connected','standalone')`)
  .run(config.installation,Buffer.from(await getPublicKeyAsync(Buffer.from(config.privateKey,'hex'))).toString('hex'),config.user);
const catalogResponse=await fetch('https://meta.grengin.com/connectors/slack/versions/1.0.0.json',{redirect:'error',signal:AbortSignal.timeout(15000)});
assert(catalogResponse.ok,'Slack catalog unavailable');
const catalog=await catalogResponse.json();
const auth=catalog.connection.auth;
const env={DB:db,SERVICE_BASE_URL:origin,CONNECTOR_BROKER_ENABLED:'true',CONNECTOR_TOKEN_KEY_VERSION:'1',
  CONNECTOR_TOKEN_KEYS:JSON.stringify({'1':config.encryptionKey}),
  CONNECTOR_OAUTH_CLIENTS:JSON.stringify({slack:{client_id:process.env.SLACK_CLIENT_ID,client_secret:process.env.SLACK_CLIENT_SECRET,
    auth_url:auth.auth_url,token_url:auth.token_url,mcp_origins:['https://mcp.slack.com'],scopes:auth.scopes,extra_params:auth.extra_params}})};
const signIn=()=>{const body=[{alg:'HS256',typ:'JWT'},{user_id:realApp?config.realUser:config.user,sub:realApp?'local-development-account':'slack-local@example.invalid',name:'Local Slack test',token_use:'access',
  exp:Math.floor(Date.now()/1000)+3600}].map(v=>Buffer.from(JSON.stringify(v)).toString('base64url')).join('.');
  return `${body}.${createHmac('sha256',process.env.JWT_SECRET).update(body).digest('base64url')}`;};
async function call(method,path,body){
  const response=await fetch(api+path,{method,headers:{Authorization:`Bearer ${signIn()}`,'Content-Type':'application/json'},
    ...(body===undefined?{}:{body:JSON.stringify(body)}),signal:AbortSignal.timeout(45000)});
  const value=await response.json();
  if(!response.ok) throw new Error(`${method} ${path}: HTTP ${response.status}; ${value.detail?.external_code||value.detail?.description||'request failed'}`);
  return value;
}
let backend;
function send(res,status,body,type='text/plain; charset=utf-8',extra={}){
  res.writeHead(status,{'Content-Type':type,'Cache-Control':'no-store','Referrer-Policy':'no-referrer','X-Content-Type-Options':'nosniff',
    'Content-Security-Policy':"default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'",...extra});res.end(body);
}
const escape=value=>String(value).replaceAll('&','&amp;').replaceAll('<','&lt;').replaceAll('>','&gt;').replaceAll('"','&quot;');
const page=async()=>{
  const rows=await call('GET','/connectors');
  const row=rows.find(r=>r.connector_key==='slack');
  return `<!doctype html><html><meta name="viewport" content="width=device-width"><title>Local Slack Connection</title>
  <style>body{font:16px system-ui;max-width:650px;margin:70px auto;padding:20px;color:#20242a}h1{font-size:28px}label,input{display:block}input{width:100%;box-sizing:border-box;padding:10px;margin:10px 0}button{padding:10px 16px;cursor:pointer}form{margin:24px 0}pre{white-space:pre-wrap;overflow-wrap:anywhere}a{color:#195aaa}</style>
  <h1>Local Slack Connection</h1><p>Isolated local account: slack-local@example.invalid</p><p>Status: <strong>${escape(row?.status||'disconnected')}</strong></p>
  <form action="${browserBase}/connect" method="post"><button>Connect Slack</button></form>
  <form action="${browserBase}/complete" method="post"><label for="confirmation">One-time confirmation code</label><input id="confirmation" name="confirmation_code" required pattern="[a-f0-9]{64}" autocomplete="off"><button>Complete connection</button></form>
  <form action="${browserBase}/status" method="post"><button>Check connection and tools</button></form>
  <form action="${browserBase}/disconnect" method="post"><button>Disconnect Slack</button></form></html>`;
};
const server=createServer({key:await readFile(`${directory}/tls.key`),cert:await readFile(`${directory}/tls.crt`)},async(req,res)=>{
  try {
    if(req.headers.host!=='localhost:8787'){send(res,403,'Unexpected host');return;}
    const url=new URL(req.url,origin);
    let raw='';for await(const chunk of req){raw+=chunk;if(Buffer.byteLength(raw)>16384){send(res,413,'Request too large');return;}}
    if(url.pathname.startsWith('/v1/connectors/')){
      const response=await handleConnectorBroker(new Request(url,{method:req.method,headers:req.headers,...(req.method==='GET'?{}:{body:raw})}),env);
      res.writeHead(response.status,Object.fromEntries(response.headers));res.end(Buffer.from(await response.arrayBuffer()));return;
    }
    if(realApp){send(res,404,'Use the Grengin webapp at http://localhost:5173/settings?tab=integrations');return;}
    if(req.method==='GET'&&url.pathname===`${base}/`){
      if(localFrontend&&req.headers['x-grengin-local-frontend']!=='1'){send(res,303,'Use the local frontend','text/plain',{'Location':browserHome});return;}
      send(res,200,await page(),'text/html; charset=utf-8');return;
    }
    if(req.method!=='POST'||req.headers.origin!==browserOrigin){send(res,404,'Not found');return;}
    if(url.pathname===`${base}/connect`){
      const rows=await call('GET','/connectors');
      const previous=rows.find(r=>r.connector_key==='slack');
      const expired=previous?.expires_at&&Date.parse(previous.expires_at)<=Date.now()&&previous.status!=='connected';
      if(previous&&(expired||['expired','failed','revoked'].includes(previous.status))) await call('POST',`/connectors/${previous.id}/disconnect`,{});
      const row=await call('POST','/connectors/slack/connect',{version:'1.0.0',server_id:config.serverId});
      if(row.authorization_url){send(res,303,'Continue to Slack','text/plain',{'Location':row.authorization_url});return;}
      send(res,303,'Return to connection','text/plain',{'Location':`${browserBase}/`});return;
    }
    const rows=await call('GET','/connectors');const row=rows.find(r=>r.connector_key==='slack');
    if(!row){send(res,409,'Start a Slack connection first');return;}
    if(url.pathname===`${base}/complete`){
      const code=new URLSearchParams(raw).get('confirmation_code');assert(/^[a-f0-9]{64}$/.test(code||''),'Invalid confirmation code');
      await call('POST',`/connectors/${row.id}/complete`,{confirmation_code:code});
      console.log('Slack owner confirmation completed; checking live state requires the status button.');
      send(res,303,'Connection confirmed','text/plain',{'Location':`${browserBase}/`});return;
    }
    if(url.pathname===`${base}/status`){
      const current=await call('GET',`/connectors/${row.id}`);
      let tools;
      if(current.status==='connected') tools=await call('POST',`/admin/mcp-servers/${config.serverId}/sync-tools`,{});
      const listed=await call('GET','/mcp-servers');const entry=listed.servers.find(v=>v.id===config.serverId);
      send(res,200,JSON.stringify({status:current.status,server_id:config.serverId,listed_connected:entry?.connected,
        tools_synced:tools?.success,total_tools:tools?.total_tools},null,2));return;
    }
    if(url.pathname===`${base}/disconnect`){await call('POST',`/connectors/${row.id}/disconnect`,{});send(res,303,'Disconnected','text/plain',{'Location':`${browserBase}/`});return;}
    send(res,404,'Not found');
  } catch(error){console.error('Local request failed:',error.message);send(res,502,`Local consent request failed: ${error.message}\nReturn to the local connection page to retry.`);}
});
server.requestTimeout=20000;
server.headersTimeout=10000;
await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(8787,'127.0.0.1',resolve);});
const dockerArgs=['docker'];
function docker(args){return execFileSync(dockerArgs[0],args,{encoding:'utf8',stdio:['ignore','pipe','pipe']}).trim();}
if(!docker(['ps','-a','--filter','name=^/grengin-slack-consent$','--format','{{.Names}}'])){
  await mkdir(`${directory}/postgres`,{recursive:true,mode:0o700});
  execFileSync('docker',['run','-d','--name','grengin-slack-consent','--memory=384m','--cpus=1',
    '-e','POSTGRES_USER=local','-e','POSTGRES_PASSWORD','-e','POSTGRES_DB=grengin_slack_consent',
    '-p','127.0.0.1::5432','--mount',`type=bind,source=${directory}/postgres,target=/var/lib/postgresql/data`,
    'pgvector/pgvector:pg16','-c','shared_buffers=32MB','-c','max_connections=25'],
    {env:{...process.env,POSTGRES_PASSWORD:config.password},stdio:['ignore','pipe','pipe']});
} else docker(['start','grengin-slack-consent']);
for(let n=0;n<60;n++){
  try {docker(['exec','grengin-slack-consent','pg_isready','-U','local','-d','grengin_slack_consent']);break;}
  catch {if(n===59)throw new Error('Isolated PostgreSQL did not become ready');await new Promise(r=>setTimeout(r,1000));}
}
const port=docker(['port','grengin-slack-consent','5432/tcp']).split(':').pop();
const databaseName=realApp?'grengin_slack_real':'grengin_slack_consent';
const database=`postgres://local:${config.password}@127.0.0.1:${port}/${databaseName}`;
await writeFile(`${directory}/trust.pem`,Buffer.concat([await readFile('/etc/ssl/certs/ca-certificates.crt'),await readFile(`${directory}/tls.crt`)]),{mode:0o600});
const log=await open(`${directory}/backend.log`,'a',0o600);
const authEnv=realApp?Object.fromEntries(Object.entries(process.env).filter(([key])=>/^(GOOGLE_|AZURE_(CLIENT|TENANT)|AUTH0_|SSO_PROXY_)/.test(key))):{};
backend=spawn(`${root}/target-security/debug/grengin-api`,[],{cwd:root,env:{...authEnv,PATH:process.env.PATH,HOME:process.env.HOME,
  DATABASE_URL:database,APP_KEY:process.env.APP_KEY,JWT_SECRET:process.env.JWT_SECRET,HOST:'127.0.0.1',PORT:'8080',
  REDIRECT_URL:'http://localhost:5173',FILE_STORAGE_ROOT:`${directory}/files`,RAG_ENABLED:'false',
  SSL_CERT_FILE:`${directory}/trust.pem`,GRENGIN_SERVICE_URL:origin,GRENGIN_CONNECTOR_INSTALLATION_ID:config.installation,
  GRENGIN_CONNECTOR_PRIVATE_KEY:config.privateKey},stdio:['ignore',log.fd,log.fd]});
await log.close();
for(let n=0;n<120;n++){
  if(backend.exitCode!==null)throw new Error('Backend startup failed; inspect the private backend log');
  try {if((await fetch(api+'/',{signal:AbortSignal.timeout(1000)})).ok)break;}
  catch{} if(n===119)throw new Error('Backend readiness timed out');await new Promise(r=>setTimeout(r,500));
}
const sql=query=>execFileSync('psql',['-X','-A','-t','-v','ON_ERROR_STOP=1','-c',query],
  {encoding:'utf8',env:{...process.env,PGHOST:'127.0.0.1',PGPORT:port,PGUSER:'local',PGPASSWORD:config.password,PGDATABASE:databaseName},stdio:['ignore','pipe','pipe']}).trim();
if(realApp){
  config.realUser=sql(`SELECT u.id FROM users u JOIN user_role_assignments a ON a."userId"=u.id JOIN roles r ON r.id=a."roleId"
    WHERE u.status='active' AND r.name='Super Admin' ORDER BY u."createdAt" LIMIT 1`);
  assert.match(config.realUser,/^[a-f0-9-]{36}$/,'The copied database needs an existing Super Admin');
} else sql(`INSERT INTO users(id,status,email,"emailVerified",name,"mfaEnabled","createdAt","updatedAt","lastLoginAt","isIndependent")
  VALUES('${config.user}','active','slack-local@example.invalid',true,'Local Slack test',false,now(),now(),now(),true) ON CONFLICT(id) DO NOTHING;
  INSERT INTO user_role_assignments(id,"userId","roleId","assignedBy","createdAt","updatedAt")
  SELECT gen_random_uuid(),'${config.user}',id,'${config.user}',now(),now() FROM roles WHERE name='Super Admin'
  AND NOT EXISTS(SELECT 1 FROM user_role_assignments WHERE "userId"='${config.user}');`);
if(!(realApp?config.realServerId:config.serverId)){
  const mcp=await call('POST','/admin/mcp-servers',{name:realApp?'Slack':'Slack (local OAuth broker)',transport_type:'http',url:catalog.connection.url,
    enabled:true,default_access:'all_users',client_id:'broker-managed',connection_config:{oauth:{auth_url:auth.auth_url,token_url:auth.token_url,
      scopes:auth.scopes,redirect_url:browserCallback}}});
  if(realApp)config.realServerId=mcp.id;else config.serverId=mcp.id;await save();
}
if(realApp){
  await save();
  await writeFile(`${directory}/real-server-id`,config.realServerId+'\n',{mode:0o600});
  console.log('READY: real Grengin webapp at http://localhost:5173/settings?tab=integrations');
  console.log(`Slack callback: ${browserCallback}; real existing accounts and model settings, original database unchanged.`);
} else {
const previous=(await call('GET','/connectors')).find(row=>row.connector_key==='slack');
if(previous?.status==='pending'&&previous.authorization_url&&new URL(previous.authorization_url).searchParams.get('redirect_uri')!==browserCallback)
  await call('POST',`/connectors/${previous.id}/disconnect`,{});
await writeFile(`${directory}/open-url`,`${browserHome}\n`,{mode:0o600});
console.log(`READY: ${browserHome}`);
console.log(`Slack callback: ${browserCallback}`);
console.log('Real Slack requests enabled; PostgreSQL and Worker storage are isolated and retained locally. No model calls are made.');
}
let closing=false;
async function stop(){if(closing)return;closing=true;backend.kill('SIGTERM');await new Promise(r=>server.close(r));sqlite.close();process.exit(0);}
process.on('SIGTERM',()=>void stop());process.on('SIGINT',()=>void stop());
