import assert from 'node:assert/strict';
import {createHash,generateKeyPairSync,randomBytes,randomUUID,sign} from 'node:crypto';
import {pathToFileURL} from 'node:url';

const worker=process.env.GRENGIN_GOOGLE_WORKER_ROOT;
assert(worker,'Run through google-preflight.sh');
const {createDatabase}=await import(pathToFileURL(`${worker}/workers/test/helpers/sqlite-d1.mjs`));
const {handleConnectorBroker}=await import(pathToFileURL(`${worker}/workers/src/services/connector-broker.ts`));
const clientId=process.env.GOOGLE_CLIENT_ID,clientSecret=process.env.GOOGLE_CLIENT_SECRET;
assert(clientId&&clientSecret,'Google client credentials are required');
const origin='https://service-staging.grengin.com';
const frontend='http://localhost:5173/mcp/oauth/callback';
const keys=['gmail','google-calendar','google-drive'];
const networkFetch=globalThis.fetch;
let checks=0;
const pass=label=>console.log(`PASS ${++checks}: ${label}`);
const sha=value=>createHash('sha256').update(value).digest('hex');
async function document(url){
  const response=await networkFetch(url,{redirect:'error',signal:AbortSignal.timeout(15000)});
  assert.equal(response.status,200,'Published connector document must be available');
  const chunks=[];let length=0;
  for await(const chunk of response.body){length+=chunk.length;assert(length<=524288,'Catalog response too large');chunks.push(chunk);}
  const bytes=Buffer.concat(chunks);
  return {value:JSON.parse(bytes.toString()),hash:sha(bytes)};
}
const listing=(await document('https://meta.grengin.com/connectors.json')).value;
const catalogs=new Map();
for(const key of keys){
  const entry=listing.catalog.find(item=>item.key===key);assert(entry,`Missing ${key} listing`);
  const version=entry.versions.find(item=>item.version===entry.latestVersion);assert(version);
  assert.equal(version.url,`https://meta.grengin.com/connectors/${key}/versions/${entry.latestVersion}.json`);
  const catalog=await document(version.url);assert.equal(catalog.hash,version.sha256);
  assert.equal(catalog.value.key,key);assert.equal(catalog.value.connection.auth.provider,'google');
  catalogs.set(key,catalog.value);pass(`${key}: published version and SHA-256 match`);
}
const scopes=[...new Set([...catalogs.values()].flatMap(value=>value.connection.auth.scopes))];
const app={client_id:clientId,client_secret:clientSecret,auth_url:'https://accounts.google.com/o/oauth2/v2/auth',
  token_url:'https://oauth2.googleapis.com/token',scopes,
  mcp_origins:[...new Set([...catalogs.values()].map(value=>new URL(value.connection.url).origin))],
  extra_params:{access_type:'offline',prompt:'consent'}};
const {db,sqlite}=createDatabase();
const encryption=randomBytes(32).toString('hex');
const env={DB:db,SERVICE_BASE_URL:origin,CONNECTOR_BROKER_ENABLED:'true',CONNECTOR_TOKEN_KEY_VERSION:'1',
  CONNECTOR_TOKEN_KEYS:JSON.stringify({'1':encryption}),CONNECTOR_OAUTH_CLIENTS:JSON.stringify({google:app})};
let active,exchangeCount=0,refreshCount=0;
globalThis.fetch=async(input,init={})=>{
  const url=String(input);
  if(url.startsWith('https://meta.grengin.com/connectors/')){
    const key=url.split('/')[4],catalog=catalogs.get(key);
    assert(catalog&&url===`https://meta.grengin.com/connectors/${key}/versions/${catalog.version}.json`);
    return Response.json(catalog);
  }
  assert.equal(url,app.token_url,'No unapproved vendor requests');
  assert.equal(init.redirect,'manual','Broker must not forward credentials on redirects');
  const body=new URLSearchParams(init.body);
  assert(body.get('client_id')===clientId,'Broker must use the configured Google client');
  assert(body.get('client_secret')===clientSecret,'Broker must use the configured Google secret');
  if(body.get('grant_type')==='refresh_token'){
    refreshCount++;assert.equal(body.get('refresh_token'),`TEST_REFRESH_${active.key}`);
    return Response.json({access_token:`TEST_REFRESHED_${active.key}`,token_type:'Bearer',expires_in:3600});
  }
  exchangeCount++;
  assert.equal(body.get('redirect_uri'),`${origin}/v1/connectors/callback/${active.key}`);
  assert.equal(body.get('code'),'TEST_AUTHORIZATION_CODE');
  const verifier=body.get('code_verifier');assert(verifier);
  assert.equal(createHash('sha256').update(verifier).digest('base64url'),active.authorization.searchParams.get('code_challenge'));
  return Response.json({access_token:`TEST_ACCESS_${active.key}`,refresh_token:`TEST_REFRESH_${active.key}`,
    token_type:'Bearer',expires_in:3600});
};
function identity(key){
  const pair=generateKeyPairSync('ed25519');
  return {key,pair,id:randomUUID(),user:randomUUID(),publicKey:Buffer.from(pair.publicKey.export({format:'jwk'}).x,'base64url').toString('hex')};
}
function request(f,action,extra={}){
  const path=action==='start'?'/v1/connectors/start':`/v1/connectors/${f.id}/${action}`;
  const body=JSON.stringify({user_id:f.user,...(action==='start'?{connection_id:f.id,connector_key:f.key,
    version:catalogs.get(f.key).version,redirect_uri:frontend}:{}),...extra});
  const timestamp=String(Math.floor(Date.now()/1000)),nonce=randomBytes(32).toString('hex');
  const proof=`grengin-connectors-v2\n${f.id}\n${f.user}\n${timestamp}\n${nonce}\nPOST\n${path}\n${sha(body)}`;
  return new Request(origin+path,{method:'POST',headers:{'Content-Type':'application/json','X-Grengin-Connection':f.id,
    'X-Grengin-Public-Key':f.publicKey,'X-Grengin-Timestamp':timestamp,'X-Grengin-Nonce':nonce,
    'X-Grengin-Signature':sign(null,Buffer.from(proof),f.pair.privateKey).toString('hex'),'CF-Connecting-IP':'192.0.2.123'},body});
}
async function post(f,action,extra={},status=200){
  const response=await handleConnectorBroker(request(f,action,extra),env);
  assert.equal(response.status,status,`${f.key}: ${action} expected HTTP ${status}`);
  assert.match(response.headers.get('cache-control')||'',/no-store/);
  const text=await response.text();assert(!text.includes(clientSecret),'Client secret leaked to caller');
  return JSON.parse(text);
}
try{
  for(const key of keys){
    const f=identity(key),catalog=catalogs.get(key);
    const initial=request(f,'start'),replay=initial.clone();
    const response=await handleConnectorBroker(initial,env);assert.equal(response.status,200);
    const pending=await response.json();assert.equal(pending.status,'pending');
    const authorization=new URL(pending.authorization_url);active={key,authorization};
    assert.equal(authorization.origin+authorization.pathname,app.auth_url);
    assert.equal(authorization.searchParams.get('client_id'),clientId);
    assert.equal(authorization.searchParams.get('redirect_uri'),`${origin}/v1/connectors/callback/${key}`);
    assert.equal(authorization.searchParams.get('access_type'),'offline');assert.equal(authorization.searchParams.get('prompt'),'consent');
    assert.equal(authorization.searchParams.get('response_type'),'code');
    assert.equal(authorization.searchParams.get('code_challenge_method'),'S256');
    assert.match(authorization.searchParams.get('code_challenge'),/^[A-Za-z0-9_-]{43}$/);
    assert.match(authorization.searchParams.get('state'),/^c2_[a-f0-9]{64}$/);
    assert.deepEqual(authorization.searchParams.get('scope').split(' '),catalog.connection.auth.scopes);
    assert(!authorization.href.includes(clientSecret));
    assert.deepEqual(await post(f,'start'),pending);
    assert.equal((await handleConnectorBroker(replay,env)).status,409);
    pass(`${key}: registration-free, idempotent start; exact callback, narrow scopes, offline consent, PKCE and replay rejection`);
    const stored=sqlite.prepare('SELECT * FROM open_connector_connections WHERE id=?').get(f.id);
    const [version,iv,ciphertext]=stored.secret_payload.split('.');assert.equal(version,'1');
    const secretKey=await crypto.subtle.importKey('raw',Buffer.from(encryption,'hex'),'AES-GCM',false,['decrypt']);
    const aad=[stored.id,`connection:${stored.connection_public_key}`,stored.local_user_id,stored.connector_key,stored.catalog_version].join('\n');
    const plain=JSON.parse(new TextDecoder().decode(await crypto.subtle.decrypt({name:'AES-GCM',iv:Buffer.from(iv,'hex'),
      additionalData:new TextEncoder().encode(aad)},secretKey,Buffer.from(ciphertext,'hex'))));
    assert.equal(plain.callback,`${origin}/v1/connectors/callback/${key}`);assert.equal(plain.redirect_uri,frontend);
    assert.equal(createHash('sha256').update(plain.pkce).digest('base64url'),authorization.searchParams.get('code_challenge'));
    await post(f,'token',{},409);
    await post({...f,user:randomUUID()},'status',{},404);
    await post({...identity(key),id:f.id,user:f.user},'status',{},404);
    pass(`${key}: encrypted PKCE/return binding; tokens blocked before confirmation; wrong user and signer refused`);
    const callback=new Request(`${origin}/v1/connectors/callback/${key}?state=${authorization.searchParams.get('state')}&code=TEST_AUTHORIZATION_CODE`);
    const approved=await handleConnectorBroker(callback,env);assert.equal(approved.status,303);
    const target=new URL(approved.headers.get('location'));assert.equal(target.origin+target.pathname,frontend);
    const receipt=new URLSearchParams(target.hash.slice(1));assert.equal(receipt.get('connection_id'),f.id);
    assert.equal(receipt.get('state'),authorization.searchParams.get('state'));
    assert.match(receipt.get('connector_approval'),/^[a-f0-9]{64}$/);
    assert(!target.href.includes('TEST_ACCESS_'));assert(!target.href.includes('TEST_REFRESH_'));
    await post(f,'token',{},409);
    assert.equal((await post(f,'complete',{confirmation_code:receipt.get('connector_approval')})).status,'connected');
    await post(f,'complete',{confirmation_code:receipt.get('connector_approval')},409);
    assert.equal((await handleConnectorBroker(callback,env)).status,409);
    assert.equal((await post(f,'token')).access_token,`TEST_ACCESS_${key}`);
    assert(!sqlite.prepare('SELECT secret_payload FROM open_connector_connections WHERE id=?').get(f.id).secret_payload.includes('TEST_ACCESS_'));
    pass(`${key}: simulated Google exchange, exact instance redirect, one-time completion and encrypted token checkout`);
    const clock=Date.now;
    try {const time=clock();Date.now=()=>time+3601000;
      assert.equal((await post(f,'token')).access_token,`TEST_REFRESHED_${key}`);
    } finally {Date.now=clock;}
    assert.equal((await post(f,'disconnect')).status,'revoked');
    await post(f,'token',{},409);
    pass(`${key}: simulated refresh and disconnect prevent further token checkout`);
  }
  assert.equal(exchangeCount,3);assert.equal(refreshCount,3);
  assert.equal(sqlite.prepare('SELECT count(*) AS n FROM installations').get().n,0);
  assert.equal(sqlite.prepare('SELECT count(*) AS n FROM organizations').get().n,0);
} finally {globalThis.fetch=networkFetch;sqlite.close();}

// An invalid code tests the token endpoint without granting access to an account.
const probe=await networkFetch(app.token_url,{method:'POST',headers:{'Content-Type':'application/x-www-form-urlencoded'},
  body:new URLSearchParams({client_id:clientId,client_secret:clientSecret,grant_type:'authorization_code',
    code:'grengin-deliberately-invalid-preflight-code',redirect_uri:`${origin}/v1/connectors/callback/gmail`,code_verifier:'a'.repeat(64)}),
  redirect:'error',signal:AbortSignal.timeout(15000)});
const answer=await probe.json();
assert.equal(probe.status,400);assert.equal(answer.error,'invalid_grant','Google did not return the expected invalid-code response');
assert(!answer.access_token);
pass('LIVE Google token endpoint: invalid_grant, not invalid_client, with the saved credentials and a deliberately invalid code');
console.log(`PASS: ${checks} grouped checks; published catalogs fetched live; callback, token and refresh success paths simulated locally.`);
console.log('NOT VERIFIED: registered redirect URIs, real user consent, Workspace Developer Preview eligibility, actual MCP tools or production Worker configuration.');
