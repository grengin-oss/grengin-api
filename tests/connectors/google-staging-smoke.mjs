import assert from 'node:assert/strict';
import {createHash,generateKeyPairSync,randomBytes,randomUUID,sign} from 'node:crypto';

const origin='https://service-staging.grengin.com';
const callback='http://localhost:5173/mcp/oauth/callback';
assert(process.env.GOOGLE_CLIENT_ID,'Saved Google client ID is required');
let passed=0,blocked=0;
function request(f,action,extra={}){
  const path=action==='start'?'/v1/connectors/start':`/v1/connectors/${f.id}/${action}`;
  const body=JSON.stringify({user_id:f.user,...(action==='start'?{connection_id:f.id,connector_key:f.key,version:'1.0.0',redirect_uri:callback}:{}),...extra});
  const timestamp=String(Math.floor(Date.now()/1000)),nonce=randomBytes(32).toString('hex');
  const proof=`grengin-connectors-v2\n${f.id}\n${f.user}\n${timestamp}\n${nonce}\nPOST\n${path}\n${createHash('sha256').update(body).digest('hex')}`;
  return new Request(origin+path,{method:'POST',headers:{'Content-Type':'application/json','X-Grengin-Connection':f.id,
    'X-Grengin-Public-Key':Buffer.from(f.identity.publicKey.export({format:'jwk'}).x,'base64url').toString('hex'),
    'X-Grengin-Timestamp':timestamp,'X-Grengin-Nonce':nonce,
    'X-Grengin-Signature':sign(null,Buffer.from(proof),f.identity.privateKey).toString('hex')},body});
}
async function send(req){
  const response=await fetch(req,{redirect:'error',signal:AbortSignal.timeout(20000)});
  const raw=await response.text();assert(raw.length<16384,'Broker response must be bounded');
  let body;try{body=JSON.parse(raw);}catch{throw new Error(`Non-JSON broker response (HTTP ${response.status}); body redacted`);}
  return {response,body};
}
for(const key of ['gmail','google-calendar','google-drive']){
  const f={key,id:randomUUID(),user:randomUUID(),identity:generateKeyPairSync('ed25519')};
  let created=false;
  try{
    const initial=request(f,'start'),replay=initial.clone();
    const {response,body}=await send(initial);
    if(response.status===503&&body.error?.type==='connector_not_configured'){
      console.log(`BLOCKED: ${key}: Google is not configured in the deployed staging broker`);blocked++;continue;
    }
    assert.equal(response.status,200,`${key}: staging start must return HTTP 200; response details redacted`);created=true;
    assert.match(response.headers.get('cache-control')||'',/no-store/);assert.equal(body.status,'pending');
    const authorization=new URL(body.authorization_url);
    assert.equal(authorization.origin+authorization.pathname,'https://accounts.google.com/o/oauth2/v2/auth');
    assert(authorization.searchParams.get('client_id')===process.env.GOOGLE_CLIENT_ID,'Staging broker uses a different Google client');
    assert.equal(authorization.searchParams.get('redirect_uri'),`${origin}/v1/connectors/callback/${key}`);
    assert.equal(authorization.searchParams.get('access_type'),'offline');assert.equal(authorization.searchParams.get('prompt'),'consent');
    assert.equal(authorization.searchParams.get('code_challenge_method'),'S256');assert.match(authorization.searchParams.get('state'),/^c2_[a-f0-9]{64}$/);
    assert(!authorization.href.includes('localhost'));assert(!authorization.href.includes(process.env.GOOGLE_CLIENT_SECRET));
    const second=await send(request(f,'start'));assert.equal(second.response.status,200);assert.equal(second.body.authorization_url,body.authorization_url);
    const repeated=await send(replay);assert.equal(repeated.response.status,409);assert.equal(repeated.body.error.type,'connector_proof_replayed');
    const pending=await send(request(f,'status'));assert.equal(pending.response.status,200);assert.equal(pending.body.status,'pending');
    assert.equal((await send(request(f,'token'))).response.status,409);
    const stranger={...f,identity:generateKeyPairSync('ed25519')};assert.equal((await send(request(stranger,'status'))).response.status,404);
    const revoked=await send(request(f,'disconnect'));assert.equal(revoked.response.status,200);assert.equal(revoked.body.status,'revoked');created=false;
    console.log(`PASS: ${key}: LIVE staging start, correct client/callback, offline consent, PKCE, idempotency, replay/ownership protection, pending-token refusal and cleanup`);passed++;
  }finally{
    if(created){const cleanup=await send(request(f,'disconnect'));assert.equal(cleanup.response.status,200,'Staging test cleanup must succeed');}
  }
}
console.log(`Result: ${passed} providers passed; ${blocked} blocked. No vendor consent, token exchange or MCP tool calls attempted.`);
if(blocked)process.exitCode=2;
