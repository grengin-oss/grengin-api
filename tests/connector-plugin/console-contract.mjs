import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';

export async function runConsoleContract({call,check,sql,quote,server,request,admin,member}) {
  const base=`/admin/mcp-servers/${server.id}`;
  const missing=`/admin/mcp-servers/${randomUUID()}`;
  for (const suffix of ['/overview','/people','/executions']) {
    await call('GET',base+suffix,undefined,401,null);
    await call('GET',base+suffix,undefined,403,member);
    await call('GET',missing+suffix,undefined,403,member);
    await call('GET',missing+suffix,undefined,404);
  }
  check('every connector detail endpoint requires sign-in, org-wide permission and an existing server',true,true);
  const overview=(await call('GET',base+'/overview')).json();
  check('overview exposes safe native metadata, prompts and explicit unsupported features',[
    overview.integration.plugin_id,Array.isArray(overview.integration.suggested_prompts),
    overview.integration.per_user_credentials,overview.reminders_supported,overview.write_confirmation_supported,
  ],['fixture-connector',true,true,false,false]);

  // These are the existing frontend's server-policy URLs, not the bulk tool-policy endpoint.
  const access=(await call('GET',base+'/access')).json();
  check('existing frontend server-access URL is live',[access.server_id,access.default_access],[server.id,'all_users']);
  await call('GET',base+'/access',undefined,403,member);
  const rules=JSON.parse(sql(`SELECT coalesce(json_agg(id),'[]') FROM mcp_access_policies WHERE "serverId"=${quote(server.id)}`));
  await call('PUT',base+'/access/default',{default_access:'admin_only'},403,member);
  await call('PUT',base+'/access/default',{default_access:'admin_only'});
  check('Admins only setting is enforced in the ordinary-user picker',
    (await call('GET','/mcp-servers',undefined,200,member)).json().servers.some(row=>row.id===server.id),false);
  check('admins still see an admin-only connector',
    (await call('GET','/mcp-servers')).json().servers.some(row=>row.id===server.id),true);
  const grant=(await call('POST',base+'/access/rules',{access_type:'user',user_id:member,permission:'read_only'},201)).json();
  check('creating a server rule returns one rule, not the whole access list',[grant.access_type,grant.user_id,grant.permission],['user',member,'read_only']);
  assert(grant.id,'Created rule UUID missing');
  check('explicit server grants are reflected in the ordinary-user picker',
    (await call('GET','/mcp-servers',undefined,200,member)).json().servers.some(row=>row.id===server.id),true);
  await call('DELETE',base+`/access/rules/${grant.id}`,undefined,204);
  check('removing a server grant hides the connector again',
    (await call('GET','/mcp-servers',undefined,200,member)).json().servers.some(row=>row.id===server.id),false);
  await call('PUT',base+'/access/default',{default_access:'all_users'});
  check('changing default access does not replace server rules',
    JSON.parse(sql(`SELECT coalesce(json_agg(id),'[]') FROM mcp_access_policies WHERE "serverId"=${quote(server.id)}`)),rules);
  const toolId=sql(`SELECT id FROM mcp_tools WHERE "serverId"=${quote(server.id)}`);
  await call('PUT',base+'/access/default',{default_access:'admin_only'});
  await call('PUT',`/admin/mcp-tools/${toolId}/access`,{inherit_from_server:false,rules:[{access_type:'user',user_id:member,permission:'read_only'}]});
  const permittedServer=(await call('GET','/mcp-servers',undefined,200,member)).json().servers.find(row=>row.id===server.id);
  check('explicit per-tool grants preserve a usable connector despite its default policy',permittedServer?.tools.map(tool=>tool.name),['plugin_echo']);
  check('picker and tool listing agree for an explicit per-tool grant',
    (await call('GET',`/mcp/tools?server_id=${server.id}`,undefined,200,member)).json().tools.map(tool=>tool.name),['plugin_echo']);
  await call('PUT',base+'/access/default',{default_access:'all_users'});
  await call('PUT',`/admin/mcp-tools/${toolId}/access`,{inherit_from_server:false,rules:[{access_type:'user',user_id:member,permission:'denied'}]});
  check('ordinary-user catalog does not expose tools denied by per-tool policies',
    (await call('GET','/mcp-servers',undefined,200,member)).json().servers.find(row=>row.id===server.id).tools,[]);
  await call('PUT',`/admin/mcp-tools/${toolId}/access`,{inherit_from_server:true,rules:[]});

  const other=(await call('POST','/admin/mcp-servers',{...request,name:'Console isolation fixture'},201)).json();
  const ids=Array.from({length:6},(_,i)=>`00000000-0000-4000-8000-${String(i+1).padStart(12,'0')}`);
  try {
    check('an installed server without executions returns an empty page',
      (await call('GET',`/admin/mcp-servers/${other.id}/executions`)).json(),{executions:[],total:0,limit:50,offset:0});
    const otherTool=sql(`SELECT id FROM mcp_tools WHERE "serverId"=${quote(other.id)}`);
    await call('PUT',base+`/tools/${otherTool}`,{enabled:false},404);
    check('tool toggle refuses a valid tool belonging to another server',sql(`SELECT enabled FROM mcp_tools WHERE id=${quote(otherTool)}`),'t');
    for (let i=0;i<6;i++) {
      const actor=i%2 ? member : admin;
      sql(`INSERT INTO mcp_executions(id,"serverId","serverName","toolName","userId","userEmail","isError","durationMs","executedAt","createdAt")
        VALUES (${quote(ids[i])},${quote(i===5?other.id:server.id)},'Console fixture','console_fixture',${quote(actor)},${quote(`${actor}@example.invalid`)},${i===2||i===3},12,'2020-01-01T00:00:00Z',now())`);
    }
    const page=await call('GET',base+'/executions?tool_name=console_fixture&limit=2&offset=1');
    check('execution pagination honors exact offset and deterministic timestamp ties',
      [page.json().executions.map(row=>row.id),page.json().total,page.json().limit,page.json().offset],[[ids[3],ids[2]],5,2,1]);
    check('execution history is private and no-store',page.headers.get('cache-control'),'private, no-store');
    const rows=(await call('GET',base+'/executions?tool_name=console_fixture')).json().executions;
    check('execution history never includes another server and is ordered newest-first on ties',rows.map(row=>row.id),ids.slice(0,5).reverse());
    check('execution rows include caller email, tool name, timestamp and duration',
      [rows[0].user_email,rows[0].tool_name,rows[0].executed_at,rows[0].duration_ms],[`${admin}@example.invalid`,'console_fixture','2020-01-01T00:00:00Z',12]);
    check('execution user and error filters are combined',
      (await call('GET',base+`/executions?tool_name=console_fixture&is_error=true&user_id=${member}`)).json().executions.map(row=>row.id),[ids[3]]);
    check('filtered execution totals do not include other statuses',
      (await call('GET',base+'/executions?tool_name=console_fixture&is_error=false')).json().total,3);
    check('an offset beyond the last row returns an empty page, preserving filtered total',
      (await call('GET',base+'/executions?tool_name=console_fixture&offset=1000000')).json(),{executions:[],total:5,limit:50,offset:1000000});
    for (const pagination of ['offset=1000001','offset=18446744073709551615']) await call('GET',base+`/executions?${pagination}`,undefined,400);
    check('execution pagination bounds offsets before integer conversion',true,true);
    for (const pagination of ['offset=-1','limit=-1','user_id=slack','is_error=maybe']) await call('GET',base+`/executions?${pagination}`,undefined,400);
    check('invalid execution filters are rejected before database queries',true,true);
    assert(!/access_token|refresh_token|client_secret/.test(JSON.stringify(rows)));
    check('console execution metadata contains no session credential fields',true,true);
  } finally {
    sql(`DELETE FROM mcp_executions WHERE id IN (${ids.map(quote).join(',')})`);
    await call('DELETE',`/admin/mcp-servers/${other.id}`,undefined,204);
  }
}
