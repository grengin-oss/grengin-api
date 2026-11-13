import assert from 'node:assert/strict';
import {createRequire} from 'node:module';
import {pathToFileURL} from 'node:url';

const website=process.env.GRENGIN_WEBSITE_ROOT||'/home/anurag/work/grengin-website';
const require=createRequire(`${website}/workers/package.json`);
const {getPublicKeyAsync,signAsync}=await import(pathToFileURL(require.resolve('@noble/ed25519')));
const {createDatabase}=await import(pathToFileURL(`${website}/workers/test/helpers/sqlite-d1.mjs`));
const {handleConnectorBroker}=await import(pathToFileURL(`${website}/workers/src/services/connector-broker.ts`));
const originalFetch=globalThis.fetch;
const hex=value=>Buffer.from(value).toString('hex');
const digest=async value=>hex(await crypto.subtle.digest('SHA-256',new TextEncoder().encode(value)));
const resource='https://mcp.audit.invalid';
const issuer='https://authorization.audit.invalid';

async function probe({name,extraResource=false,strictResource=false,wrongIssuer=false}) {
  const {db,sqlite}=createDatabase();
  const installation=`audit-${crypto.randomUUID()}`,user=crypto.randomUUID(),id=crypto.randomUUID();
  const grant=hex(crypto.getRandomValues(new Uint8Array(32)));
  const key=crypto.getRandomValues(new Uint8Array(32));
  const catalog={key:'standard-audit',version:'1.0.0',protocols:[{transport:'streamable_http'}],connection:{
    transport_type:'http',url:`${resource}/mcp`,auth:{type:'oauth',provider:'review-vendor',
      auth_url:`${issuer}/authorize`,token_url:`${issuer}/token`,scopes:['read:tools'],
      token_endpoint_auth_method:'client_secret_post',...(extraResource?{extra_params:{resource}}:{})}}};
  const app={client_id:'fake-audit-client',client_secret:'FAKE_SPEC_ONLY',auth_url:catalog.connection.auth.auth_url,
    token_url:catalog.connection.auth.token_url,mcp_origins:[resource],scopes:['read:tools'],
    ...(extraResource?{extra_params:{resource}}:{})};
  const env={DB:db,SERVICE_BASE_URL:'https://service.audit.invalid',CONNECTOR_BROKER_ENABLED:'true',
    CONNECTOR_TOKEN_KEYS:JSON.stringify({'1':'b'.repeat(64)}),CONNECTOR_TOKEN_KEY_VERSION:'1',
    CONNECTOR_OAUTH_CLIENTS:JSON.stringify({'review-vendor':app})};
  sqlite.prepare("INSERT INTO organizations(id,name,created_at,updated_at) VALUES ('audit-org','Audit','now','now')").run();
  sqlite.prepare(`INSERT INTO installations(id,organization_id,name,public_key,environment,is_revoked,created_at,instance_uuid,connection_status,kind)
    VALUES (?,'audit-org','Audit',?,'prod',0,'now',?,'connected','standalone')`).run(installation,hex(await getPublicKeyAsync(key)),user);
  let tokenRequests=0,metadataRequests=0,tokenResource=null;
  globalThis.fetch=async(url,init={})=>{
    if(String(url)==='https://meta.grengin.com/connectors/standard-audit/versions/1.0.0.json')return Response.json(catalog);
    if(String(url)===app.token_url){
      tokenRequests++;tokenResource=new URLSearchParams(init.body).get('resource');
      if(strictResource&&tokenResource!==resource)return Response.json({error:'invalid_target'},{status:400});
      return Response.json({access_token:'FAKE_AUDIT_ACCESS',token_type:'Bearer',expires_in:3600});
    }
    if(String(url)===`${resource}/.well-known/oauth-protected-resource`){
      metadataRequests++;return Response.json({resource,authorization_servers:[issuer]});
    }
    if(String(url)===`${issuer}/.well-known/oauth-authorization-server`){
      metadataRequests++;return Response.json({issuer,authorization_endpoint:app.auth_url,token_endpoint:app.token_url,
        authorization_response_iss_parameter_supported:true,code_challenge_methods_supported:['S256']});
    }
    throw new Error('Unexpected network access in isolated protocol audit');
  };
  try {
    const pathname='/v1/connectors/start';
    const body=JSON.stringify({user_id:user,connection_grant:grant,connection_id:id,connector_key:catalog.key,version:catalog.version});
    const timestamp=String(Math.floor(Date.now()/1000)),nonce=hex(crypto.getRandomValues(new Uint8Array(32)));
    const message=`grengin-connectors-v1\n${installation}\n${timestamp}\n${nonce}\nPOST\n${pathname}\n${await digest(body)}`;
    const signature=hex(await signAsync(new TextEncoder().encode(message),key));
    const start=await handleConnectorBroker(new Request(env.SERVICE_BASE_URL+pathname,{method:'POST',body,
      headers:{'Content-Type':'application/json','X-Grengin-Installation':installation,'X-Grengin-Timestamp':timestamp,
        'X-Grengin-Nonce':nonce,'X-Grengin-Signature':signature}}),env);
    assert.equal(start.status,200,'Protocol fixture must start successfully');
    const authorization=new URL((await start.json()).authorization_url);
    const callback=new URL(`${env.SERVICE_BASE_URL}/v1/connectors/callback/${catalog.key}`);
    callback.searchParams.set('state',authorization.searchParams.get('state'));
    callback.searchParams.set('code','FAKE_AUDIT_CODE');
    if(wrongIssuer)callback.searchParams.set('iss','https://wrong-issuer.audit.invalid');
    const response=await handleConnectorBroker(new Request(callback),env);
    const row=sqlite.prepare('SELECT status FROM connector_connections WHERE id=?').get(id);
    const conforms=wrongIssuer?response.status>=400&&tokenRequests===0:
      authorization.searchParams.get('resource')===resource&&tokenResource===resource&&response.status===200;
    return {name,conforms,authorization_resource:authorization.searchParams.get('resource'),token_resource:tokenResource,
      callback_status:response.status,stored_status:row.status,token_requests:tokenRequests,metadata_requests:metadataRequests};
  } finally {globalThis.fetch=originalFetch;sqlite.close();}
}

const control=await probe({name:'existing pinned-provider control'});
assert.equal(control.callback_status,200);assert.equal(control.stored_status,'awaiting_confirmation');
const results=[];
results.push(await probe({name:'strict resource server',strictResource:true}));
results.push(await probe({name:'resource configured as approved authorization extra',extraResource:true,strictResource:true}));
results.push(await probe({name:'wrong callback issuer',wrongIssuer:true}));
console.log(JSON.stringify({control_passed:true,results},null,2));
if(results.some(result=>!result.conforms)){
  console.error('Latest-protocol acceptance gaps reproduced; this is separate from the passing pinned-provider suite.');
  process.exitCode=1;
}
