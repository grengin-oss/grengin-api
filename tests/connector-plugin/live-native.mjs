import assert from 'node:assert/strict';
import {createDecipheriv,createHmac,randomBytes} from 'node:crypto';
import {execFileSync,spawn} from 'node:child_process';
import {once} from 'node:events';
import {createServer} from 'node:http';
import {mkdir,readFile,writeFile,open,chmod,rename} from 'node:fs/promises';
import {resolve,join,sep} from 'node:path';
import {callbackFor,callbackPaths,microsoftConnectorIds} from './live-callbacks.mjs';
import {runLiveChats} from './live-chat.mjs';
import {runLiveUi} from './live-ui.mjs';

process.umask(0o077);
const root=process.env.GRENGIN_BACKEND_ROOT;
const directory=join(process.env.HOME,'.local/state/grengin/native-plugin-live');
const webapp=process.env.GRENGIN_NATIVE_WEBAPP ?? '/home/anurag/work/fresh/webapp';
const master=resolve(root,'../grengin-list/master-data');
const api='http://127.0.0.1:8083';
const container='grengin-native-plugin-live-db';
await mkdir(directory,{recursive:true,mode:0o700});await chmod(directory,0o700);
const command=process.argv[2]??'status';
const sleep=ms=>new Promise(resolve=>setTimeout(resolve,ms));
const exec=(name,args,options={})=>execFileSync(name,args,{encoding:'utf8',stdio:['ignore','pipe','pipe'],...options});
const quote=value=>`'${String(value).replaceAll("'","''")}'`;
const credentialPrefix=key=>key==='jira'?'JIRA':key==='github'?(process.env.CONNECTOR_GITHUB_CLIENT_ID?'CONNECTOR_GITHUB':'GITHUB'):key==='slack'?'SLACK':microsoftConnectorIds.includes(key)?(process.env.CONNECTOR_MICROSOFT_CLIENT_ID?'CONNECTOR_MICROSOFT':process.env.MICROSOFT_CLIENT_ID?'MICROSOFT':'AZURE'):'GOOGLE';
const processEnv=async pid=>Object.fromEntries((await readFile(`/proc/${pid}/environ`,'utf8')).split('\0').filter(Boolean).map(line=>{const at=line.indexOf('=');return[line.slice(0,at),line.slice(at+1)];}));
const listenerPid=port=>Number(exec('ss',['-ltnpH',`sport = :${port}`]).match(/pid=(\d+)/)?.[1]??0);
let saved;
try{saved=JSON.parse(await readFile(join(directory,'session.json'),'utf8'));}catch(error){if(error.code!=='ENOENT')throw error;}
const save=async()=>{
  const temporary=join(directory,`session.${process.pid}.${randomBytes(6).toString('hex')}.tmp`);
  await writeFile(temporary,JSON.stringify(saved),{mode:0o600});await rename(temporary,join(directory,'session.json'));
};
function unseal(stored){
  const bytes=Buffer.from(stored.startsWith('enc:v1:')?stored.slice(7):stored,'base64');
  const decipher=createDecipheriv('aes-256-gcm',Buffer.from(saved.key,'base64'),bytes.subarray(0,12));
  decipher.setAuthTag(bytes.subarray(-16));
  return Buffer.concat([decipher.update(bytes.subarray(12,-16)),decipher.final()]).toString();
}
function pg(sql,url){
  const u=new URL(url);assert(['localhost','127.0.0.1'].includes(u.hostname));
  return exec('psql',['-XAt','-v','ON_ERROR_STOP=1','-c',sql],{env:{...process.env,PGHOST:u.hostname,PGPORT:u.port||'5432',PGUSER:decodeURIComponent(u.username),PGPASSWORD:decodeURIComponent(u.password),PGDATABASE:u.pathname.slice(1)}}).trim();
}
function bearer(){
  const body=[{alg:'HS256',typ:'JWT'},{user_id:saved.owner.id,sub:saved.owner.email,name:saved.owner.name,token_use:'access',exp:Math.floor(Date.now()/1000)+600}].map(x=>Buffer.from(JSON.stringify(x)).toString('base64url')).join('.');
  return `${body}.${createHmac('sha256',saved.jwt).update(body).digest('base64url')}`;
}
async function jsonResponse(response){
  let length=0;const chunks=[];
  for await(const chunk of response.body){length+=chunk.length;assert(length<=1024*1024,'Live test response exceeded limit');chunks.push(chunk);}
  return JSON.parse(Buffer.concat(chunks).toString());
}
async function request(method,path,body){
  const response=await fetch(api+path,{method,redirect:'manual',headers:{Authorization:`Bearer ${bearer()}`,'Content-Type':'application/json'},...(body===undefined?{}:{body:JSON.stringify(body)}),signal:AbortSignal.timeout(20000)});
  assert(response.ok,`${method} ${path.split('?')[0]} returned HTTP ${response.status}`);
  if(response.status===204)return undefined;
  return jsonResponse(response);
}
async function frontend(startNative){
  const pid=listenerPid(5173);
  if(pid){
    const cwd=await readFile(`/proc/${pid}/cmdline`,'utf8');
    assert(cwd.includes(webapp+'/node_modules/.bin/vite'),'Refusing to stop an unrelated frontend');
    if(startNative&&!saved.frontendPrevious){saved.frontendPrevious={argv:cwd.split('\0').filter(Boolean),env:await processEnv(pid)};await save();}
    process.kill(pid,'SIGTERM');
    for(let n=0;n<50&&listenerPid(5173);n++)await sleep(100);
    assert(!listenerPid(5173),'Frontend port is still busy');
  }
  const old=saved.frontendPrevious;assert(old,'Previous frontend launch was not saved');
  const env={...old.env,...(startNative?{GRENGIN_LOCAL_NATIVE_PLUGINS:'1',GRENGIN_LOCAL_SLACK_BROKER:'0',VITE_LOCAL_GOOGLE_CONNECTORS:'0',VITE_STAGING_CONNECTORS:'0',VITE_STAGING_GOOGLE_CONNECTORS:'0',GRENGIN_DEV_API_TARGET:api,VITE_API_BASE:'/api'}:{})};
  const log=await open(join(directory,startNative?'webapp.log':'webapp-restored.log'),'a',0o600);
  try{
    const child=spawn(old.argv[0],old.argv.slice(1),{cwd:webapp,env,detached:true,stdio:['ignore',log.fd,log.fd]});
    child.unref();saved.frontendPid=child.pid;await save();
  }finally{await log.close();}
  for(let n=0;n<100;n++){try{const r=await fetch('http://localhost:5173/',{signal:AbortSignal.timeout(1000)});if(r.ok)return;}catch{}await sleep(100);}
  throw new Error('Frontend readiness timed out; private log is in native-plugin-live');
}
async function installConnectors(ids){
  assert(ids.length&&ids.every(id=>Object.hasOwn(callbackPaths,id)),'Choose known live-test connectors');
  assert.equal(new URL(saved.database).pathname,'/grengin_native_plugins_live','Only the isolated test DB may be configured');
  const initial=(await request('GET','/admin/mcp-servers')).servers;
  for(const id of ids){
    const manifest=JSON.parse(await readFile(join(master,'connector-plugins',id,'versions/1.0.0/plugin.json'),'utf8'));
    const setup=JSON.parse(await readFile(join(master,'connector-plugins',id,'versions/1.0.0/setup.json'),'utf8'));
    const prefix=credentialPrefix(id);
    const client=process.env[`${prefix}_CLIENT_ID`],secret=process.env[`${prefix}_CLIENT_SECRET`];
    assert(client&&secret&&client!==secret,`Missing or invalid ${prefix} OAuth credentials`);
    const name=`${manifest.name} (native plugin)`;
    const current=initial.find(server=>server.id===saved.servers[id]||server.name===name);
    if(current){
      assert.equal(current.connection_config?.connector_plugin?.manifest?.id,id,'Existing test server has a different plugin identity');
      saved.servers[id]=current.id;await save();
      console.log(`${id}: existing native install reused without changing configuration or tokens`);continue;
    }
    const oauth={...setup.authentication.oauth,redirect_url:callbackFor(id)};
    const server=await request('POST','/admin/mcp-servers',{name,description:`Local read-only ${manifest.name} plugin test`,icon:`https://meta.grengin.com/connectors/icons/${id}.svg`,transport_type:'http',url:manifest.base_url,enabled:true,default_access:'all_users',client_id:client,client_secret:secret,connection_config:{connector_plugin:{manifest,configuration:id==='jira'?{cloud_id:'00000000-0000-4000-8000-000000000000'}:{}},oauth}});
    saved.servers[id]=server.id;await save();
    console.log(`${id}: installed in isolated DB; ${manifest.tools.length} read-only tools; browser consent required`);
  }
}
if(command==='frontend'||command==='restore-webapp'){
  assert(saved,'Start the isolated API first');await frontend(command==='frontend');
  console.log(command==='frontend'?'Local webapp now targets isolated native API 8083.':'Previous webapp process/environment restored; native database retained.');
}else if(command==='status'||command==='check'||command==='probe'||command==='configure'||command==='chat'||command==='install'||command==='ui'||command==='diagnose'){
  assert(saved,'Native plugin test setup has not been started');
  const health=await(await fetch(api+'/',{signal:AbortSignal.timeout(3000)})).json();
  console.log(`Isolated backend ${health.version}, ${health.status}; frontend http://localhost:5173/settings?tab=integrations`);
  const connections=(await request('GET','/mcp/connections')).connections;
  const selected=new Set(process.argv.slice(3));
  if(command!=='install')assert([...selected].every(key=>Object.hasOwn(saved.servers,key)),'Unknown connector selection');
  for(const [key,id]of Object.entries(saved.servers??{})){
    const connection=connections.find(row=>row.server_id===id);
    console.log(`${key}: ${connection?.connected?'connected':'awaiting browser consent'}`);
  }
  if(command==='install')await installConnectors([...selected]);
  if(command==='chat')await runLiveChats({root,saved,request,pg,api,bearer,selected});
  if(command==='ui')await runLiveUi({saved,bearer,request,selected});
  if(command==='diagnose'){
    const paths={'outlook-mail':'/me/messages?$top=1','outlook-calendar':'/me/calendars?$top=1',onedrive:'/me/drive',sharepoint:'/sites/root','microsoft-teams':'/me/joinedTeams','microsoft-work-iq':'/me'};
    assert(selected.size&&[...selected].every(id=>Object.hasOwn(paths,id)),'Select Microsoft connectors for redacted Graph diagnostics');
    for(const key of selected){
      const id=saved.servers[key];
      const row=JSON.parse(pg(`SELECT row_to_json(c) FROM mcp_connections c WHERE "serverId"=${quote(id)} AND "userId"=${quote(saved.owner.id)}`,saved.database)||'null');
      if(!row?.accessToken){console.log(`${key}: awaiting browser consent`);continue;}
      const response=await fetch('https://graph.microsoft.com/v1.0'+paths[key],{redirect:'manual',headers:{Authorization:`Bearer ${unseal(row.accessToken)}`,Accept:'application/json'},signal:AbortSignal.timeout(15000)});
      const body=await jsonResponse(response);
      const code=typeof body.error?.code==='string'&&/^[a-zA-Z0-9_.-]{1,80}$/.test(body.error.code)?body.error.code:'unclassified';
      const message=String(body.error?.message??'');
      const categories={sharepoint_license:/SPO license|sharepoint.*licen[sc]e|licen[sc]e.*sharepoint/i.test(message),mailbox:/mailbox/i.test(message),consent:/permission|consent|insufficient.*privilege/i.test(message),expired_token:/expired|lifetime validation/i.test(message),missing_resource:/not found|does not exist|not provisioned/i.test(message)};
      console.log(`${key}: fixed Graph read HTTP ${response.status}${response.ok?'':`; code=${code}; diagnostic categories=${JSON.stringify(categories)}`}; response contents withheld`);
    }
  }
  if(command==='configure'){
    saved.blockedProviders??={};
    for(const [key,id]of Object.entries(saved.servers)){
      if(selected.size&&!selected.has(key))continue;
      const prefix=credentialPrefix(key);
      const client=process.env[`${prefix}_CLIENT_ID`],secret=process.env[`${prefix}_CLIENT_SECRET`];
      if(!client||!secret||client===secret){
        await request('PUT',`/admin/mcp-servers/${id}`,{enabled:false});saved.blockedProviders[key]=true;
        console.log(`${key}: test server disabled until distinct client credentials are saved`);continue;
      }
      const current=JSON.parse(pg(`SELECT json_build_object('client_id',"clientId",'client_secret',"clientSecret") FROM mcp_servers WHERE id=${quote(id)}`,saved.database));
      const server=(await request('GET','/admin/mcp-servers')).servers.find(server=>server.id===id);
      const callbackChanged=server.connection_config.oauth.redirect_url!==callbackFor(key);
      const configuration=structuredClone(server.connection_config);
      configuration.oauth.redirect_url=callbackFor(key);
      const update={...(current.client_id!==client?{client_id:client}:{}),...(!current.client_secret||unseal(current.client_secret)!==secret?{client_secret:secret}:{}),...(callbackChanged?{connection_config:configuration}:{}),...(saved.blockedProviders[key]?{enabled:true}:{})};
      if(Object.keys(update).length){
        await request('PUT',`/admin/mcp-servers/${id}`,update);
        console.log(`${key}: changed app credentials/callback applied to isolated server only`);
      }else console.log(`${key}: app credentials unchanged; no update or session reset`);
      delete saved.blockedProviders[key];
    }
    await save();
  }
  if(command==='check'){
    assert.equal(health.version,'0.9.8');
    assert.equal((await(await fetch('http://localhost:5173/api/')).json()).version,'0.9.8');
    assert.equal((await(await fetch('http://127.0.0.1:8080/')).json()).version,'0.9.7');
    for(const [key,id]of Object.entries(saved.servers)){
      if(selected.size&&!selected.has(key))continue;
      const server=(await request('GET','/admin/mcp-servers')).servers.find(row=>row.id===id);
      const expected=callbackFor(key);
      assert.equal(server.connection_config.oauth.redirect_url,expected);
      const tools=await request('GET',`/mcp/tools?server_id=${id}`);
      assert.equal(tools.total,server.enabled?server.connection_config.connector_plugin.manifest.tools.length:0);
      assert(tools.tools.every(tool=>tool.is_read_only));
      if(!server.enabled){console.log(`PASS ${key}: blocked test server exposes no tools`);continue;}
      const pending=await request('POST',`/mcp/connections/${id}/authorize`,{});
      const authorization=new URL(pending.authorization_url);
      try{
        assert.equal(authorization.searchParams.get('redirect_uri'),expected);
        assert.equal(authorization.searchParams.get('response_type'),'code');
        const prefix=credentialPrefix(key);
        assert(!authorization.href.includes(process.env[`${prefix}_CLIENT_SECRET`]));
      }finally{pg(`DELETE FROM mcp_oauth_states WHERE state=${quote(authorization.searchParams.get('state'))} AND "userId"=${quote(saved.owner.id)}`,saved.database);}
      console.log(`PASS ${key}: real authorize API uses registered callback; ${tools.total} read-only tools; probe state removed`);
    }
    for(const [id,path]of Object.entries(callbackPaths).filter(([id])=>id!=='slack'&&(!selected.size||selected.has(id)))){
      const callback=await fetch(`http://localhost:5173${path}?code=fixture&state=fixture`,{redirect:'manual'});
      assert.equal(callback.status,302);assert.equal(callback.headers.get('location'),'/mcp/oauth/callback?code=fixture&state=fixture');
      assert.equal(callback.headers.get('cache-control'),'no-store');
      assert.equal(callback.headers.get('referrer-policy'),'no-referrer');
      assert.equal((await fetch(`http://localhost:5173${path}`,{method:'POST',redirect:'manual'})).status,400);
      console.log(`PASS ${id}: exact registered callback handoff and method/privacy headers`);
    }
    assert.equal((await fetch(api+'/mcp/connections')).status,401);
    console.log('PASS: callback aliases, authenticated routes, native proxy, and preserved old API');
  }
  if(command==='probe'){
    let passed=0,failed=0,pending=0;
    const servers=(await request('GET','/admin/mcp-servers')).servers;
    const targets={github:[['get_me',{}],['list_repositories',{}],['get_repository',{owner:'grengin-oss',repo:'grengin-api'}],['list_branches',{owner:'grengin-oss',repo:'grengin-api'}],['list_commits',{owner:'grengin-oss',repo:'grengin-api'}]],slack:[['get_team',{}],['list_conversations',{}],['list_users',{}]],gmail:[['list_messages',{}],['list_labels',{}]],'google-calendar':[['list_calendars',{}],['list_events',{}]],'google-drive':[['list_files',{}],['list_recent_files',{}]],jira:[['get_me',{}],['list_projects',{}]],'outlook-mail':[['list_messages',{}],['list_sent_messages',{}]],'outlook-calendar':[['list_calendars',{}],['list_events',{start_time:new Date().toISOString(),end_time:new Date(Date.now()+7*86400000).toISOString()}]],onedrive:[['get_drive',{}],['list_root_files',{}]],sharepoint:[['get_root_site',{}],['search_sites',{query:'*'}]],'microsoft-teams':[['list_joined_teams',{}],['list_chats',{}]],'microsoft-work-iq':[['get_me',{}],['search_files',{query:'grengin',limit:1,offset:0}]]};
    for(const [key,id]of Object.entries(saved.servers)){
      if(selected.size&&!selected.has(key))continue;
      const connection=connections.find(row=>row.server_id===id);
      if(!connection?.connected){pending++;continue;}
      const row=JSON.parse(pg(`SELECT row_to_json(c) FROM mcp_connections c WHERE "serverId"=${quote(id)} AND "userId"=${quote(saved.owner.id)}`,saved.database));
      assert(row.accessToken?.startsWith('enc:v1:'),'Only encrypted per-user tokens may be used');
      if(row.expiresAt&&new Date(row.expiresAt)<=new Date()){failed++;console.log(`${key}: BLOCKED token expired; refresh/reconnect through backend first`);continue;}
      const access=unseal(row.accessToken);
      const server=servers.find(row=>row.id===id);
      if(key==='jira'){
        const response=await fetch('https://api.atlassian.com/oauth/token/accessible-resources',{redirect:'manual',headers:{Authorization:`Bearer ${access}`,Accept:'application/json'},signal:AbortSignal.timeout(15000)});
        if(!response.ok){console.log(`jira: BLOCKED accessible resources HTTP ${response.status}`);continue;}
        const resources=(await jsonResponse(response)).filter(site=>site.scopes?.includes('read:jira-work'));
        const unique=[...new Map(resources.map(site=>[site.id,site])).values()];
        if(unique.length!==1){console.log(`jira: select one of ${unique.length} accessible Jira sites before tool testing`);continue;}
        server.connection_config.connector_plugin.configuration.cloud_id=unique[0].id;
        await request('PUT',`/admin/mcp-servers/${id}`,{connection_config:server.connection_config});
        console.log('jira: selected the single authorized resource without exposing site data');
      }
      for(const [name,original]of targets[key]){
        const args={...original};
        const schema=server.connection_config.connector_plugin.manifest.tools.find(tool=>tool.name===name).input_schema;
        for(const parameter of ['limit','per_page','page_size','max_results'])if(schema.properties?.[parameter])args[parameter]=1;
        let result;
        try{
          result=JSON.parse(exec(join(root,'target/debug/examples/live_probe'),[],{stdio:['pipe','pipe','pipe'],input:JSON.stringify({config:server.connection_config.connector_plugin,tool:name,arguments:args,access_token:access,user_id:saved.owner.id}),timeout:35000,maxBuffer:65536}));
        }catch(error){try{result=JSON.parse(error.stdout);}catch{result={success:false,error:'probe execution failed; details redacted'};}}
        if(result.success)passed++;else failed++;
        console.log(`${result.success?'PASS':'CHECK'} ${key}/${name}: ${result.success?'live provider read succeeded':result.error}; response contents withheld`);
      }
    }
    console.log(`Live reads: ${passed} passed; ${failed} failed; ${pending} connectors await consent. Actual Rust runtime and user tokens; no paid LLM calls; not a chat-model test.`);
    if(failed||pending)process.exitCode=1;
  }
}else if(command==='serve'){
  assert(!listenerPid(8083),'Port 8083 is already in use');
  if(!saved){
    const sourcePid=listenerPid(8080);assert(sourcePid,'Existing isolated test API is required to identify the test owner');
    const sourceEnv=await processEnv(sourcePid);const source=new URL(sourceEnv.DATABASE_URL);
    assert.equal(source.pathname,'/grengin_slack_real','Refusing to use an unrecognized source DB');
    const owner=JSON.parse(pg(`SELECT json_build_object('id',u.id,'email',u.email,'name',u.name,'google_id',u."googleId",'azure_id',u."azureId",'identities',u.identities) FROM users u JOIN user_role_assignments a ON a."userId"=u.id JOIN roles r ON r.id=a."roleId" WHERE u.status='active' AND r.name='Super Admin' AND a."scopeDepartmentId" IS NULL ORDER BY u."createdAt" LIMIT 1`,source.href)||'null');
    assert(owner?.id,'Existing test DB needs an active organization-wide admin');
    saved={owner,password:randomBytes(24).toString('hex'),key:randomBytes(32).toString('base64'),jwt:randomBytes(32).toString('hex'),servers:{}};await save();
  }
  let existing=false;
  try{assert.equal(exec('docker',['inspect','--format','{{index .Config.Labels "com.grengin.test"}}',container]).trim(),'native-plugins');existing=true;}catch(error){if(String(error.message).includes('Refusing'))throw error;}
  if(!existing)exec('docker',['run','-d','--name',container,'--label','com.grengin.test=native-plugins','--memory=384m','--cpus=1','-p','127.0.0.1::5432','-e','POSTGRES_USER=local','-e','POSTGRES_PASSWORD','-e','POSTGRES_DB=grengin_native_plugins_live','pgvector/pgvector:pg16','-c','shared_buffers=32MB','-c','max_connections=30'],{env:{...process.env,POSTGRES_PASSWORD:saved.password}});
  else exec('docker',['start',container]);
  for(let n=0;n<60;n++){try{exec('docker',['exec',container,'pg_isready','-U','local','-d','grengin_native_plugins_live']);break;}catch{}await sleep(250);}
  const port=exec('docker',['port',container,'5432/tcp']).trim().split(':').at(-1);
  saved.database=`postgres://local:${saved.password}@127.0.0.1:${port}/grengin_native_plugins_live`;await save();
  for(let n=0;n<80;n++){
    try{assert.equal(pg('SELECT 1',saved.database),'1');break;}
    catch{assert(n<79,'Isolated PostgreSQL TCP readiness timed out');await sleep(250);}
  }
  const metadata=createServer(async(req,res)=>{
    try{
      const url=new URL(req.url,'http://localhost');assert.equal(req.method,'GET');
      assert(/^[a-zA-Z0-9._/-]+$/.test(url.pathname));
      let bytes;
      if(url.pathname==='/distributions/grengin-api/0.9.8/index.json'){
        const draft=JSON.parse(await readFile(resolve(root,'../grengin-list/docs/drafts/grengin-api/0.9.8/index.json'),'utf8'));
        draft.grenginApiCommit='0'.repeat(40);draft.catalogCommit='0'.repeat(40);bytes=Buffer.from(JSON.stringify(draft));
      }else{
        const file=resolve(master,'.'+url.pathname);assert(file.startsWith(master+sep)&&file.endsWith('.json'));
        bytes=await readFile(file);
      }
      assert(bytes.length<=1048576);res.writeHead(200,{'Content-Type':'application/json','Cache-Control':'no-store'});res.end(bytes);
    }catch{res.writeHead(404);res.end();}
  });
  await new Promise((resolve,reject)=>{metadata.once('error',reject);metadata.listen(18084,'127.0.0.1',resolve);});
  await mkdir(join(directory,'files'),{recursive:true,mode:0o700});
  const log=await open(join(directory,'backend.log'),'a',0o600);
  const env={PATH:process.env.PATH,HOME:process.env.HOME,APP_KEY:saved.key,JWT_SECRET:saved.jwt,DATABASE_URL:saved.database,HOST:'127.0.0.1',PORT:'8083',REDIRECT_URL:'http://localhost:5173',RAG_ENABLED:'false',FILE_STORAGE_ROOT:join(directory,'files'),GRENGIN_METADATA_BASE_URL:'http://127.0.0.1:18084/',GOOGLE_CLIENT_ID:process.env.GOOGLE_CLIENT_ID,GOOGLE_CLIENT_SECRET:process.env.GOOGLE_CLIENT_SECRET};
  const backend=spawn(join(root,'target/debug/grengin-api'),[],{cwd:directory,env,stdio:['ignore',log.fd,log.fd]});await log.close();
  let closing=false;
  async function stop(){if(closing)return;closing=true;backend.kill('SIGTERM');metadata.closeAllConnections();await new Promise(resolve=>metadata.close(resolve));if(backend.exitCode===null)await once(backend,'exit');process.exit(0);}
  process.on('SIGTERM',()=>void stop());process.on('SIGINT',()=>void stop());
  for(let n=0;n<120;n++){
    assert(backend.exitCode===null,'Native backend exited; inspect private backend.log');
    try{const response=await fetch(api+'/',{signal:AbortSignal.timeout(1000)});if(response.ok){assert.equal((await response.json()).version,'0.9.8');break;}}catch{}
    assert(n<119,'Native backend readiness timed out');await sleep(250);
  }
  if(!pg(`SELECT id FROM users WHERE id=${quote(saved.owner.id)}`,saved.database)){
    const o=saved.owner;
    pg(`BEGIN;INSERT INTO users(id,status,email,"emailVerified",name,"googleId","azureId",identities,"mfaEnabled","createdAt","updatedAt","lastLoginAt","isIndependent") VALUES(${quote(o.id)},'active',${quote(o.email)},true,${o.name?quote(o.name):'NULL'},${o.google_id?quote(o.google_id):'NULL'},${o.azure_id?quote(o.azure_id):'NULL'},${o.identities?quote(JSON.stringify(o.identities))+'::jsonb':'NULL'},false,now(),now(),now(),true);INSERT INTO user_role_assignments(id,"userId","roleId","assignedBy","createdAt","updatedAt") SELECT gen_random_uuid(),${quote(o.id)},id,${quote(o.id)},now(),now() FROM roles WHERE name='Super Admin';COMMIT;`,saved.database);
  }
  await installConnectors(['jira','github','slack','gmail','google-calendar','google-drive']);
  console.log('READY: isolated native API 0.9.8 at http://127.0.0.1:8083; six explicitly installed plugins.');
  console.log('Jira cloud_id awaits accessible-resource selection after consent. No existing server or user tokens were copied.');
  backend.once('exit',()=>{if(!closing){console.error('Native backend exited');void stop();}});
}else throw new Error('Usage: live-native.sh serve|frontend|restore-webapp|status|check|probe|configure|chat|install|ui|diagnose [connector IDs]');
