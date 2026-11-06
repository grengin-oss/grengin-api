import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {join} from 'node:path';
import {callbackFor,microsoftConnectorIds} from './live-callbacks.mjs';

const root=process.env.GRENGIN_BACKEND_ROOT;
const catalog=process.env.GRENGIN_CONNECTOR_PLUGIN_CATALOG_DIR ?? join(root,'../grengin-list/master-data/connector-plugins');
const providers=[
  {id:'jira',prefix:'JIRA',url:'https://auth.atlassian.com/oauth/token'},
  {id:'github',prefix:process.env.CONNECTOR_GITHUB_CLIENT_ID?'CONNECTOR_GITHUB':'GITHUB',url:'https://github.com/login/oauth/access_token'},
  {id:'slack',prefix:'SLACK',url:'https://slack.com/api/oauth.v2.access'},
  {id:'gmail',prefix:'GOOGLE',url:'https://oauth2.googleapis.com/token'},
  {id:'google-calendar',prefix:'GOOGLE',url:'https://oauth2.googleapis.com/token'},
  {id:'google-drive',prefix:'GOOGLE',url:'https://oauth2.googleapis.com/token'},
  ...microsoftConnectorIds.map(id=>({id,prefix:process.env.CONNECTOR_MICROSOFT_CLIENT_ID?'CONNECTOR_MICROSOFT':process.env.MICROSOFT_CLIENT_ID?'MICROSOFT':'AZURE',url:'https://login.microsoftonline.com/organizations/oauth2/v2.0/token'})),
];
const selected=new Set(process.argv.slice(2));
assert([...selected].every(id=>providers.some(provider=>provider.id===id)),'Unknown preflight connector');
const accepted=new Set(['invalid_grant','bad_verification_code','bad_code','invalid_code']);
let failures=0;
for(const provider of providers){
  if(selected.size&&!selected.has(provider.id))continue;
  const manifest=JSON.parse(await readFile(join(catalog,provider.id,'versions/1.0.0/plugin.json'),'utf8'));
  const setup=JSON.parse(await readFile(join(catalog,provider.id,'versions/1.0.0/setup.json'),'utf8'));
  assert.equal(setup.plugin_id,manifest.id);
  assert.equal(setup.authentication.oauth.token_url,provider.url);
  const id=process.env[`${provider.prefix}_CLIENT_ID`],secret=process.env[`${provider.prefix}_CLIENT_SECRET`];
  if(!id||!secret){console.log(`${provider.id}: BLOCKED missing ${provider.prefix}_CLIENT_ID/SECRET`);failures++;continue;}
  if(id===secret){console.log(`${provider.id}: BLOCKED client ID and secret are identical; correct the local credentials`);failures++;continue;}
  const oauth=setup.authentication.oauth;
  const callback=callbackFor(provider.id);
  const payload={grant_type:'authorization_code',client_id:id,client_secret:secret,code:'grengin-invalid-code-preflight-no-consent',redirect_uri:callback};
  if(oauth.use_pkce)payload.code_verifier='a'.repeat(64);
  try{
    const response=await fetch(provider.url,{method:'POST',redirect:'manual',headers:{Accept:'application/json','Content-Type':oauth.token_request_format==='json'?'application/json':'application/x-www-form-urlencoded'},body:oauth.token_request_format==='json'?JSON.stringify(payload):new URLSearchParams(payload),signal:AbortSignal.timeout(15000)});
    let bytes=0,text='';
    for await(const chunk of response.body){bytes+=chunk.length;assert(bytes<=65536,'Token endpoint response exceeded bound');text+=Buffer.from(chunk).toString();}
    const body=JSON.parse(text);
    assert(!body.access_token&&!body.authed_user?.access_token,'An invalid code must not create an account session');
    const raw=typeof body.error==='string'?body.error:body.error?.code;
    const error=typeof raw==='string'&&/^[a-z_]{1,64}$/.test(raw)?raw:'unclassified_error';
    const pass=accepted.has(error);
    if(!pass)failures++;
    console.log(`${provider.id}: ${pass?'PASS':'CHECK'} HTTP ${response.status}; ${error}; no token returned`);
    if(Array.isArray(body.error_codes))console.log(`${provider.id}: Microsoft diagnostic codes ${body.error_codes.filter(code=>Number.isSafeInteger(code)).slice(0,5).join(',')}`);
    if(!pass){
      const description=String(body.error_description??body.error?.message??body.message??'').toLowerCase();
      console.log(`${provider.id}: redacted rejection categories ${JSON.stringify({code:/authorization code|invalid code|malformed.*code/.test(description),redirect:/redirect|callback/.test(description),pkce:/pkce|verifier|challenge/.test(description),client:/client/.test(description)})}`);
    }
  }catch(error){
    failures++;
    console.log(`${provider.id}: BLOCKED ${error.name==='TimeoutError'?'request timeout':'transport or response validation failure'}; details redacted`);
  }
}
console.log('These are live invalid-code probes, NOT successful user consent or tool-call tests. No redirects were followed and no token/secret was printed.');
process.exitCode=failures?1:0;
