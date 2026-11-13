#!/usr/bin/env bash
set -euo pipefail
set +x
umask 077
cd "$(dirname "$0")/../.."
set -a
source /home/anurag/work/secrets/grengin.sh
source .grengin_auth_token
set +a
node <<'JS'
const assert=require('node:assert/strict');const fs=require('node:fs');const {createHmac}=require('node:crypto');
const config=JSON.parse(fs.readFileSync(process.env.HOME+'/.local/state/grengin/slack-consent/session.json','utf8'));
const body=[{alg:'HS256',typ:'JWT'},{user_id:config.realUser,sub:'local-development-account',token_use:'access',exp:Math.floor(Date.now()/1000)+600}]
 .map(v=>Buffer.from(JSON.stringify(v)).toString('base64url')).join('.');
const token=body+'.'+createHmac('sha256',process.env.JWT_SECRET).update(body).digest('base64url');
async function call(method,path,body){const response=await fetch('http://127.0.0.1:8080'+path,{method,headers:{Authorization:'Bearer '+token,'Content-Type':'application/json'},
 ...(body===undefined?{}:{body:JSON.stringify(body)}),signal:AbortSignal.timeout(30000)});assert(response.ok,method+' '+path+' HTTP '+response.status);return response.json();}
(async()=>{
 const me=await call('GET','/admin/users/'+config.realUser);assert.equal(me.id,config.realUser);assert.notEqual(me.email,'slack-local@example.invalid');
 console.log('PASS: authenticated existing account from the development database copy.');
 await call('PUT','/admin/mcp-servers/'+config.realServerId,{name:'Slack (Grengin broker)'});
 const servers=await call('GET','/mcp-servers');assert(servers.servers.some(s=>s.id===config.realServerId));
 console.log('PASS: real MCP listing includes Slack (Grengin broker).');
 const models=await call('GET','/models');
 const toolModels=models.providers.flatMap(provider=>provider.models).filter(model=>model.supports_tools);
 assert(toolModels.length>0,'No tool-capable models available');
 console.log('PASS: model listing includes '+toolModels.length+' tool-capable models.');
 const started=await call('POST','/connectors/slack/connect',{version:'1.0.0',server_id:config.realServerId});
 assert.equal(started.status,'pending');const url=new URL(started.authorization_url);
 assert.equal(url.origin+url.pathname,'https://slack.com/oauth/v2_user/authorize');
 assert.equal(url.searchParams.get('redirect_uri'),'http://localhost:5173/mcp/oauth/callback');
 assert.equal(url.searchParams.get('client_id'),process.env.SLACK_CLIENT_ID);
 assert.equal(url.searchParams.get('code_challenge_method'),'S256');
 assert.equal((await call('GET','/connectors/'+started.id)).status,'pending');
 console.log('PASS: actual account starts a signed broker connection with the saved Slack client and registered webapp callback.');
 const callback=await fetch('http://localhost:5173/mcp/oauth/callback?state=local-routing-check');
 assert(callback.ok);assert((await callback.text()).includes('/src/main.ts'));
 console.log('PASS: Slack callback renders the real webapp instead of the test page.');
 const broker=await fetch('http://localhost:5173/__local/connector-broker/callback/slack?state=invalid');assert.equal(broker.status,400);
 console.log('PASS: dedicated local callback relay reaches the broker and rejects invalid state.');
 const proxy=await fetch('http://localhost:5173/api/');assert(proxy.ok);assert.equal((await proxy.json()).expected_migration_head,'m20261028_000001_create_connectors');
 console.log('PASS: real frontend API proxy reaches the feature backend. Real Slack approval/tool calls remain user-driven.');
})().catch(error=>{console.error(error.message);process.exitCode=1;});
JS
