import assert from 'node:assert/strict';
import {createHmac, randomUUID} from 'node:crypto';
import {execFile, execFileSync} from 'node:child_process';
import {promisify} from 'node:util';
import {readFile} from 'node:fs/promises';
import {createServer} from 'node:http';

const db = new URL(process.env.DATABASE_URL);
assert.equal(db.hostname, '127.0.0.1');
assert.equal(db.pathname, '/grengin_security_test', 'refusing a non-disposable database');
const base = process.env.GRENGIN_SECURITY_TEST_URL;
assert.equal(new URL(base).hostname, '127.0.0.1');
const sql = query => execFileSync('psql', ['-X','-A','-t','-v','ON_ERROR_STOP=1',process.env.DATABASE_URL,'-c',query], {encoding:'utf8',stdio:['ignore','pipe','pipe']}).trim();
const q = value => `'${String(value).replaceAll("'", "''")}'`;
const j = value => `${q(JSON.stringify(value))}::jsonb`;
const delay = milliseconds => new Promise(resolve=>setTimeout(resolve,milliseconds));
async function waitFor(predicate,description) {
  for (let index=0;index<400;index++) {if(predicate())return;await delay(10);}
  assert.fail(`timed out waiting for ${description}`);
}
const adminId = sql(`SELECT a."userId" FROM user_role_assignments a JOIN roles r ON r.id=a."roleId" JOIN users u ON u.id=a."userId" WHERE r.name='Super Admin' AND a."scopeDepartmentId" IS NULL AND u.status='active' LIMIT 1`);
function token(user_id) {
  const body = [Buffer.from(JSON.stringify({alg:'HS256',typ:'JWT'})).toString('base64url'),Buffer.from(JSON.stringify({sub:'staging@example.invalid',user_id,name:'Staging fixture',exp:Math.floor(Date.now()/1000)+1800,token_use:'access'})).toString('base64url')].join('.');
  return `${body}.${createHmac('sha256',process.env.JWT_SECRET).update(body).digest('base64url')}`;
}
const admin = token(adminId);
let checks = 0;
function check(name, actual, expected = true) {assert.deepEqual(actual,expected,name);console.log(`PASS extended ${++checks}: ${name}`);}
async function request(method,path,body,bearer=admin) {
  return fetch(`${base}${path}`,{method,redirect:'manual',headers:{'content-type':'application/json',...(bearer?{authorization:`Bearer ${bearer}`}:{})},body:body===undefined?undefined:JSON.stringify(body),signal:AbortSignal.timeout(45000)});
}
async function expect(method,path,body,status,bearer=admin) {
  const response = await request(method,path,body,bearer), text = await response.text();
  assert.equal(response.status,status,`${method} ${path}: ${response.status}: ${text.slice(0,200)}`);
  checks++; return {text,json:()=>JSON.parse(text)};
}
async function newUser() {
  const email = `staging-${randomUUID()}@example.invalid`;
  await expect('POST','/admin/users',{name:'Staging fixture',email},201);
  const id = sql(`SELECT id FROM users WHERE email=${q(email)}`);
  await expect('PATCH',`/admin/users/${id}/status`,{status:'active'},200);
  return id;
}
async function fault(table, operation, body, run) {
  sql(`CREATE FUNCTION staging_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN ${body}; RETURN NEW; END $$; CREATE TRIGGER staging_failure BEFORE ${operation} ON ${table} FOR EACH ROW EXECUTE FUNCTION staging_failure()`);
  try {await run();} finally {sql(`DROP TRIGGER staging_failure ON ${table}; DROP FUNCTION staging_failure()`);}
}
function events(text) {
  return [...text.matchAll(/^data: (.+)$/gm)].flatMap(match=>{try{return [JSON.parse(match[1])];}catch{return [];}});
}
const payload = (content,model='staging-fast') => ({provider:'staging-fixture',model_name:model,messages:[{role:'user',content,files:[]}]});
const tokenCalls = [];
const provider = createServer(async (req,res) => {
  let raw = ''; for await (const part of req) raw += part;
  if (req.url === '/token') {
    const code = new URLSearchParams(raw).get('code'); tokenCalls.push(code);
    res.writeHead(code==='rejected'?400:200,{'content-type':'application/json'});
    res.end(JSON.stringify(code==='rejected'?{error:'invalid_grant'}:{access_token:'fixture-access',refresh_token:'fixture-refresh',token_type:'Bearer',expires_in:3600}));
    return;
  }
  const input = JSON.parse(raw);
  if (input.model==='staging-failure') {res.writeHead(400,{'content-type':'application/json'});res.end(JSON.stringify({error:{message:'fixture rejection',type:'invalid_request_error'}}));return;}
  res.writeHead(200,{'content-type':'text/event-stream'});
  res.write(`data: ${JSON.stringify({choices:[{delta:{content:'staging fixture reply'}}]})}\n\n`);
  const finish = () => res.end(`data: ${JSON.stringify({choices:[{delta:{},finish_reason:'stop'}],usage:{prompt_tokens:1,completion_tokens:1,total_tokens:2}})}\n\ndata: [DONE]\n\n`);
  if (input.model==='staging-slow') {
    const timer = setTimeout(finish,10000);
    res.on('close',()=>clearTimeout(timer));
  } else finish();
});
await new Promise(resolve=>provider.listen(0,'127.0.0.1',resolve));
try {
  const ownerId = await newUser(), viewerId = await newUser();
  const owner = token(ownerId), viewer = token(viewerId);
  const roleBody = {name:`Staging ${randomUUID()}`,permissions:['chats:share']};
  await fault('auth_audit_events','INSERT',"RAISE EXCEPTION 'staging audit failure'",async()=>{
    await expect('POST','/admin/roles',roleBody,503);
    check('role create and audit are atomic',sql(`SELECT count(*) FROM roles WHERE name=${q(roleBody.name)}`),'0');
  });
  const role = (await expect('POST','/admin/roles',roleBody,201)).json();
  await fault('role_permissions','INSERT',"RAISE EXCEPTION 'staging permission failure'",async()=>{
    await expect('PUT',`/admin/roles/${role.id}`,{name:'should roll back',permissions:['skills:view']},503);
    const unchanged = (await expect('GET',`/admin/roles/${role.id}`,undefined,200)).json();
    check('failed role update retains name and permissions',[unchanged.name,unchanged.permissions],[roleBody.name,['chats:share']]);
  });
  await fault('auth_audit_events','INSERT',"RAISE EXCEPTION 'staging audit failure'",async()=>{
    await expect('POST',`/admin/users/${viewerId}/roles`,{role_id:role.id},503);
    check('failed grant audit leaves no assignment',sql(`SELECT count(*) FROM user_role_assignments WHERE "roleId"=${q(role.id)}`),'0');
    await expect('DELETE',`/admin/roles/${role.id}`,undefined,503);
    check('failed delete audit preserves role',sql(`SELECT count(*) FROM roles WHERE id=${q(role.id)}`),'1');
  });
  const assignment = (await expect('POST',`/admin/users/${viewerId}/roles`,{role_id:role.id},201)).json();
  await fault('auth_audit_events','INSERT',"RAISE EXCEPTION 'staging audit failure'",async()=>{
    await expect('DELETE',`/admin/users/${viewerId}/roles/${assignment.id}`,undefined,503);
    check('failed revoke audit preserves assignment',sql(`SELECT count(*) FROM user_role_assignments WHERE id=${q(assignment.id)}`),'1');
  });
  await expect('DELETE',`/admin/users/${viewerId}/roles/${assignment.id}`,undefined,204);
  const superRole = sql(`SELECT id FROM roles WHERE name='Super Admin' LIMIT 1`);
  const hrRole = sql(`SELECT id FROM roles WHERE name='HR Admin' LIMIT 1`);
  await expect('POST',`/admin/users/${ownerId}/roles`,{role_id:hrRole},201);
  await expect('POST',`/admin/users/${viewerId}/roles`,{role_id:superRole},403,owner);
  await expect('PATCH',`/admin/users/${adminId}/status`,{status:'deactivated'},403,owner);
  check('HR admin cannot grant or deactivate Super Admin',sql(`SELECT status FROM users WHERE id=${q(adminId)}`),'active');
  const departmentBody = {name:`Staging department ${randomUUID()}`,description:'atomic test',admin_ids:[viewerId]};
  await expect('POST','/admin/departments',{...departmentBody,admin_ids:[randomUUID()]},404);
  check('invalid department admin leaves no department',sql(`SELECT count(*) FROM departments WHERE name=${q(departmentBody.name)}`),'0');
  await fault('auth_audit_events','INSERT',"RAISE EXCEPTION 'staging audit failure'",async()=>{
    await expect('POST','/admin/departments',departmentBody,503);
    check('failed create audit rolls back department',sql(`SELECT count(*) FROM departments WHERE name=${q(departmentBody.name)}`),'0');
  });
  const department = (await expect('POST','/admin/departments',departmentBody,201)).json();
  await expect('POST',`/admin/users/${ownerId}/roles`,{role_id:superRole,scope_department_id:department.id},403);
  await fault('auth_audit_events','INSERT',"RAISE EXCEPTION 'staging audit failure'",async()=>{
    await expect('PUT',`/admin/departments/${department.id}`,{name:'should roll back',admin_ids:[ownerId]},503);
    check('department fields and admin changes roll back together',[sql(`SELECT name FROM departments WHERE id=${q(department.id)}`),sql(`SELECT "userId" FROM user_role_assignments WHERE "scopeDepartmentId"=${q(department.id)}`)],[departmentBody.name,viewerId]);
  });
  await expect('PUT',`/admin/departments/${department.id}`,{budget_allocated:-1},400);
  await fault('user_role_assignments','INSERT',`IF NEW."scopeDepartmentId"=${q(department.id)}::uuid THEN PERFORM pg_sleep(1); END IF`,async()=>{
    const first=expect('PUT',`/admin/departments/${department.id}`,{admin_ids:[ownerId]},200);
    await waitFor(()=>sql(`SELECT count(*) FROM pg_stat_activity WHERE wait_event='PgSleep' AND query ILIKE '%user_role_assignments%'`)!=='0','first department grant');
    const second=expect('PUT',`/admin/departments/${department.id}`,{admin_ids:[viewerId]},200);
    await Promise.all([first,second]);
    check('concurrent admin replacement does not combine admin sets',sql(`SELECT "userId" FROM user_role_assignments WHERE "scopeDepartmentId"=${q(department.id)}`),viewerId);
  });
  async function holdDepartment(seconds,change='') {
    const name=`staging-lock-${randomUUID()}`;
    const task=promisify(execFile)('psql',['-X','-q','-v','ON_ERROR_STOP=1',process.env.DATABASE_URL,'-c',`SET application_name=${q(name)}; BEGIN; SELECT id FROM departments WHERE id=${q(department.id)} FOR UPDATE; ${change} SELECT pg_sleep(${seconds}); COMMIT;`]);
    await waitFor(()=>sql(`SELECT count(*) FROM pg_stat_activity WHERE application_name=${q(name)} AND wait_event='PgSleep'`)==='1','department lock');
    return {task};
  }
  const budgetWrite=await holdDepartment(1,`UPDATE departments SET "budgetAllocated"=75,"actionOnExceed"='block' WHERE id=${q(department.id)};`);
  await expect('PUT',`/admin/departments/${department.id}`,{admin_ids:[viewerId]},200);
  await budgetWrite.task;
  check('queued admin edit preserves freshly committed budget',sql(`SELECT "budgetAllocated"::int::text||':'||"actionOnExceed" FROM departments WHERE id=${q(department.id)}`),'75:block');
  const longWrite=await holdDepartment(7);
  const waitStarted=Date.now();
  await expect('PUT',`/admin/departments/${department.id}`,{description:'must time out'},503);
  check('lock contention is bounded to five seconds',Date.now()-waitStarted>=4500&&Date.now()-waitStarted<6500);
  await longWrite.task;
  check('timed-out department write leaves no partial changes',sql(`SELECT description FROM departments WHERE id=${q(department.id)}`),'atomic test');

  const manifest = JSON.parse(await readFile('llm-plugin/examples/openai-compatible.provider.json','utf8'));
  manifest.id='staging-fixture';manifest.name='Staging fixture';manifest.capabilities.modelListing=true;delete manifest.operations.listModels;
  manifest.models=['staging-fast','staging-failure','staging-slow'].map(id=>({id,name:id,modelType:'text_generator',capabilities:{chat:manifest.capabilities.chat},metadata:{inputTokenRate:0,outputTokenRate:0}}));
  await expect('POST','/admin/ai-engines',{is_enabled:true,api_key:'local-only',whitelisted_models:manifest.models.map(m=>m.id),default_model:'staging-fast',plugin_config:{manifest,configuration:{},baseUrlOverride:`http://127.0.0.1:${provider.address().port}/`,allowInsecureHttp:true,allowPrivateNetwork:true}},201);
  async function createChat(bearer,text) {
    const response = await expect('POST','/chat/stream',payload(text),200,bearer);
    assert(!response.text.includes('ai_error'));
    const id = events(response.text).find(event=>event.is_new)?.id;
    assert.match(id??'',/^[0-9a-f-]{36}$/);return id;
  }
  const chat = await createChat(owner,'original staging question');
  const otherChat = await createChat(viewer,'viewer private title');
  const project = (await expect('POST','/projects',{name:'Staging team',visibility:'team'},201,owner)).json();
  await expect('POST',`/projects/${project.id}/members`,{user_id:viewerId},201,owner);
  await expect('POST',`/conversations/${chat}/projects`,{project_id:project.id},201,owner);
  await expect('POST',`/conversations/${otherChat}/projects`,{project_id:project.id},201,viewer);
  for (const [bearer,id] of [[owner,chat],[viewer,otherChat]]) {
    const detail = (await expect('GET',`/projects/${project.id}/detail`,undefined,200,bearer)).json();
    check('team project never leaks another member private chat',detail.chats.map(c=>c.id),[id]);
  }
  const userMessage = sql(`SELECT id FROM messages WHERE "conversationId"=${q(chat)} AND role='user' LIMIT 1`);
  const visible = () => sql(`SELECT coalesce(string_agg(id::text,',' ORDER BY "createdAt",id),'') FROM messages WHERE "conversationId"=${q(chat)} AND NOT deleted AND NOT "inactiveBranch"`);
  const before = visible();
  const counters = () => sql(`SELECT "messageCount"::text||':'||"lastMessageAt"::text FROM conversations WHERE id=${q(chat)}`);
  const beforeCounters = counters();
  const failed = await expect('PATCH',`/chat/${chat}/message/${userMessage}/stream`,payload('failed attempt','staging-failure'),200,owner);
  check('provider failure emits an error',failed.text.includes('ai_error'));
  check('failed edit preserves visible history and counters',[visible(),counters()],[before,beforeCounters]);
  await expect('PATCH',`/chat/${chat}/message/${userMessage}/stream`,payload('foreign attempt'),404,viewer);
  const share = (await expect('POST',`/chat/${chat}/shares`,{access:'organization'},201,owner)).json();
  const edited = await expect('PATCH',`/chat/${chat}/message/${userMessage}/stream`,payload('edited staging question'),200,owner);
  check('successful edit has no provider error',edited.text.includes('ai_error'),false);
  const editedMessage = events(edited.text).find(event=>event.user_message_id)?.user_message_id;
  assert.match(editedMessage??'',/^[0-9a-f-]{36}$/);
  const view = (await expect('GET',`/chat/${chat}`,undefined,200,owner)).json();
  const version = view.messages.find(message=>message.id===editedMessage)?.versions;
  check('successful edit exposes two versions',version?.total,2);
  await expect('PUT',`/chat/${chat}/branch`,{message_id:userMessage},200,owner);
  check('switch restores original thread',visible(),before);
  await expect('PUT',`/chat/${chat}/branch`,{message_id:editedMessage},404,viewer);
  const sharedPath = new URL(share.share_url).pathname;
  const frozen = await expect('GET',sharedPath,undefined,200,viewer);
  check('share remains a frozen pre-edit snapshot',frozen.text.includes('original staging question')&&!frozen.text.includes('edited staging question'));
  await expect('PUT',`/chat/${chat}/branch`,{message_id:editedMessage},200,owner);
  await fault('messages','INSERT',`IF NEW.role='assistant' AND NEW."modelName"='staging-fast' THEN RAISE EXCEPTION 'staging save failure'; END IF`,async()=>{
    const savedBefore = visible();
    const result = await expect('PATCH',`/chat/${chat}/message/${editedMessage}/stream`,payload('save failure'),200,owner);
    check('reply persistence failure emits exactly one error',(result.text.match(/^event: "ai_error"$/gm)??[]).length,1);
    check('reply persistence failure preserves visible history',visible(),savedBefore);
  });
  const slow = await request('POST',`/chat/stream/${chat}`,payload('cancel this turn','staging-slow'),owner);
  assert.equal(slow.status,200);
  const reader = slow.body.getReader(), decoder = new TextDecoder();let received='', assistantId;
  while (!assistantId) {
    const {done,value}=await reader.read();assert(!done,'slow stream ended before message_start');received+=decoder.decode(value,{stream:true});
    assistantId=events(received).find(event=>event.message_id&&event.user_message_id)?.message_id;
  }
  await expect('POST',`/chat/stream/${assistantId}/cancel`,undefined,404,viewer);
  await expect('POST',`/chat/stream/${assistantId}/cancel`,undefined,202,owner);
  while (true) {const {done,value}=await reader.read();if(done)break;received+=decoder.decode(value,{stream:true});}
  check('owner cancellation reports user_cancelled',received.includes('user_cancelled'));
  check('owner cancellation persists stop reason',sql(`SELECT metadata->>'stopReason' FROM messages WHERE id=${q(assistantId)}`),'user_cancelled');
  const disconnected=await request('POST',`/chat/stream/${chat}`,payload('disconnect then Stop','staging-slow'),owner);
  const disconnectedReader=disconnected.body.getReader();let disconnectedText='',disconnectedId;
  while(!disconnectedId) {
    const {done,value}=await disconnectedReader.read();assert(!done);
    disconnectedText+=decoder.decode(value,{stream:true});
    disconnectedId=events(disconnectedText).find(event=>event.message_id&&event.user_message_id)?.message_id;
  }
  await disconnectedReader.cancel();await delay(250);
  await expect('POST',`/chat/stream/${disconnectedId}/cancel`,undefined,202,owner);
  await waitFor(()=>sql(`SELECT metadata->>'stopReason' FROM messages WHERE id=${q(disconnectedId)}`)==='user_cancelled','Stop after disconnect');
  check('Stop after disconnect persists as cancellation, not network error',true);

  const serverId=randomUUID();
  const oauth={auth_url:`http://127.0.0.1:${provider.address().port}/authorize`,token_url:`http://127.0.0.1:${provider.address().port}/token`,redirect_url:'http://localhost:5173/mcp/oauth/callback',scopes:[],flow:'oauth2',use_pkce:true};
  sql(`INSERT INTO mcp_servers (id,name,"transportType","connectionConfig","clientId",enabled,"toolCount","defaultAccess","createdAt","updatedAt") VALUES (${q(serverId)},'Staging OAuth','http',${j({oauth})},'staging-client',false,0,'admin_only',now(),now())`);
  const state = () => {
    const value=randomUUID();
    sql(`INSERT INTO mcp_oauth_states (id,"serverId","userId",state,"pkceVerifier","expiresAt","createdAt") VALUES (${q(randomUUID())},${q(serverId)},${q(ownerId)},${q(value)},'fixture-verifier',now()+interval '5 minutes',now())`);return value;
  };
  const callback = (value,code='accepted') => `/mcp/oauth/callback?state=${value}&code=${code}`;
  const exists = value => sql(`SELECT count(*) FROM mcp_oauth_states WHERE state=${q(value)}`);
  const wrongUser = state();
  await expect('GET',callback(wrongUser),undefined,404,viewer);
  check('wrong-user OAuth callback never consumes state',exists(wrongUser),'1');
  await expect('GET',callback(wrongUser),undefined,200,owner);
  check('successful exchange consumes state',exists(wrongUser),'0');
  check('MCP tokens stored encrypted',sql(`SELECT "accessToken" <> 'fixture-access' AND "refreshToken" <> 'fixture-refresh' FROM mcp_connections WHERE "serverId"=${q(serverId)} AND "userId"=${q(ownerId)}`),'t');
  const callsBefore=tokenCalls.length;
  await expect('GET',callback(wrongUser),undefined,404,owner);
  check('OAuth replay rejected before token exchange',tokenCalls.length,callsBefore);
  const rejected=state();await expect('GET',callback(rejected,'rejected'),undefined,503,owner);
  check('provider rejection consumes state',exists(rejected),'0');
  const expired=state();sql(`UPDATE mcp_oauth_states SET "expiresAt"=now()-interval '1 minute' WHERE state=${q(expired)}`);
  await expect('GET',callback(expired),undefined,404,owner);check('expired state removed',exists(expired),'0');
  const unavailable=createServer();await new Promise(resolve=>unavailable.listen(0,'127.0.0.1',resolve));
  const unusedPort=unavailable.address().port;await new Promise(resolve=>unavailable.close(resolve));
  sql(`UPDATE mcp_servers SET "connectionConfig"=${j({oauth:{...oauth,token_url:`http://127.0.0.1:${unusedPort}/token`}})} WHERE id=${q(serverId)}`);
  const retry=state();await expect('GET',callback(retry),undefined,503,owner);check('unreachable token endpoint keeps retry state',exists(retry),'1');
  sql(`UPDATE mcp_servers SET "connectionConfig"=${j({oauth})} WHERE id=${q(serverId)}`);
  await expect('GET',callback(retry),undefined,200,owner);check('same callback succeeds after endpoint recovery',exists(retry),'0');
  const failedStore=state();
  await fault('mcp_connections','UPDATE',"RAISE EXCEPTION 'staging token store failure'",async()=>{
    await expect('GET',callback(failedStore),undefined,503,owner);
    check('failed token store also rolls back state deletion',exists(failedStore),'1');
  });
  await expect('GET',callback(failedStore,'rejected'),undefined,503,owner);
  check('spent-code rejection cleans rolled-back state',exists(failedStore),'0');
  console.log(`Extended single-instance HTTP regressions: ${checks} passed`);
} finally {provider.closeAllConnections();await new Promise(resolve=>provider.close(resolve));}
