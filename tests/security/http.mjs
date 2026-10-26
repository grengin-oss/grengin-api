import assert from 'node:assert/strict';
import { createHmac, randomUUID } from 'node:crypto';
import { execFile, execFileSync } from 'node:child_process';
import { promisify } from 'node:util';
import { readFile } from 'node:fs/promises';
import { createServer } from 'node:http';

const db = new URL(process.env.DATABASE_URL);
assert.equal(db.pathname, '/grengin_security_test', 'refusing a non-test database');
assert.equal(db.hostname, '127.0.0.1');
const base = process.env.GRENGIN_SECURITY_TEST_URL;
assert.equal(new URL(base).hostname, '127.0.0.1');
const sql = query => execFileSync('psql', ['-X', '-A', '-t', '-v', 'ON_ERROR_STOP=1', process.env.DATABASE_URL, '-c', query], {encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe']}).trim();
const quote = value => `'${String(value).replaceAll("'", "''")}'`;
const ownerId = sql(`SELECT a."userId" FROM user_role_assignments a JOIN roles r ON r.id=a."roleId" JOIN users u ON u.id=a."userId" WHERE r.name='Super Admin' AND a."scopeDepartmentId" IS NULL AND u.status='active' LIMIT 1`);
assert.match(ownerId, /^[0-9a-f-]{36}$/);
function token(userId, use = 'access') {
  const claims = {sub: 'security-test@example.invalid', user_id: userId, name: 'Security test', exp: Math.floor(Date.now()/1000)+1800, token_use: use};
  if (use === 'refresh') claims.refresh = true;
  const body = [Buffer.from(JSON.stringify({alg: 'HS256', typ: 'JWT'})).toString('base64url'), Buffer.from(JSON.stringify(claims)).toString('base64url')].join('.');
  return `${body}.${createHmac('sha256', process.env.JWT_SECRET).update(body).digest('base64url')}`;
}
const owner = token(ownerId);
let passed = 0;
function check(name, actual, expected) { assert.deepEqual(actual, expected, name); console.log(`PASS ${++passed}: ${name}`); }
async function request(method, path, body, bearer = owner) {
  const response = await fetch(`${base}${path}`, {method, headers: {'content-type': 'application/json', ...(bearer ? {authorization: `Bearer ${bearer}`} : {})}, body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(45000)});
  const text = await response.text();
  return {status: response.status, text, json: () => JSON.parse(text)};
}
async function expect(method, path, body, status, bearer = owner) {
  const response = await request(method, path, body, bearer);
  assert.equal(response.status, status, `${method} ${path}: expected ${status}, got ${response.status}: ${response.text.slice(0,300)}`);
  return response;
}
const calls = [];
const fixture = createServer(async (req, res) => {
  let raw = ''; for await (const chunk of req) raw += chunk;
  calls.push(req.url);
  if (req.url.includes('chat/completions')) {
    res.writeHead(200, {'content-type':'text/event-stream'});
    for (const event of [
      {id:'fixture', choices:[{delta:{content:'security fixture'}}]},
      {choices:[{delta:{}, finish_reason:'stop'}], usage:{prompt_tokens:10, completion_tokens:2, total_tokens:12}},
    ]) res.write(`data: ${JSON.stringify(event)}\n\n`);
    res.end('data: [DONE]\n\n');
  } else if (req.url.includes('images/')) {
    res.writeHead(200, {'content-type':'application/json'});
    res.end(JSON.stringify({data:[{b64_json:'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg=='}], usage:{input_tokens:2,output_tokens:1}}));
  } else {
    res.writeHead(404, {'content-type':'application/json'}); res.end('{}');
  }
});
await new Promise(resolve => fixture.listen(0, '127.0.0.1', resolve));
try {
  const manifest = JSON.parse(await readFile('llm-plugin/examples/openai-compatible.provider.json', 'utf8'));
  manifest.id = 'security-fixture'; manifest.name = 'Security fixture';
  manifest.capabilities.modelListing = true; delete manifest.operations.listModels;
  manifest.models = [
    {id:'security-chat', name:'Security chat', modelType:'text_generator', capabilities:{chat:manifest.capabilities.chat}, metadata:{inputTokenRate:1, outputTokenRate:2}},
    {id:'security-unpriced', name:'Unpriced', modelType:'text_generator', capabilities:{chat:manifest.capabilities.chat}},
    {id:'security-free', name:'Free', modelType:'text_generator', capabilities:{chat:manifest.capabilities.chat}, metadata:{inputTokenRate:0, outputTokenRate:0}},
    {id:'security-expensive', name:'Expensive fixture', modelType:'text_generator', capabilities:{chat:manifest.capabilities.chat}, metadata:{inputTokenRate:1000000, outputTokenRate:2000000}},
    {id:'security-image', name:'Image', modelType:'image_generator', capabilities:{imageGeneration:true}, metadata:{inputTokenRate:1, outputTokenRate:2, imageInputTokenRate:3, imageOutputTokenRate:4}},
  ];
  const pluginConfig = {manifest, configuration:{}, baseUrlOverride:`http://127.0.0.1:${fixture.address().port}/`, allowInsecureHttp:true, allowPrivateNetwork:true};
  const validation = (await expect('POST','/admin/ai-engines/plugin-validate',{plugin_config:pluginConfig},200)).json();
  assert.equal(validation.valid,true,`fixture plugin: ${validation.error}`);
  await expect('POST', '/admin/ai-engines', {is_enabled:true, api_key:'local-fixture-only', whitelisted_models:manifest.models.map(m=>m.id), default_model:'security-chat', plugin_config:pluginConfig}, 201);
  await expect('GET', '/models', undefined, 401, token(ownerId,'refresh'));
  await expect('GET', '/mcp/tools', undefined, 401, '');
  check('refresh tokens and anonymous MCP listing denied', true, true);

  const newUser = async () => {
    const email = `security-${randomUUID()}@example.invalid`;
    await expect('POST', '/admin/users', {name:'Security test', email}, 201);
    const id = sql(`SELECT id FROM users WHERE email=${quote(email)}`);
    await expect('PATCH', `/admin/users/${id}/status`, {status:'active'}, 200);
    return id;
  };
  const otherId = await newUser(); const other = token(otherId);
  const uploaded = async (name, bearer=owner) => (await expect('POST', '/files', {attachment:{file:Buffer.from('<html>same bytes</html>').toString('base64'),name,type:'text/html'}, description:name}, 200, bearer)).json();
  const file = await uploaded('alpha.html'); const duplicate = await uploaded('renamed.html');
  check('dedup preserves independent file identities and metadata', [file.id !== duplicate.id, duplicate.name], [true,'renamed.html']);
  const download = await fetch(`${base}/files/${file.id}`, {headers:{authorization:`Bearer ${owner}`}});
  // Downloads use their dedicated /download path, not the metadata endpoint.
  const bytesResponse = await fetch(`${base}/files/${file.id}/download`, {headers:{authorization:`Bearer ${owner}`}});
  check('HTML is attachment-only with nosniff', [bytesResponse.status, bytesResponse.headers.get('x-content-type-options'), bytesResponse.headers.get('content-disposition')?.startsWith('attachment')], [200,'nosniff',true]);
  await download.body?.cancel(); await bytesResponse.body?.cancel();
  const foreign = await uploaded('foreign.html', other);
  const payload = files => ({provider:manifest.id, model_name:'security-chat', messages:[{role:'user',content:'test',files}]});
  const reference = id => ({id,name:'file.html',type:'text/html'});
  const counts = () => sql('SELECT (SELECT count(*) FROM conversations)::text || \':\' || (SELECT count(*) FROM messages)::text');
  for (const id of [foreign.id, randomUUID()]) {
    const before = counts(); const previousCalls = calls.length;
    await expect('POST','/chat/stream',payload([reference(id)]),404);
    check('invalid attachment rejected before history or provider writes', [counts(),calls.length], [before,previousCalls]);
  }
  const multiple = payload([]); multiple.messages.push({role:'user',content:'invalid second message',files:[reference(foreign.id)]});
  const multipleBefore = counts();
  await expect('POST','/chat/stream',multiple,404);
  check('a later invalid message cannot leave an earlier partial write',counts(),multipleBefore);
  const stream = await expect('POST','/chat/stream',payload([]),200);
  assert(!stream.text.includes('ai_error'), 'fixture chat must succeed');
  const events = [...stream.text.matchAll(/^data: (.+)$/gm)].flatMap(match => {try{return [JSON.parse(match[1])];}catch{return [];}});
  const chatId = events.find(e=>e.is_new===true)?.id;
  assert.match(chatId ?? '', /^[0-9a-f-]{36}$/);
  const before = counts();
  await expect('POST',`/chat/stream/${chatId}`,payload([reference(foreign.id)]),404);
  check('bad continuation leaves existing history unchanged', counts(), before);
  await expect('POST',`/chat/stream/${chatId}`,payload([]),404,other);
  check('another user cannot continue a private chat', counts(),before);
  const image = payload([{...reference(foreign.id),base64:'YQ=='}]); image.model_name='security-image';
  await expect('POST','/chat/stream',image,404);
  check('inline bytes do not bypass image ownership validation',counts(),before);
  sql(`CREATE FUNCTION security_image_save_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.role='assistant' AND NEW."modelName"='security-image' THEN RAISE EXCEPTION 'forced test image save failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER security_image_save_failure BEFORE INSERT ON messages FOR EACH ROW EXECUTE FUNCTION security_image_save_failure()`);
  const failedImage = payload([]); failedImage.model_name='security-image';
  const imageError = await expect('POST','/chat/stream',failedImage,200);
  check('image save failure emits one error and no success',[(imageError.text.match(/^event: "ai_error"$/gm)??[]).length,imageError.text.includes('event: "image_generated"')],[1,false]);
  sql('DROP TRIGGER security_image_save_failure ON messages; DROP FUNCTION security_image_save_failure()');

  const artifact = randomUUID();
  const message = sql(`SELECT id FROM messages WHERE "conversationId"=${quote(chatId)} AND role='assistant' LIMIT 1`);
  sql(`INSERT INTO artifacts (id,"fileId","messageId","conversationId",title,"contentType","createdAt","updatedAt") VALUES (${quote(artifact)},${quote(file.id)},${quote(message)},${quote(chatId)},'Shared artifact','text/html',now(),now())`);
  const project = (await expect('POST','/projects',{name:'Security shared artifact'},201)).json();
  await expect('POST',`/projects/${project.id}/sources`,{file_id:file.id,file_name:file.name,file_type:'text/html',file_size:25,origin:'uploaded'},201);
  sql(`CREATE FUNCTION security_delete_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'forced test delete failure'; END $$; CREATE TRIGGER security_delete_failure BEFORE DELETE ON artifacts FOR EACH ROW EXECUTE FUNCTION security_delete_failure()`);
  await expect('DELETE',`/artifacts/${artifact}`,undefined,503);
  check('failed artifact delete never unlinks shared bytes',(await request('GET',`/files/${file.id}/download`)).status,200);
  sql('DROP TRIGGER security_delete_failure ON artifacts; DROP FUNCTION security_delete_failure()');
  await expect('DELETE',`/artifacts/${artifact}`,undefined,204);
  check('artifact delete retains shared project file',[(await request('GET',`/files/${file.id}/download`)).status,sql(`SELECT count(*) FROM project_sources WHERE "fileId"=${quote(file.id)}`)],[200,'1']);

  const treeA = (await expect('POST','/admin/departments',{name:`Tree A ${randomUUID()}`,description:'test'},201)).json();
  const treeB = (await expect('POST','/admin/departments',{name:`Tree B ${randomUUID()}`,description:'test'},201)).json();
  const pathA = sql(`SELECT path::text FROM departments WHERE id=${quote(treeA.id)}`);
  const pathB = sql(`SELECT path::text FROM departments WHERE id=${quote(treeB.id)}`);
  // Model an already-running move holding the same transaction lock as the API.
  const move = promisify(execFile)('psql',['-X','-q','-v','ON_ERROR_STOP=1',process.env.DATABASE_URL,'-c',`BEGIN; SELECT pg_advisory_xact_lock(1769084371,1); UPDATE departments SET "parentId"=${quote(treeB.id)},depth=1,path=${quote(`${pathB}.${pathA}`)}::ltree WHERE id=${quote(treeA.id)}; SELECT pg_sleep(1); COMMIT;`]);
  for (let i=0;i<100;i++) {
    if (sql("SELECT count(*) FROM pg_locks WHERE locktype='advisory' AND classid=1769084371 AND objid=1 AND granted") !== '0') break;
    await new Promise(resolve=>setTimeout(resolve,10));
  }
  const newChild = (await expect('POST','/admin/departments',{name:`Concurrent child ${randomUUID()}`,description:'test',parent_id:treeA.id},201)).json();
  await move;
  check('create queued behind a move uses the committed parent path',sql(`SELECT path::text FROM departments WHERE id=${quote(newChild.id)}`).startsWith(`${pathB}.${pathA}.`),true);

  const department = (await expect('POST','/admin/departments',{name:`Security ${randomUUID()}`,description:'test'},201)).json();
  const child = (await expect('POST','/admin/departments',{name:`Child ${randomUUID()}`,description:'test',parent_id:department.id},201)).json();
  const deptRole = sql(`SELECT id FROM roles WHERE name='Department Admin' LIMIT 1`);
  await expect('POST',`/admin/users/${otherId}/roles`,{role_id:deptRole,scope_department_id:department.id},201);
  await expect('PUT',`/admin/users/${otherId}`,{department_id:department.id},200);
  await expect('PUT',`/admin/users/${otherId}`,{unassign_department:true},403,other);
  check('scoped admin cannot self-unassign',sql(`SELECT "departmentId" FROM users WHERE id=${quote(otherId)}`),department.id);
  await expect('DELETE',`/admin/departments/${department.id}/members`,[otherId],403,other);
  check('scoped admin cannot unassign via member removal',sql(`SELECT "departmentId" FROM users WHERE id=${quote(otherId)}`),department.id);
  sql(`UPDATE departments SET "budgetAllocated"=100,"actionOnExceed"='block' WHERE id=${quote(department.id)}; UPDATE users SET "departmentId"=${quote(child.id)} WHERE id=${quote(otherId)}`);
  const unpriced = payload([]); unpriced.model_name='security-unpriced';
  await expect('POST','/chat/stream',unpriced,400,other);
  check('unfunded child inherits pricing requirements',true,true);
  const free = payload([]); free.model_name='security-free';
  const freeResponse = await expect('POST','/chat/stream',free,200,other);
  check('explicit free models remain usable with an active budget',freeResponse.text.includes('ai_error'),false);
  sql(`UPDATE messages m SET cost=60 FROM conversations c WHERE m."conversationId"=c.id AND c."userId"=${quote(otherId)} AND m.role='assistant'`);
  await expect('PUT',`/admin/departments/${child.id}`,{budget_allocated:80},200);
  check('funding a child does not double-count its historical spend',sql(`SELECT "budgetAllocated"::int FROM departments WHERE id=${quote(child.id)}`),'80');
  sql(`UPDATE departments SET "budgetAllocated"=0 WHERE id=${quote(child.id)}; UPDATE messages m SET cost=0 FROM conversations c WHERE m."conversationId"=c.id AND c."userId"=${quote(otherId)}`);
  const alertRoot = (await expect('POST','/admin/departments',{name:`Alert root ${randomUUID()}`,description:'test'},201)).json();
  const alertChild = (await expect('POST','/admin/departments',{name:`Alert child ${randomUUID()}`,description:'test',parent_id:alertRoot.id},201)).json();
  sql(`UPDATE departments SET "budgetAllocated"=15 WHERE id=${quote(alertRoot.id)}; UPDATE users SET "departmentId"=${quote(alertChild.id)} WHERE id=${quote(otherId)}`);
  const expensive = payload([]); expensive.model_name='security-expensive';
  const billed = await expect('POST','/chat/stream',expensive,200,other);
  assert(!billed.text.includes('ai_error'));
  check('inherited spending alerts the funded ancestor',Number(sql(`SELECT count(*) FROM notifications WHERE "departmentId"=${quote(alertRoot.id)} AND kind='budget.low'`))>0,true);
  sql(`UPDATE users SET "departmentId"=${quote(child.id)} WHERE id=${quote(otherId)}`);
  sql(`UPDATE users SET "departmentId"=${quote(department.id)} WHERE id=${quote(ownerId)}; UPDATE messages SET cost=100 WHERE "conversationId"=${quote(chatId)} AND role='assistant'`);
  await expect('POST','/chat/stream',payload([]),403,other);
  check('exhausted parent cannot be escaped through a zero-budget child',true,true);

  const audited = (await expect('POST','/admin/departments',{name:`Audited ${randomUUID()}`,description:'test',admin_ids:[otherId]},201)).json();
  sql(`CREATE FUNCTION security_audit_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'forced test audit failure'; END $$; CREATE TRIGGER security_audit_failure BEFORE INSERT ON auth_audit_events FOR EACH ROW EXECUTE FUNCTION security_audit_failure()`);
  await expect('DELETE',`/admin/departments/${audited.id}`,undefined,503);
  check('department deletion rolls back if grant audit fails',sql(`SELECT count(*) FROM user_role_assignments WHERE "scopeDepartmentId"=${quote(audited.id)}`),'1');
  sql('DROP TRIGGER security_audit_failure ON auth_audit_events; DROP FUNCTION security_audit_failure()');
  await expect('DELETE',`/admin/departments/${audited.id}`,undefined,204);
  check('successful department deletion durably audits grant revocation',sql(`SELECT count(*) FROM auth_audit_events WHERE event='auth.role_unassigned' AND payload->>'scope_department_id'=${quote(audited.id)}`),'1');
  await expect('PATCH',`/admin/users/${otherId}/status`,{status:'deactivated'},200);
  await expect('GET','/models',undefined,401,other);
  check('deactivation cuts off an already-issued token',true,true);
  console.log(`HTTP regressions: ${passed} passed`);
} finally { await new Promise(resolve=>fixture.close(resolve)); }
