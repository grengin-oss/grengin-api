import assert from 'node:assert/strict';
import { createHmac, randomUUID } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { readdir, readFile, open, unlink } from 'node:fs/promises';
import { constants } from 'node:fs';
import { createServer } from 'node:http';

const db = new URL(process.env.DATABASE_URL);
assert.equal(db.hostname, '127.0.0.1');
assert.equal(db.pathname, '/grengin_security_test', 'refusing a non-disposable database');
const base = process.env.GRENGIN_SECURITY_TEST_URL;
assert.equal(new URL(base).hostname, '127.0.0.1');
const sql = query => execFileSync('psql', ['-X', '-A', '-t', '-v', 'ON_ERROR_STOP=1', process.env.DATABASE_URL, '-c', query], {encoding:'utf8', stdio:['ignore','pipe','pipe']}).trim();
const q = value => `'${String(value).replaceAll("'", "''")}'`;
const j = value => `${q(JSON.stringify(value))}::jsonb`;
function token(user_id, token_use = 'access') {
  const claims = {sub:'share-test@example.invalid',user_id,name:'Share test',exp:Math.floor(Date.now()/1000)+1800,token_use};
  const body = [Buffer.from(JSON.stringify({alg:'HS256',typ:'JWT'})).toString('base64url'),Buffer.from(JSON.stringify(claims)).toString('base64url')].join('.');
  return `${body}.${createHmac('sha256',process.env.JWT_SECRET).update(body).digest('base64url')}`;
}
const adminId = sql(`SELECT a."userId" FROM user_role_assignments a JOIN roles r ON r.id=a."roleId" JOIN users u ON u.id=a."userId" WHERE r.name='Super Admin' AND a."scopeDepartmentId" IS NULL AND u.status='active' LIMIT 1`);
const admin = token(adminId);
let defaultViewer;
const notificationFeeds = [];
let checks = 0;
function check(name, actual, expected = true) {assert.deepEqual(actual,expected,name);console.log(`PASS share ${++checks}: ${name}`);}
async function req(method,path,body,bearer) {
  if (bearer === undefined && path.startsWith('/share/')) bearer = defaultViewer;
  const response = await fetch(`${base}${path}`,{method,headers:{'content-type':'application/json',...(bearer ? {authorization:`Bearer ${bearer}`} : {})},body:body===undefined ? undefined : JSON.stringify(body),signal:AbortSignal.timeout(45000)});
  const text = await response.text();
  return {status:response.status,text,headers:response.headers,json:()=>JSON.parse(text)};
}
async function expect(method,path,body,status,bearer) {
  const result = await req(method,path,body,bearer);
  assert.equal(result.status,status,`${method} ${path}: ${result.status}: ${result.text.slice(0,200)}`);
  checks++; return result;
}
async function newUser() {
  const email = `share-${randomUUID()}@example.invalid`;
  await expect('POST','/admin/users',{name:'Share test',email},201,admin);
  const id = sql(`SELECT id FROM users WHERE email=${q(email)}`);
  await expect('PATCH',`/admin/users/${id}/status`,{status:'active'},200,admin);
  return id;
}
async function openNotificationFeed(bearer) {
  const controller = new AbortController();
  const response = await fetch(`${base}/me/notifications/stream`,{headers:{authorization:`Bearer ${bearer}`},signal:controller.signal});
  assert.equal(response.status,200);
  const reader = response.body.getReader(), events = [], decoder = new TextDecoder();
  const task = (async()=>{
    let pending = '';
    try {
      while (true) {
        const {done,value} = await reader.read(); if (done) break;
        pending += decoder.decode(value,{stream:true}).replaceAll('\r\n','\n');
        let boundary;
        while ((boundary=pending.indexOf('\n\n'))!==-1) {
          const block = pending.slice(0,boundary); pending=pending.slice(boundary+2);
          const data = block.split('\n').filter(line=>line.startsWith('data:')).map(line=>line.slice(5).trimStart()).join('\n');
          if (data) events.push(JSON.parse(data));
        }
      }
    } catch (error) { if (!controller.signal.aborted) throw error; }
  })();
  const feed = {events,close:async()=>{controller.abort();await task;}};
  notificationFeeds.push(feed);
  return feed;
}
async function waitForNotification(feed,shareId) {
  for (let index=0;index<400;index++) {
    const notification = feed.events.find(event=>event.payload?.share_id===shareId);
    if (notification) return notification;
    await new Promise(resolve=>setTimeout(resolve,10));
  }
  assert.fail('committed sharing notification was not delivered over SSE');
}
async function diskFiles() {
  return (await readdir(process.env.FILE_STORAGE_ROOT,{recursive:true,withFileTypes:true})).filter(entry=>entry.isFile()).map(entry=>`${entry.parentPath}/${entry.name}`).sort();
}
const fixture = createServer(async (request,response) => {
  for await (const _ of request) { /* Drain the request before replying. */ }
  response.writeHead(200,{'content-type':'text/event-stream'});
  response.write(`data: ${JSON.stringify({choices:[{delta:{content:'private continuation fixture'}}]})}\n\n`);
  response.write(`data: ${JSON.stringify({choices:[{delta:{},finish_reason:'stop'}],usage:{prompt_tokens:1,completion_tokens:1,total_tokens:2}})}\n\n`);
  response.end('data: [DONE]\n\n');
});
await new Promise(resolve=>fixture.listen(0,'127.0.0.1',resolve));
try {
  const openapi=(await expect('GET','/openapi.json',undefined,200)).json();
  for(const [path,method] of [
    ['/chat/{chat_id}/shares','post'],['/chat/{chat_id}/shares','get'],
    ['/chat/{chat_id}/shares/{share_id}','put'],['/chat/{chat_id}/shares/{share_id}','delete'],
    ['/share/{token}','get'],['/share/{token}/files/{file_id}','get'],
    ['/share/{token}/fork','post'],['/me/shared-chats/{share_id}/link','get'],
  ]) {
    const operation=openapi.paths[path]?.[method];
    assert(operation?.summary && operation?.description,`${method} ${path} is missing documentation`);
    for(const status of ['401','404','429','503']) assert(operation.responses[status]?.description,`${method} ${path} is missing ${status}`);
  }
  check('all eight sharing OpenAPI operations explain access and failures',true);
  check('share create Swagger includes executable examples',Boolean(openapi.paths['/chat/{chat_id}/shares'].post.requestBody.content['application/json'].examples.departments));
  const migrator = `${process.env.CARGO_TARGET_DIR || 'target-security'}/debug/migration`;
  execFileSync(migrator,['down','-n','1'],{env:process.env,stdio:['ignore','pipe','pipe']});
  check('down migration removes shares and share permission',sql(`SELECT to_regclass('public.chat_shares') IS NULL AND NOT EXISTS(SELECT 1 FROM permissions WHERE domain='chats' AND action='share')`),'t');
  await expect('GET','/',undefined,503);
  execFileSync(migrator,['up'],{env:process.env,stdio:['ignore','pipe','pipe']});
  await expect('GET','/',undefined,200);
  check('up migration restores the share schema',sql(`SELECT to_regclass('public.chat_shares') IS NOT NULL`),'t');
  const ownerId = await newUser(), viewerId = await newUser();
  const owner = token(ownerId), viewer = token(viewerId);
  defaultViewer = viewer;
  const outsiderId = await newUser(), outsider = token(outsiderId);
  const createDepartment = async (parent_id) => (await expect('POST','/admin/departments',{name:`Share ${randomUUID()}`,description:'share scope',parent_id},201,admin)).json().id;
  const department = await createDepartment();
  const childDepartment = await createDepartment(department);
  const otherDepartment = await createDepartment();
  sql(`UPDATE users SET "departmentId"=${q(department)} WHERE id=${q(viewerId)}`);
  const ineligibleIds = [];
  for (const status of ['deactivated','pending','suspended','deleted']) {
    const id = await newUser(); ineligibleIds.push(id);
    sql(`UPDATE users SET status=${q(status)},"departmentId"=${q(department)} WHERE id=${q(id)}`);
  }
  check('User role receives chats:share',sql(`SELECT count(*) FROM role_permissions rp JOIN permissions p ON p.id=rp."permissionId" JOIN roles r ON r.id=rp."roleId" WHERE r.name='User' AND p.domain='chats' AND p.action='share'`),'1');
  const manifest = JSON.parse(await readFile('llm-plugin/examples/openai-compatible.provider.json','utf8'));
  manifest.id = 'share-fixture'; manifest.name = 'Share fixture'; manifest.capabilities.modelListing=true; delete manifest.operations.listModels;
  manifest.models=[{id:'share-chat',name:'Share chat',modelType:'text_generator',capabilities:{chat:manifest.capabilities.chat},metadata:{inputTokenRate:0,outputTokenRate:0}}];
  await expect('POST','/admin/ai-engines',{is_enabled:true,api_key:'local-only',whitelisted_models:['share-chat'],default_model:'share-chat',plugin_config:{manifest,configuration:{},baseUrlOverride:`http://127.0.0.1:${fixture.address().port}/`,allowInsecureHttp:true,allowPrivateNetwork:true}},201,admin);
  const input = {provider:'share-fixture',model_name:'share-chat',messages:[{role:'user',content:'original question',files:[]}]};
  const stream = await expect('POST','/chat/stream',input,200,owner);
  assert(!stream.text.includes('ai_error'));
  const events = [...stream.text.matchAll(/^data: (.+)$/gm)].flatMap(match=>{try{return[JSON.parse(match[1])];}catch{return[];}});
  const chat = events.find(e=>e.is_new===true)?.id;
  assert.match(chat ?? '',/^[0-9a-f-]{36}$/);
  const message = sql(`SELECT id FROM messages WHERE "conversationId"=${q(chat)} AND role='user' LIMIT 1`);
  const reply = sql(`SELECT id FROM messages WHERE "conversationId"=${q(chat)} AND role='assistant' LIMIT 1`);
  const project = (await expect('POST','/projects',{name:'private source project',instructions:'private project instructions'},201,owner)).json();
  sql(`INSERT INTO conversation_projects (id,"conversationId","projectId","createdAt") VALUES (${q(randomUUID())},${q(chat)},${q(project.id)},now())`);
  sql(`INSERT INTO conversation_summaries (id,"conversationId",summary,"lastMessageId","lastMessageAt","createdAt","updatedAt") VALUES (${q(randomUUID())},${q(chat)},'private source summary',${q(reply)},now(),now(),now())`);
  const upload = async (name,bearer=owner) => (await expect('POST','/files',{attachment:{file:Buffer.from('<html>shared attachment</html>').toString('base64'),name,type:'text/html'}},200,bearer)).json();
  const file = await upload('share.html'), foreign = await upload('foreign.html',viewer);
  const inlineId = randomUUID(), artifact = randomUUID();
  const attachment = {id:file.id,name:file.name,type:'text/html'};
  const inline = {id:inlineId,name:'inline.txt',type:'text/plain',base64:Buffer.from('inline contents').toString('base64')};
  sql(`UPDATE messages SET metadata=${j({files:[attachment,inline],private:'private metadata',webSearch:true})},"toolsCalls"=ARRAY[${j({secret:'private tool args'})}],"toolsResults"=ARRAY[${j({secret:'private tool results'})}],"requestId"='private-request' WHERE id=${q(message)}`);
  sql(`INSERT INTO artifacts (id,"fileId","messageId","conversationId",title,"contentType","createdAt","updatedAt") VALUES (${q(artifact)},${q(file.id)},${q(reply)},${q(chat)},'Artifact','text/html',now(),now())`);
  // Clone a row to exercise exclusion without relying on model output or a UI branch switch.
  sql(`INSERT INTO messages SELECT (jsonb_populate_record(NULL::messages,to_jsonb(m)||${j({id:randomUUID(),previousMessageId:null,messageContent:'hidden branch secret',inactiveBranch:true})})).* FROM messages m WHERE m.id=${q(message)}`);
  sql(`INSERT INTO messages SELECT (jsonb_populate_record(NULL::messages,to_jsonb(m)||${j({id:randomUUID(),previousMessageId:null,role:'system',messageContent:'private system prompt',metadata:null})})).* FROM messages m WHERE m.id=${q(message)}`);
  await expect('POST',`/chat/${chat}/shares`,{},401);
  await expect('POST',`/chat/${chat}/shares`,{},404,viewer);
  await expect('POST',`/chat/${chat}/shares`,{access:'invalid'},422,owner);
  await expect('POST',`/chat/${chat}/shares`,{expires_at:'2000-01-01T00:00:00Z'},400,owner);
  const [viewerFeed,outsiderFeed,ownerFeed] = await Promise.all([openNotificationFeed(viewer),openNotificationFeed(outsider),openNotificationFeed(owner)]);
  const org = (await expect('POST',`/chat/${chat}/shares`,{},201,owner)).json();
  check('organization links are silent by default',sql(`SELECT count(*) FROM notifications WHERE "shareId"=${q(org.id)}`),'0');
  await expect('PUT',`/chat/${chat}/shares/${org.id}`,{notify:true},403,owner);
  const superRole = sql(`SELECT id FROM roles WHERE name='Super Admin' LIMIT 1`);
  const notifyGrant = (await expect('POST',`/admin/users/${ownerId}/roles`,{role_id:superRole},201,admin)).json();
  await expect('PUT',`/chat/${chat}/shares/${org.id}`,{notify:true},200,owner);
  await expect('DELETE',`/admin/users/${ownerId}/roles/${notifyGrant.id}`,undefined,204,admin);
  await waitForNotification(viewerFeed,org.id);
  await waitForNotification(outsiderFeed,org.id);
  check('explicit admin broadcast durably notifies every active non-owner',sql(`SELECT count(*) FROM notifications WHERE "shareId"=${q(org.id)}`),sql(`SELECT count(*) FROM users WHERE status='active' AND id<>${q(ownerId)}`));
  check('owner and inactive users receive no share notification',sql(`SELECT count(*) FROM notifications WHERE "shareId"=${q(org.id)} AND "userId" IN (${[ownerId,...ineligibleIds].map(q).join(',')})`),'0');
  const tokenOf = link => new URL(link.share_url).pathname.split('/').at(-1);
  const orgToken = tokenOf(org), orgPath = `/share/${orgToken}`;
  check('access defaults to organization',org.access,'organization');
  await expect('GET',orgPath,undefined,401,'');
  await expect('GET',orgPath,undefined,200,viewer);
  await expect('GET',orgPath,undefined,401,token(viewerId,'refresh'));
  for (const body of [{access:'public'},{access:'users'},{access:'roles'},{user_ids:[viewerId]},{role_ids:[randomUUID()]}]) {
    await expect('POST',`/chat/${chat}/shares`,body,422,owner);
  }
  for (const body of [{access:'department'},{access:'department',department_ids:[]},{access:'organization',department_ids:[department]}]) {
    await expect('POST',`/chat/${chat}/shares`,body,400,owner);
  }
  await expect('POST',`/chat/${chat}/shares`,{access:'department',department_ids:[randomUUID()]},404,owner);
  const departmentLink = (await expect('POST',`/chat/${chat}/shares`,{access:'department',department_ids:[department,department]},201,owner)).json();
  const delivered = await waitForNotification(viewerFeed,departmentLink.id);
  await new Promise(resolve=>setTimeout(resolve,100));
  check('SSE is delivered only to eligible users',outsiderFeed.events.some(event=>event.payload?.share_id===departmentLink.id) || ownerFeed.events.some(event=>event.payload?.share_id===departmentLink.id),false);
  check('department share excludes owner, outsiders and inactive accounts',sql(`SELECT string_agg("userId"::text,',') FROM notifications WHERE "shareId"=${q(departmentLink.id)}`),viewerId);
  check('notification contains only a share identifier, not a bearer link',delivered.payload,{share_id:departmentLink.id});
  for (const feed of notificationFeeds) await feed.close();
  check('department targets are deduplicated',departmentLink.department_ids,[department]);
  const departmentToken = tokenOf(departmentLink), path = `/share/${departmentToken}`;
  const notice = (await expect('GET','/me/notifications?limit=100',undefined,200,viewer)).json().notifications.find(notification=>notification.payload?.share_id===departmentLink.id);
  assert(notice);
  for (const secret of [departmentToken,chat,'original question',file.name]) assert(!JSON.stringify(notice).includes(secret));
  await expect('POST',`/me/notifications/${notice.id}/read`,undefined,204,viewer);
  await expect('POST',`/me/notifications/${notice.id}/read`,undefined,404,outsider);
  const resolved = await expect('GET',`/me/shared-chats/${departmentLink.id}/link`,undefined,200,viewer);
  check('notification action resolves the current share link',resolved.json().share_url,departmentLink.share_url);
  check('resolved links are not cacheable',resolved.headers.get('cache-control'),'private, no-store');
  await expect('GET',`/me/shared-chats/${departmentLink.id}/link`,undefined,401);
  await expect('GET',`/me/shared-chats/${departmentLink.id}/link`,undefined,404,outsider);
  const shared = await expect('GET',path,undefined,200);
  await expect('GET',path,undefined,401,'');
  await expect('GET',`${path}/files/${file.id}`,undefined,401,'');
  await expect('GET',path,undefined,404,outsider);
  const departmentAdminRole = sql(`SELECT id FROM roles WHERE name='Department Admin' LIMIT 1`);
  await expect('POST',`/admin/users/${outsiderId}/roles`,{role_id:departmentAdminRole,scope_department_id:department},201,admin);
  await expect('GET',path,undefined,404,outsider);
  await expect('GET',`${path}/files/${file.id}`,undefined,404,outsider);
  await expect('POST',`${path}/fork`,undefined,404,outsider);
  await expect('GET',path,undefined,200,owner);
  sql(`UPDATE users SET "departmentId"=${q(childDepartment)} WHERE id=${q(viewerId)}`);
  await expect('GET',path,undefined,404,viewer);
  await expect('GET',`/me/shared-chats/${departmentLink.id}/link`,undefined,404,viewer);
  sql(`UPDATE users SET "departmentId"=${q(otherDepartment)} WHERE id=${q(viewerId)}`);
  await expect('GET',path,undefined,404,viewer);
  sql(`UPDATE users SET "departmentId"=${q(department)} WHERE id=${q(viewerId)}`);
  const snapshot = shared.json();
  check('only visible user and assistant messages are shared',snapshot.chat.messages.map(m=>m.role),['user','assistant']);
  for (const secret of ['private metadata','private tool args','private tool results','private-request','hidden branch secret','private system prompt','private source summary','private project instructions',project.id,inline.base64]) assert(!shared.text.includes(secret));
  check('private metadata, tools and inline bytes are not returned',true);
  check('artifact content is snapshotted',snapshot.chat.messages[1].artifacts[0].content,'<html>shared attachment</html>');
  check('signed-in response privacy headers',[shared.headers.get('cache-control'),shared.headers.get('referrer-policy'),shared.headers.get('x-content-type-options')],['private, no-store','no-referrer','nosniff']);
  await expect('GET',path,undefined,401,'invalid');
  await expect('GET',path,undefined,401,token(viewerId,'refresh'));
  await expect('GET',path,undefined,401,token(randomUUID()));
  await expect('GET',`${path}/files/${foreign.id}`,undefined,404);
  await expect('GET',`${path}/files/${randomUUID()}`,undefined,404);
  const bytes = await expect('GET',`${path}/files/${file.id}`,undefined,200);
  check('HTML share download is attachment-only',[bytes.text,bytes.headers.get('content-disposition').startsWith('attachment'),bytes.headers.get('x-content-type-options')],['<html>shared attachment</html>',true,'nosniff']);
  check('inline attachment can be downloaded',(await expect('GET',`${path}/files/${inlineId}`,undefined,200)).text,'inline contents');
  const originalPath = sql(`SELECT "localPath" FROM files WHERE id=${q(file.id)}`);
  sql(`UPDATE files SET "localPath"='/etc/hostname' WHERE id=${q(file.id)}`);
  await expect('GET',`${path}/files/${file.id}`,undefined,404);
  const unsafeCopy = (await expect('POST',`${path}/fork`,undefined,201,owner)).json().conversation_id;
  check('unsafe source path is omitted rather than blocking the private copy',sql(`SELECT count(*) FROM files f JOIN artifacts a ON a."fileId"=f.id WHERE a."conversationId"=${q(unsafeCopy)}`),'0');
  sql(`UPDATE files SET "localPath"=${q(originalPath)} WHERE id=${q(file.id)}`);
  await expect('GET',`/files/${file.id}/download`,undefined,404,viewer);
  sql(`UPDATE messages SET "messageContent"='modified after sharing' WHERE id=${q(message)}`);

  sql(`UPDATE conversations SET "archivedAt"=now() WHERE id=${q(chat)}`);
  await expect('POST',`/chat/${chat}/shares`,{},404,owner);
  sql(`UPDATE conversations SET "archivedAt"=NULL WHERE id=${q(chat)}`);
  check('snapshot does not follow live message changes',(await expect('GET',path,undefined,200)).json().chat.messages[0].text,'original question');
  await expect('PUT',`/chat/${chat}/shares/${departmentLink.id}`,{refresh_snapshot:true},404,viewer);
  await expect('PUT',`/chat/${chat}/shares/${departmentLink.id}`,{expires_at:new Date(Date.now()+60000).toISOString(),clear_expiry:true},400,owner);
  await expect('PUT',`/chat/${chat}/shares/${departmentLink.id}`,{refresh_snapshot:true},200,owner);
  check('refresh neither duplicates notices nor resets read state',sql(`SELECT count(*)||':'||bool_and("readAt" IS NOT NULL)::text FROM notifications WHERE "shareId"=${q(departmentLink.id)} AND "userId"=${q(viewerId)}`),'1:true');
  check('refresh keeps the same notification id',sql(`SELECT id FROM notifications WHERE "shareId"=${q(departmentLink.id)} AND "userId"=${q(viewerId)}`),notice.id);
  check('explicit refresh replaces the snapshot',(await expect('GET',path,undefined,200)).json().chat.messages[0].text,'modified after sharing');
  const encrypted = JSON.parse(sql(`SELECT json_build_object('hash',"tokenHash",'token',token)::text FROM chat_shares WHERE id=${q(departmentLink.id)}`));
  check('tokens stored encrypted with hash lookup',encrypted.token!==departmentToken && encrypted.hash.length===64 && !encrypted.token.includes(departmentToken));
  await expect('GET',`/chat/${chat}/shares`,undefined,404,viewer);
  check('owner can list stable share URLs',(await expect('GET',`/chat/${chat}/shares`,undefined,200,owner)).json().some(link=>link.share_url===departmentLink.share_url));
  check('owner link lists cannot be cached',(await expect('GET',`/chat/${chat}/shares`,undefined,200,owner)).headers.get('cache-control'),'private, no-store');
  await expect('PUT',`/chat/${chat}/shares/${departmentLink.id}`,{access:'organization'},200,owner);
  check('ordinary organization scope expansion does not broadcast',sql(`SELECT count(*) FROM notifications WHERE "shareId"=${q(departmentLink.id)} AND "userId"=${q(outsiderId)}`),'0');
  await expect('GET',path,undefined,401,'');
  await expect('GET',path,undefined,200,outsider);
  await expect('PUT',`/chat/${chat}/shares/${departmentLink.id}`,{access:'public'},422,owner);
  await expect('PUT',`/chat/${chat}/shares/${departmentLink.id}`,{access:'department'},400,owner);
  await expect('PUT',`/chat/${chat}/shares/${departmentLink.id}`,{access:'department',department_ids:[otherDepartment]},200,owner);
  check('scope restriction retains dedupe/read history',sql(`SELECT id FROM notifications WHERE "shareId"=${q(departmentLink.id)} AND "userId"=${q(viewerId)}`),notice.id);
  const restrictedInbox=(await expect('GET','/me/notifications?limit=100',undefined,200,viewer)).json();
  check('scope restriction hides ineligible inbox entries',restrictedInbox.notifications.some(n=>n.payload?.share_id===departmentLink.id),false);
  await expect('GET',path,undefined,404,viewer);
  await expect('PUT',`/chat/${chat}/shares/${departmentLink.id}`,{department_ids:[department,otherDepartment]},200,owner);
  const restoredInbox=(await expect('GET','/me/notifications?limit=100',undefined,200,viewer)).json();
  const restoredNotice=restoredInbox.notifications.find(n=>n.payload?.share_id===departmentLink.id);
  check('scope toggles preserve notice identity and read state',[restoredNotice?.id,Boolean(restoredNotice?.read_at)],[notice.id,true]);
  await expect('GET',path,undefined,200,viewer);
  sql(`UPDATE users SET "departmentId"=${q(otherDepartment)} WHERE id=${q(outsiderId)}`);
  await expect('GET',path,undefined,200,outsider);
  await expect('PUT',`/chat/${chat}/shares/${departmentLink.id}`,{department_ids:[department]},200,owner);
  await expect('GET',path,undefined,404,outsider);
  sql(`INSERT INTO chat_shares SELECT (jsonb_populate_record(NULL::chat_shares,to_jsonb(s)||jsonb_build_object('id',gen_random_uuid(),'tokenHash',repeat(md5(gen_random_uuid()::text),2),'snapshot',s.snapshot||'{"test_limit":true}'::jsonb))).* FROM chat_shares s CROSS JOIN generate_series(1,97) WHERE s.id=${q(departmentLink.id)}`);
  const racing = await Promise.all([req('POST',`/chat/${chat}/shares`,{},owner),req('POST',`/chat/${chat}/shares`,{},owner)]);
  check('concurrent creates cannot exceed the share limit',sql(`SELECT count(*) FROM chat_shares WHERE "conversationId"=${q(chat)}`),'100');
  check('one concurrent create succeeds, the other fails safely',racing.filter(response=>response.status===201).length,1);
  assert(racing.every(response=>[201,400,503].includes(response.status)));
  sql(`DELETE FROM chat_shares WHERE snapshot->>'test_limit'='true' OR id=${q(racing.find(response=>response.status===201).json().id)}`);

  await expect('POST',`${path}/fork`,undefined,401,'');
  const beforeFork = sql(`SELECT count(*) FROM messages WHERE "conversationId"=${q(chat)}`);
  const fork = (await expect('POST',`${path}/fork`,undefined,201,viewer)).json().conversation_id;
  assert.notEqual(fork,chat);
  await expect('GET',`/chat/${fork}`,undefined,404,owner);
  const forked = (await expect('GET',`/chat/${fork}`,undefined,200,viewer)).json();
  check('fork contains independent message identities',forked.messages.length===2 && forked.messages.every(m=>![message,reply].includes(m.id)));
  check('fork has a fresh linear chain',sql(`SELECT count(*) FROM messages m JOIN messages p ON p.id=m."previousMessageId" WHERE m."conversationId"=${q(fork)} AND p."conversationId"=${q(fork)}`),'1');
  check('fork has no private associations or copied RAG state',sql(`SELECT (SELECT count(*) FROM conversation_projects WHERE "conversationId"=${q(fork)})+(SELECT count(*) FROM conversation_skills WHERE "conversationId"=${q(fork)})+(SELECT count(*) FROM conversation_summaries WHERE "conversationId"=${q(fork)})+(SELECT count(*) FROM message_embeddings WHERE "conversationId"=${q(fork)})`),'0');
  check('fork usage and user-turn count reset',sql(`SELECT "messageCount"||':'||"totalTokens"||':'||("totalCost"=0)::text FROM conversations WHERE id=${q(fork)}`),'1:0:true');
  const forkFile = sql(`SELECT metadata->'files'->0->>'id' FROM messages WHERE "conversationId"=${q(fork)} AND role='user'`);
  assert.notEqual(forkFile,file.id);
  await expect('GET',`/files/${forkFile}/download`,undefined,200,viewer);
  check('copied file belongs to viewer',sql(`SELECT "userId" FROM files WHERE id=${q(forkFile)}`),viewerId);
  const forkArtifact = sql(`SELECT "fileId" FROM artifacts WHERE "conversationId"=${q(fork)}`);
  check('artifact references the private file copy',forkArtifact,forkFile);
  const continued = await expect('POST',`/chat/stream/${fork}`,input,200,viewer);
  assert(!continued.text.includes('ai_error'),continued.text);
  check('private copy can continue without writing source',sql(`SELECT count(*) FROM messages WHERE "conversationId"=${q(chat)}`),beforeFork);

  const dbCounts = () => sql(`SELECT (SELECT count(*) FROM files)||':'||(SELECT count(*) FROM conversations)||':'||(SELECT count(*) FROM messages)||':'||(SELECT count(*) FROM artifacts)`);
  let before = dbCounts(), disk = await diskFiles();
  sql(`CREATE FUNCTION share_fail_audit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.event LIKE 'chat.share_%' THEN RAISE EXCEPTION 'forced audit failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER share_fail_audit BEFORE INSERT ON auth_audit_events FOR EACH ROW EXECUTE FUNCTION share_fail_audit()`);
  await expect('POST',`${path}/fork`,undefined,503,viewer);
  check('failed fork rolls back all rows and copied bytes',[dbCounts(),await diskFiles()],[before,disk]);
  const shareCount = sql('SELECT count(*) FROM chat_shares');
  const notificationCount = sql('SELECT count(*) FROM notifications');
  await expect('POST',`/chat/${chat}/shares`,{access:'department',department_ids:[department]},503,owner);
  check('share creation fails closed when required audit fails',sql('SELECT count(*) FROM chat_shares'),shareCount);
  check('failed share writes never leave notification rows',sql('SELECT count(*) FROM notifications'),notificationCount);
  await expect('DELETE',`/chat/${chat}/shares/${departmentLink.id}`,undefined,503,owner);
  await expect('PUT',`/chat/${chat}/shares/${departmentLink.id}`,{department_ids:[otherDepartment]},503,owner);
  check('failed scope change preserves department grants',sql(`SELECT "departmentId" FROM chat_share_departments WHERE "shareId"=${q(departmentLink.id)}`),department);
  await expect('GET',path,undefined,200);
  sql('DROP TRIGGER share_fail_audit ON auth_audit_events; DROP FUNCTION share_fail_audit()');
  const failedFeed = await openNotificationFeed(outsider);
  const requiredAuditCount = sql('SELECT count(*) FROM auth_audit_events');
  sql(`CREATE FUNCTION share_fail_notification() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW."shareId" IS NOT NULL THEN RAISE EXCEPTION 'forced notification failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER share_fail_notification BEFORE INSERT ON notifications FOR EACH ROW EXECUTE FUNCTION share_fail_notification()`);
  await expect('POST',`/chat/${chat}/shares`,{access:'department',department_ids:[department]},503,owner);
  check('notification insertion failure rolls back the share and its audit',sql('SELECT count(*) FROM chat_shares'),shareCount);
  const preservedNotice = sql(`SELECT id FROM notifications WHERE "shareId"=${q(departmentLink.id)} AND "userId"=${q(viewerId)}`);
  await expect('PUT',`/chat/${chat}/shares/${departmentLink.id}`,{department_ids:[otherDepartment,department]},503,owner);
  check('failed notification update preserves existing inbox rows',sql(`SELECT id FROM notifications WHERE "shareId"=${q(departmentLink.id)} AND "userId"=${q(viewerId)}`),preservedNotice);
  check('failed notification writes also roll back required audit events',sql('SELECT count(*) FROM auth_audit_events'),requiredAuditCount);
  await new Promise(resolve=>setTimeout(resolve,100));
  check('uncommitted sharing notifications are never broadcast',failedFeed.events.length,0);
  await failedFeed.close();
  sql('DROP TRIGGER share_fail_notification ON notifications; DROP FUNCTION share_fail_notification()');
  const metadata = {files:[{id:foreign.id,name:'foreign.html',type:'text/html'}]};
  sql(`UPDATE messages SET metadata=${j(metadata)} WHERE id=${q(message)}`);
  await expect('POST',`/chat/${chat}/shares`,{},404,owner);
  sql(`UPDATE messages SET metadata=${j({files:[attachment,{...inline,id:file.id}]})} WHERE id=${q(message)}`);
  await expect('POST',`/chat/${chat}/shares`,{},400,owner);
  sql(`UPDATE messages SET metadata=${j({files:[attachment,inline]})} WHERE id=${q(message)}`);
  sql(`UPDATE messages SET "messageContent"=repeat('x',4194305) WHERE id=${q(message)}`);
  await expect('POST',`/chat/${chat}/shares`,{},400,owner);
  sql(`UPDATE messages SET "messageContent"='modified after sharing' WHERE id=${q(message)}`);

  const userRole = sql(`SELECT id FROM roles WHERE name='User' LIMIT 1`);
  sql(`DELETE FROM user_role_assignments WHERE "userId"=${q(ownerId)}`);
  await expect('POST',`/chat/${chat}/shares`,{},403,owner);
  await expect('DELETE',`/chat/${chat}/shares/${org.id}`,undefined,204,owner);
  await expect('DELETE',`/chat/${chat}/shares/${org.id}`,undefined,204,owner);
  await expect('GET',orgPath,undefined,404,viewer);
  sql(`INSERT INTO user_role_assignments (id,"userId","roleId","scopeDepartmentId","assignedBy","createdAt","updatedAt") VALUES (${q(randomUUID())},${q(ownerId)},${q(userRole)},NULL,${q(adminId)},now(),now())`);
  await expect('PUT',`/chat/${chat}/shares/${org.id}`,{access:'organization'},404,owner);
  sql(`UPDATE chat_shares SET "expiresAt"=now()-interval '1 second' WHERE id=${q(departmentLink.id)}`);
  await expect('GET',path,undefined,404);
  await expect('GET',`/me/shared-chats/${departmentLink.id}/link`,undefined,404,viewer);
  await expect('POST',`${path}/fork`,undefined,404,viewer);
  await expect('PUT',`/chat/${chat}/shares/${departmentLink.id}`,{clear_expiry:true},200,owner);
  sql(`UPDATE users SET status='deactivated' WHERE id=${q(ownerId)}`);
  await expect('GET',path,undefined,404);
  sql(`UPDATE users SET status='active' WHERE id=${q(ownerId)}; UPDATE users SET status='deactivated' WHERE id=${q(viewerId)}`);
  await expect('GET',path,undefined,401,viewer);
  await expect('GET',path,undefined,401,'');
  sql(`UPDATE users SET status='active' WHERE id=${q(viewerId)}`);

  await new Promise(resolve=>setTimeout(resolve,250));
  const audit = sql(`SELECT coalesce(string_agg(details::text,' '),'') FROM audit_logs WHERE action LIKE 'chat_share_%'`);
  assert(audit.includes('[REDACTED]'),'fork path must be audited with redacted token');
  for (const secret of [departmentToken,orgToken,'original question','modified after sharing','shared attachment','inline contents']) assert(!audit.includes(secret),'audit must not contain share secrets');
  const durable = sql(`SELECT string_agg(payload::text,' ') FROM auth_audit_events WHERE event LIKE 'chat.share_%'`);
  assert(!durable.includes(departmentToken));
  check('audit events do not expose share tokens or snapshot content',true);
  await expect('DELETE',`/chat/${chat}/shares/${departmentLink.id}`,undefined,404,viewer);
  await expect('DELETE',`/chat/${chat}/shares/${departmentLink.id}`,undefined,204,owner);
  check('revocation removes sharing notifications',sql(`SELECT count(*) FROM notifications WHERE "shareId"=${q(departmentLink.id)}`),'0');
  await expect('GET',`/me/shared-chats/${departmentLink.id}/link`,undefined,404,viewer);
  await expect('GET',`${path}/files/${file.id}`,undefined,404);
  await expect('POST',`${path}/fork`,undefined,404,viewer);
  check('copied files survive source revocation',(await req('GET',`/files/${forkFile}/download`,undefined,viewer)).status,200);
  const doomedDepartment = await createDepartment();
  const scoped = (await expect('POST',`/chat/${chat}/shares`,{access:'department',department_ids:[doomedDepartment]},201,owner)).json();
  const scopedPath = `/share/${tokenOf(scoped)}`;
  sql(`UPDATE users SET "departmentId"=${q(doomedDepartment)} WHERE id=${q(viewerId)}`);
  await expect('GET',scopedPath,undefined,200,viewer);
  sql(`DELETE FROM departments WHERE id=${q(doomedDepartment)}`);
  await expect('GET',scopedPath,undefined,404,viewer);
  await expect('POST',`${scopedPath}/fork`,undefined,404,viewer);
  check('department deletion never widens sharing',sql(`SELECT count(*) FROM chat_share_departments WHERE "shareId"=${q(scoped.id)}`),'0');
  const cascade = (await expect('POST',`/chat/${chat}/shares`,{access:'organization'},201,owner)).json();
  sql(`DELETE FROM conversations WHERE id=${q(chat)}`);
  await expect('GET',`/share/${tokenOf(cascade)}`,undefined,404);
  check('source deletion cascades shares',sql(`SELECT count(*) FROM chat_shares WHERE "conversationId"=${q(chat)}`),'0');
  check('source deletion also cascades sharing notifications',sql(`SELECT count(*) FROM notifications WHERE "shareId"=${q(cascade.id)}`),'0');
  const staleShare=(await expect('POST',`/chat/${fork}/shares`,{notify:false},201,viewer)).json();
  const stalePath=`/share/${tokenOf(staleShare)}`;
  await expect('DELETE',`/files/${forkFile}`,undefined,200,viewer);
  await expect('GET',`${stalePath}/files/${forkFile}`,undefined,404,viewer);
  const staleView=(await expect('GET',stalePath,undefined,200,viewer)).json();
  check('deleted attachments disappear without losing shared text',staleView.chat.messages.length>0 && staleView.chat.messages.every(m=>m.files.every(f=>f.id!==forkFile)&&m.artifacts.every(a=>a.file_id!==forkFile)));
  const withoutDeleted=(await expect('POST',`${stalePath}/fork`,undefined,201,owner)).json().conversation_id;
  check('continue omits deleted files and their artifacts',sql(`SELECT count(*) FROM artifacts WHERE "conversationId"=${q(withoutDeleted)}`),'0');
  await expect('PUT',`/chat/${fork}/shares/${staleShare.id}`,{refresh_snapshot:true,notify:false},200,viewer);
  const freshWithoutDeleted=(await expect('POST',`/chat/${fork}/shares`,{notify:false},201,viewer)).json();
  check('create and refresh both omit deleted attachments',(await expect('GET',`/share/${tokenOf(freshWithoutDeleted)}`,undefined,200,viewer)).json().chat.messages.every(m=>m.files.every(f=>f.id!==forkFile)));
  const liveFile=sql(`SELECT f->>'id' FROM messages m CROSS JOIN LATERAL jsonb_array_elements(m.metadata->'files') f WHERE m."conversationId"=${q(fork)} AND f->>'id'<>${q(forkFile)} LIMIT 1`);
  assert.match(liveFile,/^[0-9a-f-]{36}$/);
  const livePath=sql(`SELECT "localPath" FROM files WHERE id=${q(liveFile)}`);
  const fifo=`${livePath}.test-fifo`;
  const secondLink=(await expect('POST',`/chat/${withoutDeleted}/shares`,{notify:false},201,owner)).json();
  const secondFile=sql(`SELECT f->>'id' FROM messages m CROSS JOIN LATERAL jsonb_array_elements(m.metadata->'files') f WHERE m."conversationId"=${q(withoutDeleted)} LIMIT 1`);
  const secondPath=sql(`SELECT "localPath" FROM files WHERE id=${q(secondFile)}`);
  const secondFifo=`${secondPath}.test-fifo`;
  execFileSync('mkfifo',[fifo]);
  execFileSync('mkfifo',[secondFifo]);
  let writer, secondWriter;
  try {
    sql(`UPDATE files SET "localPath"=${q(fifo)} WHERE id=${q(liveFile)}`);
    sql(`UPDATE files SET "localPath"=${q(secondFifo)} WHERE id=${q(secondFile)}`);
    const forkBefore=dbCounts(), diskBefore=await diskFiles();
    const pendingFork=req('POST',`/share/${tokenOf(freshWithoutDeleted)}/fork`,undefined,owner);
    for(let index=0;index<400;index++) {
      try {writer=await open(fifo,constants.O_WRONLY|constants.O_NONBLOCK);break;}
      catch(error) {if(error.code!=='ENXIO')throw error;await new Promise(resolve=>setTimeout(resolve,10));}
    }
    assert(writer,'fork never reached the controlled slow file');
    const secondFork=req('POST',`/share/${tokenOf(secondLink)}/fork`,undefined,viewer);
    for(let index=0;index<400;index++) {
      try {secondWriter=await open(secondFifo,constants.O_WRONLY|constants.O_NONBLOCK);break;}
      catch(error) {if(error.code!=='ENXIO')throw error;await new Promise(resolve=>setTimeout(resolve,10));}
    }
    assert(secondWriter,'second fork never reached its controlled slow file');
    const busyCopy=await expect('POST',`/share/${tokenOf(freshWithoutDeleted)}/fork`,undefined,429,owner);
    check('copy saturation is explicitly retryable',[busyCopy.json().error,Number(busyCopy.headers.get('retry-after'))>0],['share_fork_busy',true]);
    await expect('POST','/share/invalid-link/fork',undefined,404,owner);
    await expect('GET','/',undefined,200);
    check('slow fork file read holds no share row lock',sql(`SELECT count(*) FROM pg_locks l JOIN pg_class c ON c.oid=l.relation WHERE c.relname='chat_shares' AND l.mode='RowShareLock'`),'0');
    await expect('DELETE',`/chat/${fork}/shares/${freshWithoutDeleted.id}`,undefined,204,viewer);
    await expect('DELETE',`/chat/${withoutDeleted}/shares/${secondLink.id}`,undefined,204,owner);
    await writer.writeFile('inline contents');await writer.close();writer=undefined;
    await secondWriter.writeFile('inline contents');await secondWriter.close();secondWriter=undefined;
    const denied=await pendingFork;
    check('revocation during staging is rechecked before commit',denied.status,404);
    check('second revoked copy releases its slot too',(await secondFork).status,404);
    check('failed final authorization cleans staged bytes and writes no rows',[dbCounts(),await diskFiles()],[forkBefore,diskBefore]);
  } finally {
    await writer?.close();
    await secondWriter?.close();
    sql(`UPDATE files SET "localPath"=${q(livePath)} WHERE id=${q(liveFile)}`);
    sql(`UPDATE files SET "localPath"=${q(secondPath)} WHERE id=${q(secondFile)}`);
    await unlink(fifo);
    await unlink(secondFifo);
  }
  await expect('POST',`${stalePath}/fork`,undefined,201,owner);
  check('copy slots recover immediately after failed copies',true);
  sql(`INSERT INTO users SELECT (jsonb_populate_record(NULL::users,to_jsonb(u)||jsonb_build_object('id',gen_random_uuid(),'email',gen_random_uuid()::text||'@share-fanout.invalid','googleId',NULL,'azureId',NULL,'identities',NULL))).* FROM users u CROSS JOIN generate_series(1,505) WHERE u.id=${q(viewerId)}`);
  const fanoutRole=(await expect('POST',`/admin/users/${viewerId}/roles`,{role_id:superRole},201,admin)).json();
  const largeShare = (await expect('POST',`/chat/${fork}/shares`,{access:'organization',notify:true},201,viewer)).json();
  await expect('DELETE',`/admin/users/${viewerId}/roles/${fanoutRole.id}`,undefined,204,admin);
  check('organization fan-out covers multiple recipient batches',sql(`SELECT count(*) FROM notifications WHERE "shareId"=${q(largeShare.id)}`),sql(`SELECT count(*) FROM users WHERE status='active' AND id<>${q(viewerId)}`));
  assert(Number(sql(`SELECT count(*) FROM notifications WHERE "shareId"=${q(largeShare.id)}`))>500);
  const budgetNotes = sql(`SELECT count(*) FROM notifications WHERE kind LIKE 'budget.%'`);
  execFileSync(migrator,['down','-n','1'],{env:process.env,stdio:['ignore','pipe','pipe']});
  check('down migration removes share notices but preserves budget notifications',sql(`SELECT count(*) FROM notifications WHERE kind LIKE 'budget.%'`),budgetNotes);
  check('down migration leaves no orphaned sharing notifications',sql(`SELECT count(*) FROM notifications WHERE kind='chat.shared'`),'0');
  execFileSync(migrator,['up'],{env:process.env,stdio:['ignore','pipe','pipe']});
  await expect('GET','/',undefined,200);
  await expect('GET',`/chat/${fork}`,undefined,200,viewer);
  for(let index=0;index<140;index++) await expect('GET','/share/not-a-valid-token',undefined,401,'');
  check('anonymous requests never consume signed-in quotas',(await req('GET','/share/not-a-valid-token',undefined,owner)).status,404);
  let limited;
  for (let index=0;index<130;index++) {
    const response = await req('GET','/share/not-a-valid-token');
    if (response.status===429) {limited=response;break;}
    assert.equal(response.status,404);
  }
  check('share endpoints enforce a retryable rate limit',limited?.status,429);
  check('rate limit includes privacy and retry headers',Number(limited.headers.get('retry-after'))>0 && limited.headers.get('cache-control')==='private, no-store');
  await expect('GET',`/share/not-a-valid-token/files/${randomUUID()}`,undefined,404,viewer);
  await expect('GET','/share/not-a-valid-token',undefined,404,owner);
  const revokeAfterLimit=(await expect('POST',`/chat/${fork}/shares`,{notify:false},201,viewer)).json();
  let changeLimited;
  for(let index=0;index<65;index++) {
    const response=await req('PUT',`/chat/${fork}/shares/${randomUUID()}`,{},viewer);
    if(response.status===429){changeLimited=response;break;}
    assert.equal(response.status,404);
  }
  check('link changes have their own authenticated rate limit',changeLimited?.status,429);
  await expect('DELETE',`/chat/${fork}/shares/${revokeAfterLimit.id}`,undefined,204,viewer);
  check('exhausted changes do not prevent link revocation',true);
  console.log(`Share Chat HTTP checks: ${checks} passed`);
} finally {
  for (const feed of notificationFeeds) await feed.close();
  await new Promise(resolve=>fixture.close(resolve));
}
