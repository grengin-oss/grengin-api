import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';

const website=process.cwd();
const require=createRequire(`${website}/workers/package.json`);
const {getPublicKeyAsync,signAsync}=await import(pathToFileURL(require.resolve('@noble/ed25519')));
const {createDatabase}=await import(pathToFileURL(`${website}/workers/test/helpers/sqlite-d1.mjs`));
const {handleConnectorBroker}=await import(pathToFileURL(`${website}/workers/src/services/connector-broker.ts`));
const {SLACK_CLIENT_ID:clientId,SLACK_CLIENT_SECRET:clientSecret}=process.env;
assert(clientId && clientSecret,'Slack client settings are required');
const originalFetch=globalThis.fetch;
const encoder=new TextEncoder();
const digest=async bytes=>Buffer.from(await crypto.subtle.digest('SHA-256',bytes)).toString('hex');
let requests=0;
globalThis.fetch=async(url,options)=>{
  assert.equal(options?.redirect,'error');
  assert(/^https:\/\/meta\.grengin\.com\/(connectors\.json|connectors\/slack\/versions\/\d+\.\d+\.\d+\.json)$/.test(String(url)),
    'Preflight must never contact a vendor token endpoint');
  requests++;
  return originalFetch(url,{...options,signal:AbortSignal.timeout(15000)});
};
async function document(url){
  const response=await globalThis.fetch(url,{redirect:'error'});assert.equal(response.status,200);
  const chunks=[];let size=0;
  for await(const chunk of response.body){size+=chunk.byteLength;assert(size<=131072,'Catalog response too large');chunks.push(chunk);}
  const bytes=Buffer.concat(chunks);
  return {value:JSON.parse(bytes.toString('utf8')),hash:await digest(bytes)};
}
const {db,sqlite}=createDatabase();
try{
  const listing=(await document('https://meta.grengin.com/connectors.json')).value;
  const entry=listing.catalog.find(item=>item.key==='slack');assert(entry,'Slack is missing from the published catalog');
  const release=entry.versions.find(item=>item.version===entry.latestVersion);assert(release);
  const expectedUrl=`https://meta.grengin.com/connectors/slack/versions/${entry.latestVersion}.json`;
  assert.equal(release.url,expectedUrl);
  const template=await document(expectedUrl);assert.equal(template.hash,release.sha256);
  const auth=template.value.connection.auth;
  const privateKey=crypto.getRandomValues(new Uint8Array(32));
  const publicKey=Buffer.from(await getPublicKeyAsync(privateKey)).toString('hex');
  const installation='slack_local_preflight';const user=crypto.randomUUID();const connection=crypto.randomUUID();
  const grant=Buffer.from(crypto.getRandomValues(new Uint8Array(32))).toString('hex');
  sqlite.exec("INSERT INTO organizations(id,name,created_at,updated_at) VALUES ('slack_preflight','Local preflight','now','now')");
  sqlite.prepare(`INSERT INTO installations(id,organization_id,name,public_key,environment,is_revoked,created_at,instance_uuid,connection_status,kind)
    VALUES (?,'slack_preflight','Local preflight',?,'prod',0,'now',?,'connected','standalone')`).run(installation,publicKey,user);
  const origin='https://service.grengin.com';
  const env={DB:db,CONNECTOR_BROKER_ENABLED:'true',SERVICE_BASE_URL:origin,
    CONNECTOR_TOKEN_KEYS:JSON.stringify({'1':Buffer.from(crypto.getRandomValues(new Uint8Array(32))).toString('hex')}),
    CONNECTOR_OAUTH_CLIENTS:JSON.stringify({slack:{client_id:clientId,client_secret:clientSecret,
      auth_url:auth.auth_url,token_url:auth.token_url,mcp_origins:['https://mcp.slack.com'],scopes:auth.scopes,
      extra_params:{response_type:'code'}}})};
  async function start(){
    const body=JSON.stringify({user_id:user,connection_grant:grant,connection_id:connection,connector_key:'slack',version:entry.latestVersion});
    const timestamp=String(Math.floor(Date.now()/1000));const nonce=Buffer.from(crypto.getRandomValues(new Uint8Array(32))).toString('hex');
    const proof=`grengin-connectors-v1\n${installation}\n${timestamp}\n${nonce}\nPOST\n/v1/connectors/start\n${await digest(encoder.encode(body))}`;
    const signature=Buffer.from(await signAsync(encoder.encode(proof),privateKey)).toString('hex');
    const response=await handleConnectorBroker(new Request(`${origin}/v1/connectors/start`,{method:'POST',body,headers:{
      'Content-Type':'application/json','X-Grengin-Installation':installation,'X-Grengin-Timestamp':timestamp,
      'X-Grengin-Nonce':nonce,'X-Grengin-Signature':signature}}),env);
    assert.equal(response.status,200);assert.equal(response.headers.get('Cache-Control'),'no-store');
    const text=await response.text();assert(!text.includes(clientSecret));assert(!text.includes(grant));return JSON.parse(text);
  }
  const result=await start();assert.equal(result.status,'pending');
  const url=new URL(result.authorization_url);
  assert.equal(url.origin+url.pathname,'https://slack.com/oauth/v2_user/authorize');
  assert.equal(url.searchParams.get('client_id'),clientId);
  assert.equal(url.searchParams.get('redirect_uri'),`${origin}/v1/connectors/callback/slack`);
  assert.equal(url.searchParams.get('code_challenge_method'),'S256');
  assert.equal(url.searchParams.get('code_challenge').length,43);
  assert.equal(url.searchParams.get('state').length,64);
  assert.deepEqual(url.searchParams.get('scope').split(' '),auth.scopes);
  assert.deepEqual(await start(),result);
  const stored=sqlite.prepare('SELECT * FROM connector_connections').get();
  assert(stored.secret_payload && !stored.secret_payload.includes('pkce'));
  assert.equal(stored.status,'pending');assert.equal(requests,3);
  console.log(`PASS: published Slack ${entry.latestVersion} matches its hash; saved client settings produce an idempotent broker start with PKCE and the exact service callback.`);
  console.log('No vendor consent or token exchange was attempted. This does not validate the client secret or Slack app registration with Slack.');
} finally {
  globalThis.fetch=originalFetch;sqlite.close();
}
