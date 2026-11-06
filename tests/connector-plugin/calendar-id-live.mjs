import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import {createHmac,createDecipheriv} from 'node:crypto';
import {execFileSync} from 'node:child_process';
import {fileURLToPath} from 'node:url';
import path from 'node:path';

const root=path.resolve(path.dirname(fileURLToPath(import.meta.url)),'../..');
const saved=JSON.parse(await readFile(process.env.HOME+'/.local/state/grengin/native-plugin-live/session.json','utf8'));
const uri=new URL(saved.database);
assert.equal(uri.pathname,'/grengin_native_plugins_live');
assert(['localhost','127.0.0.1'].includes(uri.hostname));
const id=saved.servers['google-calendar'];assert.match(id,/^[0-9a-f-]{36}$/);
const parts=[{alg:'HS256',typ:'JWT'},{user_id:saved.owner.id,sub:saved.owner.email,name:saved.owner.name,token_use:'access',exp:Math.floor(Date.now()/1000)+300}]
  .map(value=>Buffer.from(JSON.stringify(value)).toString('base64url')).join('.');
const auth=parts+'.'+createHmac('sha256',saved.jwt).update(parts).digest('base64url');
async function get(url){
  const response=await fetch('http://127.0.0.1:8083'+url,{redirect:'manual',headers:{Authorization:'Bearer '+auth},signal:AbortSignal.timeout(15000)});
  assert(response.ok,'Isolated diagnostic request HTTP '+response.status);
  return response.json();
}
const server=await get(`/admin/mcp-servers/${id}`);
const history=await get(`/admin/mcp-servers/${id}/executions?limit=30`);
const failed=history.executions.find(row=>row.is_error&&row.tool_name==='list_events'
  &&typeof row.arguments?.calendar_id==='string'&&row.arguments.calendar_id.includes('#'));
assert(failed,'No original # calendar identifier failure to replay');
const quote=value=>"'"+value.replaceAll("'","''")+"'";
const raw=execFileSync('psql',['-XAt','-v','ON_ERROR_STOP=1','-c',
  `SELECT json_build_object('token',"accessToken",'expires',"expiresAt") FROM mcp_connections WHERE "serverId"=${quote(id)} AND "userId"=${quote(saved.owner.id)}`],{
  encoding:'utf8',stdio:['ignore','pipe','pipe'],env:{...process.env,PGHOST:uri.hostname,PGPORT:uri.port||'5432',
    PGUSER:decodeURIComponent(uri.username),PGPASSWORD:decodeURIComponent(uri.password),PGDATABASE:uri.pathname.slice(1),
    PGOPTIONS:'-c default_transaction_read_only=on -c statement_timeout=5000'},
}).trim();
const row=JSON.parse(raw);assert(row.token,'Connected user token required');
assert(!row.expires||Date.parse(row.expires)>Date.now(),'Reconnect or refresh Calendar before this explicit live test');
const bytes=Buffer.from(row.token.startsWith('enc:v1:')?row.token.slice(7):row.token,'base64');
const decipher=createDecipheriv('aes-256-gcm',Buffer.from(saved.key,'base64'),bytes.subarray(0,12));
decipher.setAuthTag(bytes.subarray(-16));
const token=Buffer.concat([decipher.update(bytes.subarray(12,-16)),decipher.final()]).toString();
const candidate=JSON.parse(await readFile(path.join(root,'../grengin-list/docs/drafts/grengin-api/0.9.8/google-calendar-1.2.1/plugin.json'),'utf8'));
assert.equal(candidate.base_url,'https://www.googleapis.com/calendar/v3');
const config={...server.connection_config.connector_plugin,manifest:candidate};
delete config.base_url_override;
const response=execFileSync(path.join(root,'target/debug/examples/live_probe'),[],{
  encoding:'utf8',stdio:['pipe','pipe','pipe'],timeout:35000,maxBuffer:65536,
  input:JSON.stringify({config,tool:'list_events',arguments:{...failed.arguments,limit:1},access_token:token,user_id:saved.owner.id}),
});
const result=JSON.parse(response);assert.equal(result.success,true);
console.log('PASS real Google Calendar read with the previously failing # identifier; output shape='+result.shape+'; event contents and tokens withheld');
console.log('No LLM request, server update, token copy or R2 publication was performed.');
