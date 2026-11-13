import assert from 'node:assert/strict';
import {createRequire} from 'node:module';

const frontend='/home/anurag/work/fresh/webapp';
const require=createRequire('/home/anurag/work/grengin-website/package.json');
const esbuild=createRequire(require.resolve('tsx/package.json'))('esbuild');
const server='local-slack-server';
const state='a'.repeat(64);
const receipt='b'.repeat(64);
const calls=[];
let user={id:'existing-user'};
let callbackCalls=0;
let storedConnection=null;
let disconnected={id:'connection',status:'revoked',revocation_pending:false};
let authorization='https://slack.com/oauth/v2_user/authorize?state='+state+'&redirect_uri='+encodeURIComponent('http://localhost:5173/mcp/oauth/callback');
globalThis.localConnectorUser=()=>user;
globalThis.localConnectorRequest=async(path,options={})=>{
  calls.push({path,options});
  if(path==='/connectors')return storedConnection?[storedConnection]:[];
  if(path==='/connectors/slack/connect')return {id:'connection',server_id:server,status:'pending',authorization_url:authorization};
  if(path==='/connectors/connection/complete')return {};
  if(path==='/connectors/connection')return storedConnection??{id:'connection',status:'connected'};
  if(path==='/connectors/connection/disconnect')return disconnected;
  if(path===`/admin/mcp-servers/${server}/sync-tools`)return {success:true};
  throw new Error('Unexpected API path');
};
globalThis.sessionStorage={data:new Map(),getItem(key){return this.data.get(key)??null;},
  setItem(key,value){this.data.set(key,value);},removeItem(key){this.data.delete(key);}};
globalThis.fetch=async(path,options)=>{callbackCalls++;assert(path.startsWith('/__local/connector-broker/callback/slack?'));
  assert.equal(options.credentials,'omit');assert(!options.headers?.Authorization);
  return new Response('Enter this code:\n'+receipt+'\n');};
async function load(dev){
  const result=await esbuild.build({entryPoints:[frontend+'/src/lib/api/cloudConnectors.ts'],bundle:true,write:false,
    format:'esm',platform:'node',target:'es2022',logLevel:'silent',define:{
      'import.meta.env.DEV':String(dev),'import.meta.env.VITE_LOCAL_CONNECTOR_BROKER':'"1"',
      'import.meta.env.VITE_LOCAL_SLACK_SERVER_ID':JSON.stringify(server)},plugins:[{name:'local-api-boundary',setup(build){
        build.onResolve({filter:/(^|\/)client\.js$/},()=>({path:'client',namespace:'mock'}));
        build.onResolve({filter:/state\.svelte\.js$/},()=>({path:'auth',namespace:'mock'}));
        build.onLoad({filter:/.*/,namespace:'mock'},args=>({contents:args.path==='client'
          ?'export const request=globalThis.localConnectorRequest; export class ApiError extends Error { constructor(status,detail){super(detail);this.status=status;} }'
          :'export const getUser=globalThis.localConnectorUser;',loader:'js'}));
      }}]});
  return import('data:text/javascript;base64,'+Buffer.from(result.outputFiles[0].contents).toString('base64'));
}
const api=await load(true);
assert(api.isLocalSlackServer(server));assert(!api.isLocalSlackServer('other-server'));
assert.equal(await api.authorizeLocalSlack(server),authorization);
assert.deepEqual(JSON.parse(calls.find(v=>v.path==='/connectors/slack/connect').options.body),{version:'1.0.0',server_id:server});
const popup={sessionStorage:{setItem(key,value){assert.equal(value,sessionStorage.getItem(key));}}};
api.prepareConnectorPopup(popup,server);
assert(api.isLocalConnectorCallback());
await assert.rejects(()=>api.receiveConnectorApproval(new URLSearchParams({state:'wrong',code:'example'})));
assert.equal(callbackCalls,0);
user={id:'another-user'};
await assert.rejects(()=>api.receiveConnectorApproval(new URLSearchParams({state,code:'example'})));
assert.equal(callbackCalls,0);
user={id:'existing-user'};
const originalFetch=globalThis.fetch;
globalThis.fetch=async()=>Response.json({error:{type:'connector_reconnect_required',description:'PRIVATE_PROVIDER_BODY'}},{status:409});
await assert.rejects(()=>api.receiveConnectorApproval(new URLSearchParams({state,code:'example'})),/fresh connection/);
globalThis.fetch=async()=>Response.json({error:{type:'PRIVATE_UNTRUSTED_CODE',description:'PRIVATE_PROVIDER_BODY'}},{status:503});
await assert.rejects(()=>api.receiveConnectorApproval(new URLSearchParams({state,code:'example'})),error=>!error.message.includes('PRIVATE'));
globalThis.fetch=originalFetch;
const approval=await api.receiveConnectorApproval(new URLSearchParams({state,code:'example'}));
assert.equal(approval.confirmation_code,receipt);
assert(![...sessionStorage.data.values()].some(value=>value.includes(receipt)));
assert(!calls.some(value=>value.path.includes('/complete')));
assert.equal(await api.completeConnectorApproval(approval),server);
assert.deepEqual(JSON.parse(calls.find(value=>value.path.endsWith('/complete')).options.body),{confirmation_code:receipt});
assert(!api.isLocalConnectorCallback());
for(const status of ['connected','awaiting_confirmation','expired','revoked','failed']){
  storedConnection={id:'connection',server_id:server,status};
  const start=calls.length;
  assert.equal(await api.authorizeLocalSlack(server),authorization);
  const paths=calls.slice(start).map(call=>call.path);
  assert(paths.includes('/connectors/connection/disconnect'));
  assert(paths.indexOf('/connectors/connection/disconnect')<paths.indexOf('/connectors/slack/connect'));
}
storedConnection={id:'connection',server_id:server,status:'connected'};
disconnected={...disconnected,revocation_pending:true};
const waiting=calls.length;
await assert.rejects(()=>api.authorizeLocalSlack(server),error=>error.status===409&&error.message.includes('cleaned up'));
assert(!calls.slice(waiting).some(call=>call.path==='/connectors/slack/connect'));
disconnected={...disconnected,revocation_pending:false};
storedConnection={id:'connection',server_id:server,status:'pending',authorization_url:authorization};
const pending=calls.length;
assert.equal(await api.authorizeLocalSlack(server),authorization);
assert(!calls.slice(pending).some(call=>call.path.endsWith('/disconnect')));
storedConnection=null;
authorization='https://untrusted.example/authorize?state='+state;
await assert.rejects(()=>api.authorizeLocalSlack(server));
const production=await load(false);
assert(!production.isLocalSlackServer(server));assert(!production.isLocalConnectorCallback());
console.log('PASS: real frontend broker API helper: correct start payload, explicit reconnect of connected/abandoned/terminal states, cleanup-pending blocks fresh consent, pending consent reuses state, popup context, state/owner rejection, anonymous callback without bearer, in-memory-only receipt, explicit confirmation, verified status/tool sync, unsafe URL refusal, production gate disabled.');
