import assert from 'node:assert/strict';
import {createHash,createHmac,randomBytes} from 'node:crypto';
import {execFileSync,spawn} from 'node:child_process';
import {createServer} from 'node:https';
import {createRequire} from 'node:module';
import {readFile,writeFile,mkdir,open,unlink} from 'node:fs/promises';
import {existsSync} from 'node:fs';
import {DatabaseSync} from 'node:sqlite';

process.umask(0o077);
const root=process.env.GRENGIN_BACKEND_ROOT,state=process.env.GRENGIN_GOOGLE_STATE;
const worker=process.env.GRENGIN_GOOGLE_WORKER_ROOT;
const origin='https://localhost:8788',api='http://127.0.0.1:8082';
const browserCallback='http://localhost:5173/__local/google-broker/callback';
const frontend='http://localhost:5173';
const keys=['gmail','google-calendar','google-drive'];
const names={gmail:'Gmail','google-calendar':'Google Calendar','google-drive':'Google Drive'};
const runtime=execFileSync('ss',['-ltnpH','sport = :8080'],{encoding:'utf8'}).match(/pid=(\d+)/)?.[1];
assert(runtime,'Existing isolated local API must be running on 8080');
const sourceEnv=Object.fromEntries((await readFile(`/proc/${runtime}/environ`,'utf8')).split('\0').filter(Boolean)
  .map(value=>{const i=value.indexOf('=');return[value.slice(0,i),value.slice(i+1)];}));
const source=new URL(sourceEnv.DATABASE_URL);
assert.equal(source.hostname,'127.0.0.1');
assert.equal(source.pathname,'/grengin_slack_real','Refusing to modify or copy an unrecognized database');
const database=new URL(source);database.pathname='/grengin_google_local';
function docker(args,options={}){return execFileSync('docker',['exec',...args],{encoding:'utf8',stdio:['ignore','pipe','pipe'],...options});}
const present=docker(['grengin-slack-consent','psql','-XAt','-U','local','-d','postgres','-c',
  "SELECT 1 FROM pg_database WHERE datname='grengin_google_local'"]).trim();
if(!present){
  const dump=await open(`${state}/seed.dump`,'w',0o600);
  try{docker(['grengin-slack-consent','pg_dump','-U','local','-d','grengin_slack_real','-Fc','--no-owner','--no-privileges'],
    {stdio:['ignore',dump.fd,'pipe']});}finally{await dump.close();}
  docker(['grengin-slack-consent','createdb','-U','local','grengin_google_local']);
  const input=await open(`${state}/seed.dump`,'r');
  try{docker(['-i','grengin-slack-consent','pg_restore','-U','local','-d','grengin_google_local','--no-owner','--no-privileges','--exit-on-error'],
    {stdio:[input.fd,'pipe','pipe']});}finally{await input.close();await unlink(`${state}/seed.dump`);}
  console.log('Created Google-only test database from the existing isolated copy; source databases unchanged.');
}
const require=createRequire(`${worker}/workers/package.json`);
const esbuild=createRequire(require.resolve('tsx/package.json'))('esbuild');
const brokerSource=await readFile(`${worker}/workers/src/services/connector-broker.ts`,'utf8');
const statement='const callback = `${secureUrl(env.SERVICE_BASE_URL).origin}/v1/connectors/callback/${key}`;';
assert.equal(brokerSource.split(statement).length,2,'Review the local-only callback shim after broker changes');
const built=await esbuild.build({stdin:{contents:brokerSource.replace(statement,`const callback = ${JSON.stringify(browserCallback)};`),
  resolveDir:`${worker}/workers/src/services`,sourcefile:'google-local-broker.ts',loader:'ts'},
  bundle:true,write:false,format:'esm',platform:'node',target:'es2022',logLevel:'silent'});
const {handleConnectorBroker}=await import(`data:text/javascript;base64,${Buffer.from(built.outputFiles[0].contents).toString('base64')}`);
const {createDatabase}=await import(`${worker}/workers/test/helpers/sqlite-d1.mjs`);
// Reuse the real migration helper, but retain encrypted broker rows across restarts.
const seed=createDatabase();
const sqlite=new DatabaseSync(`${state}/broker.sqlite`);
if(!sqlite.prepare("SELECT 1 FROM sqlite_master WHERE type='table' AND name='open_connector_connections'").get()){
  for(const row of seed.sqlite.prepare("SELECT sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%' ORDER BY type='index'").all()){
    sqlite.exec(row.sql);
  }
}
seed.sqlite.close();sqlite.exec('PRAGMA foreign_keys=ON');
const db={withSession(mode){assert.equal(mode,'first-primary');return db;},
  async batch(statements){sqlite.exec('BEGIN IMMEDIATE');try{const result=statements.map(s=>s.execute());sqlite.exec('COMMIT');return result;}
    catch(error){sqlite.exec('ROLLBACK');throw error;}},
  prepare(sql){const statement=sqlite.prepare(sql);const bound=(args=[])=>({bind:(...values)=>bound(values),
    first:async()=>statement.get(...args)??null,all:async()=>({results:statement.all(...args),success:true}),
    execute:()=>({success:true,meta:{changes:Number(statement.run(...args).changes)}}),
    run:async()=>({success:true,meta:{changes:Number(statement.run(...args).changes)}})});return bound();}};
const catalogs=new Map();
for(const key of keys){
  const response=await fetch(`https://meta.grengin.com/connectors/${key}/versions/1.0.0.json`,{redirect:'error',signal:AbortSignal.timeout(15000)});
  assert(response.ok,`${key} catalog unavailable`);catalogs.set(key,await response.json());
}
let encryption;
try{encryption=(await readFile(`${state}/token-key`,'utf8')).trim();}
catch(error){if(error.code!=='ENOENT')throw error;encryption=randomBytes(32).toString('hex');await writeFile(`${state}/token-key`,encryption,{mode:0o600});}
const first=catalogs.get('gmail').connection.auth;
const env={DB:db,SERVICE_BASE_URL:origin,CONNECTOR_BROKER_ENABLED:'true',CONNECTOR_TOKEN_KEY_VERSION:'1',
  CONNECTOR_TOKEN_KEYS:JSON.stringify({'1':encryption}),CONNECTOR_OAUTH_CLIENTS:JSON.stringify({google:{
    client_id:process.env.GOOGLE_CLIENT_ID,client_secret:process.env.GOOGLE_CLIENT_SECRET,
    auth_url:first.auth_url,token_url:first.token_url,revocation_url:'https://oauth2.googleapis.com/revoke',
    mcp_origins:[...new Set([...catalogs.values()].map(value=>new URL(value.connection.url).origin))],
    scopes:[...new Set([...catalogs.values()].flatMap(value=>value.connection.auth.scopes))],
    extra_params:{access_type:'offline',prompt:'consent'}}})};
let backend,closing=false;
function reply(res,status,text,type='text/plain; charset=utf-8'){
  res.writeHead(status,{'Content-Type':type,'Cache-Control':'no-store','Referrer-Policy':'no-referrer','X-Content-Type-Options':'nosniff'});res.end(text);
}
const server=createServer({key:await readFile(`${state}/tls.key`),cert:await readFile(`${state}/tls.crt`)},async(req,res)=>{
  try{
    if(!['localhost:8788','127.0.0.1:8788'].includes(req.headers.host)){reply(res,403,'Unexpected local host');return;}
    let url=new URL(req.url,origin);
    if(req.method==='GET'&&url.pathname==='/__local/google-broker/health'){reply(res,200,'Local Google broker ready');return;}
    if(url.pathname==='/__local/google-broker/callback'){
      if(req.method!=='GET'){reply(res,405,'Method not allowed');return;}
      const stateValue=url.searchParams.get('state')||'';
      if(!/^c2_[a-f0-9]{64}$/.test(stateValue)){reply(res,400,'Invalid Google callback state');return;}
      const row=sqlite.prepare('SELECT connector_key FROM open_connector_connections WHERE state_hash=?').get(createHash('sha256').update(stateValue).digest('hex'));
      if(!row||!keys.includes(row.connector_key)){reply(res,409,'This Google connection expired or was already used. Start again.');return;}
      url=new URL(`/v1/connectors/callback/${row.connector_key}${url.search}`,origin);
    }
    if(!url.pathname.startsWith('/v1/connectors/')){reply(res,404,'Not found');return;}
    let body='';for await(const chunk of req){body+=chunk;if(Buffer.byteLength(body)>16384){reply(res,413,'Request too large');return;}}
    const response=await handleConnectorBroker(new Request(url,{method:req.method,headers:req.headers,
      ...(req.method==='GET'?{}:{body})}),env);
    console.log('Local Google broker result',{method:req.method,operation:url.pathname.split('/').at(-1),status:response.status});
    res.writeHead(response.status,Object.fromEntries(response.headers));res.end(Buffer.from(await response.arrayBuffer()));
  }catch{console.error('Local Google broker request failed; details redacted');reply(res,503,'Local Google broker unavailable');}
});
server.requestTimeout=20000;server.headersTimeout=10000;
async function stop(){
  if(closing)return;closing=true;backend?.kill('SIGTERM');server.closeAllConnections();
  await new Promise(resolve=>server.close(resolve));sqlite.close();await unlink(`${state}/runner.pid`).catch(()=>{});
  process.exit(0);
}
process.on('SIGTERM',()=>void stop());process.on('SIGINT',()=>void stop());
await new Promise((resolve,reject)=>{server.once('error',reject);server.listen(8788,'127.0.0.1',resolve);});
await mkdir(`${state}/files`,{recursive:true,mode:0o700});
await writeFile(`${state}/trust.pem`,Buffer.concat([await readFile('/etc/ssl/certs/ca-certificates.crt'),await readFile(`${state}/tls.crt`)]),{mode:0o600});
const log=await open(`${state}/backend.log`,'a',0o600);
backend=spawn(`${root}/target-security/debug/grengin-api`,[],{cwd:root,env:{...sourceEnv,
  GOOGLE_CLIENT_ID:process.env.GOOGLE_CLIENT_ID,GOOGLE_CLIENT_SECRET:process.env.GOOGLE_CLIENT_SECRET,
  DATABASE_URL:database.href,HOST:'127.0.0.1',PORT:'8082',GRENGIN_SERVICE_URL:origin,
  GRENGIN_CONNECTOR_INSTALLATION_ID:'',GRENGIN_CONNECTOR_PRIVATE_KEY:'',
  REDIRECT_URL:frontend,FILE_STORAGE_ROOT:`${state}/files`,SSL_CERT_FILE:`${state}/trust.pem`,RAG_ENABLED:'false'},
  stdio:['ignore',log.fd,log.fd]});
await log.close();
for(let n=0;n<120;n++){
  if(backend.exitCode!==null)throw new Error('Google local backend exited; inspect its private log');
  try{const response=await fetch(api+'/',{signal:AbortSignal.timeout(1000)});if(response.ok)break;}
  catch{}if(n===119)throw new Error('Google local backend readiness timeout');await new Promise(r=>setTimeout(r,500));
}
const pg={...process.env,PGHOST:database.hostname,PGPORT:database.port,PGUSER:decodeURIComponent(database.username),
  PGPASSWORD:decodeURIComponent(database.password),PGDATABASE:database.pathname.slice(1)};
const admin=JSON.parse(execFileSync('psql',['-XAt','-c',`SELECT row_to_json(u) FROM users u JOIN user_role_assignments a ON a."userId"=u.id
  JOIN roles r ON r.id=a."roleId" WHERE u.status='active' AND r.name='Super Admin' ORDER BY u."createdAt" LIMIT 1`],
  {env:pg,encoding:'utf8',stdio:['ignore','pipe','pipe']}).trim());
assert(admin?.id,'Test database needs an active existing Super Admin');
const payload=[{alg:'HS256',typ:'JWT'},{user_id:admin.id,sub:admin.email,name:admin.name,token_use:'access',exp:Math.floor(Date.now()/1000)+3600}]
  .map(value=>Buffer.from(JSON.stringify(value)).toString('base64url')).join('.');
const token=`${payload}.${createHmac('sha256',sourceEnv.JWT_SECRET).update(payload).digest('base64url')}`;
async function call(path,body){
  const response=await fetch(api+path,{method:body?'POST':'GET',headers:{Authorization:`Bearer ${token}`,'Content-Type':'application/json'},
    ...(body?{body:JSON.stringify(body)}:{}),signal:AbortSignal.timeout(20000)});
  assert(response.ok,`Local API ${path} failed with HTTP ${response.status}`);return response.json();
}
const existing=await call('/admin/mcp-servers');
for(const key of keys){
  const catalog=catalogs.get(key),auth=catalog.connection.auth;
  if(existing.servers.some(server=>server.url===catalog.connection.url&&server.connection_config?.grengin_connector_key===key))continue;
  await call('/admin/mcp-servers',{name:`${names[key]} (Grengin broker)`,description:names[key],
    icon:`https://meta.grengin.com/connectors/icons/${key}.svg`,transport_type:'http',url:catalog.connection.url,
    enabled:true,default_access:'all_users',client_id:'broker-managed',connection_config:{grengin_connector_key:key,
      oauth:{auth_url:auth.auth_url,token_url:auth.token_url,scopes:auth.scopes}}});
}
await writeFile(`${state}/runner.pid`,String(process.pid),{mode:0o600});
console.log('READY: Google connectors at http://localhost:5173/settings?tab=integrations');
console.log(`Google callback: ${browserCallback}; local API 8082, local broker 8788, isolated grengin_google_local database.`);
console.log('No staging configuration or original database was changed. Google consent must be completed in the browser.');
backend.once('exit',code=>{if(!closing){console.error('Google local backend exited',{code});void stop();}});
