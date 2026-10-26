import assert from 'node:assert/strict';
import {createHmac,randomUUID} from 'node:crypto';
import {execFile,execFileSync} from 'node:child_process';
import {readFile} from 'node:fs/promises';
import {promisify} from 'node:util';
const db=new URL(process.env.DATABASE_URL);
assert.equal(db.hostname,'127.0.0.1');assert.equal(db.pathname,'/grengin_security_test');
const base=process.env.GRENGIN_SECURITY_TEST_URL;assert.equal(new URL(base).hostname,'127.0.0.1');
const sql=query=>execFileSync('psql',['-X','-A','-t','-v','ON_ERROR_STOP=1',process.env.DATABASE_URL,'-c',query],{encoding:'utf8',stdio:['ignore','pipe','pipe']}).trim();
const q=value=>`'${String(value).replaceAll("'","''")}'`;
const admin=sql(`SELECT a."userId" FROM user_role_assignments a JOIN roles r ON r.id=a."roleId" JOIN users u ON u.id=a."userId" WHERE r.name='Super Admin' AND a."scopeDepartmentId" IS NULL AND u.status='active' LIMIT 1`);
const content=[Buffer.from(JSON.stringify({alg:'HS256',typ:'JWT'})).toString('base64url'),Buffer.from(JSON.stringify({user_id:admin,sub:'engine-test@example.invalid',name:'Engine test',token_use:'access',exp:Math.floor(Date.now()/1000)+1800})).toString('base64url')].join('.');
const token=`${content}.${createHmac('sha256',process.env.JWT_SECRET).update(content).digest('base64url')}`;
let checks=0;
function check(name,actual,expected=true){assert.deepEqual(actual,expected,name);console.log(`PASS engines ${++checks}: ${name}`);}
async function expect(method,path,body,status,bearer=token){
  const response=await fetch(`${base}${path}`,{method,headers:{'content-type':'application/json',...(bearer?{authorization:`Bearer ${bearer}`}:{})},body:body===undefined?undefined:JSON.stringify(body),signal:AbortSignal.timeout(45000)});
  const text=await response.text();assert.equal(response.status,status,`${method} ${path}: ${response.status}: ${text.slice(0,200)}`);checks++;
  return text ? JSON.parse(text) : undefined;
}
check('startup does not install the discovery catalog',sql('SELECT count(*) FROM ai_engines'),'0');
check('admin listing does not install engines',await expect('GET','/admin/ai-engines',undefined,200),[]);
const manifest=JSON.parse(await readFile('llm-plugin/examples/openai-compatible.provider.json','utf8'));
manifest.id=`install-${randomUUID()}`;manifest.name='Installation fixture';
manifest.capabilities.modelListing=true;delete manifest.operations.listModels;
manifest.models=[
  ['older','text_generator','2025-01-01'],['newest','text_generator','2026-10-01'],
  ['next','text_generator','2026-09-01'],['image','image_generator','2026-08-01'],
  ['embed','text_embedder','2026-10-03'],['retired','text_generator','2026-10-04'],
].map(([id,modelType,launchDate])=>({id,name:id,modelType,
  capabilities:modelType==='text_generator'?{chat:manifest.capabilities.chat}:modelType==='text_embedder'?{embeddings:true}:{imageGeneration:true},
  metadata:{launchDate,inputTokenRate:0,outputTokenRate:0,...(id==='retired'?{deprecated:true}:{})}}));
const config=()=>({manifest:structuredClone(manifest),configuration:{}});
await expect('POST','/admin/ai-engines',{plugin_config:config()},401,null);
await expect('POST','/admin/ai-engines',{engine_key:''},400);
check('rejected requests do not install engines',sql('SELECT count(*) FROM ai_engines'),'0');
await expect('POST','/admin/ai-engines',{plugin_config:config(),api_key:'   '},400);
const created=await expect('POST','/admin/ai-engines',{plugin_config:config(),api_key:'fixture-key'},201);
check('key installation auto-enables with only recent generators',[created.is_enabled,created.whitelisted_models],[true,['newest','next','image']]);
check('API key remains encrypted',sql(`SELECT "apiKey"<>'fixture-key' FROM ai_engines WHERE "engineKey"=${q(manifest.id)}`),'t');
const marker=`engine-disable-${randomUUID()}`;
const disabling=promisify(execFile)('psql',['-X','-A','-t','-v','ON_ERROR_STOP=1',process.env.DATABASE_URL,'-c',`BEGIN; SELECT id FROM ai_engines WHERE "engineKey"=${q(manifest.id)} FOR UPDATE; SELECT pg_sleep(1.5) /* ${marker} */; UPDATE ai_engines SET "isEnabled"=false WHERE "engineKey"=${q(manifest.id)}; COMMIT;`],{encoding:'utf8'});
let locked=false;
for(let attempt=0;attempt<100;attempt++){
  if(sql(`SELECT count(*) FROM pg_stat_activity WHERE wait_event='PgSleep' AND query LIKE ${q(`%${marker}%`)}`)==='1'){locked=true;break;}
  await new Promise(resolve=>setTimeout(resolve,20));
}
assert.equal(locked,true,'disable transaction must hold the row before key rotation reads it');
const racedRotation=await expect('PUT',`/admin/ai-engines/${manifest.id}`,{api_key:'racing-key'},200);
await disabling;
check('key rotation cannot undo a concurrent disable',[racedRotation.is_enabled,sql(`SELECT "isEnabled" FROM ai_engines WHERE "engineKey"=${q(manifest.id)}`)],[false,'f']);
await expect('PUT',`/admin/ai-engines/${manifest.id}`,{whitelisted_models:['older'],default_model:'older',is_enabled:false},200);
const rotated=await expect('PUT',`/admin/ai-engines/${manifest.id}`,{api_key:'rotated-fixture-key'},200);
check('rotation preserves configured models and disabled state',[rotated.is_enabled,rotated.whitelisted_models,rotated.default_model],[false,['older'],'older']);
await expect('PUT',`/admin/ai-engines/${manifest.id}`,{api_key:'   '},400);
const empty=config();empty.manifest.id=`empty-${randomUUID()}`;
const explicit=await expect('POST','/admin/ai-engines',{plugin_config:empty,api_key:'fixture-key',is_enabled:false,whitelisted_models:[]},201);
check('explicit empty whitelist and disabled state win',[explicit.is_enabled,explicit.whitelisted_models],[false,[]]);
const controlled=config();controlled.manifest.id=`controlled-${randomUUID()}`;
await expect('POST','/admin/ai-engines',{plugin_config:controlled},201);
const controlledKey=await expect('PUT',`/admin/ai-engines/${controlled.manifest.id}`,{api_key:'fixture-key',is_enabled:false,whitelisted_models:['older'],default_model:'older'},200);
check('explicit first-key settings override activation defaults',[controlledKey.is_enabled,controlledKey.whitelisted_models,controlledKey.default_model],[false,['older'],'older']);
const deny=config();deny.manifest.id=`deny-${randomUUID()}`;
await expect('POST','/admin/ai-engines',{plugin_config:deny,whitelisted_models:[]},201);
const deniedKey=await expect('PUT',`/admin/ai-engines/${deny.manifest.id}`,{api_key:'fixture-key'},200);
check('first key cannot broaden an explicit deny-all policy',deniedKey.whitelisted_models,[]);
const limited=config();limited.manifest.id=`limited-${randomUUID()}`;
await expect('POST','/admin/ai-engines',{plugin_config:limited,whitelisted_models:['older'],default_model:'older'},201);
const limitedKey=await expect('PUT',`/admin/ai-engines/${limited.manifest.id}`,{api_key:'fixture-key'},200);
check('first key preserves a previously chosen small allowlist',[limitedKey.whitelisted_models,limitedKey.default_model],[['older'],'older']);
const subset=config();subset.manifest.id=`subset-${randomUUID()}`;
await expect('POST','/admin/ai-engines',{plugin_config:subset,whitelisted_models:['older','retired','embed','image']},201);
const subsetKey=await expect('PUT',`/admin/ai-engines/${subset.manifest.id}`,{api_key:'fixture-key'},200);
check('legacy narrowing never grants a previously denied model',subsetKey.whitelisted_models,['older','image']);
const legacy=config();legacy.manifest.id=`legacy-${randomUUID()}`;
const placeholder=await expect('POST','/admin/ai-engines',{plugin_config:legacy,whitelisted_models:manifest.models.map(model=>model.id)},201);
check('keyless engine stays disabled',placeholder.is_enabled,false);
const racing=config();racing.manifest.id=`race-${randomUUID()}`;
const race=await Promise.all(Array.from({length:3},()=>fetch(`${base}/admin/ai-engines`,{method:'POST',headers:{'content-type':'application/json',authorization:`Bearer ${token}`},body:JSON.stringify({plugin_config:racing})})));
check('concurrent installation creates exactly one row',race.map(response=>response.status).sort(),[201,409,409]);
for(const response of race) await response.body?.cancel();
check('concurrent installation leaves no duplicate engines',sql(`SELECT count(*) FROM ai_engines WHERE "engineKey"=${q(racing.manifest.id)}`),'1');
const activated=await expect('PUT',`/admin/ai-engines/${legacy.manifest.id}`,{api_key:'fixture-key'},200);
check('first key replaces legacy all-model defaults',[activated.is_enabled,activated.whitelisted_models],[true,['newest','next','image']]);
const catalog=await expect('POST','/admin/ai-engines',{engine_key:'groq'},201);
check('catalog installation stores policy but not the manifest',[catalog.engine_key,catalog.plugin_config??null,catalog.whitelisted_models.length<=3],['groq',null,true]);
await expect('POST','/admin/ai-engines',{engine_key:'groq'},409);
await expect('POST','/admin/ai-engines',{engine_key:'groq',plugin_config:config()},400);
await expect('DELETE','/admin/ai-engines/groq',undefined,204);
await expect('GET','/admin/ai-engines/groq',undefined,404);
console.log(`AI engine HTTP checks: ${checks} passed`);
