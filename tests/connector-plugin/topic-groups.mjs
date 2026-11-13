import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';

export async function runTopicAudit({call,check,sql,quote,request,admin,member}) {
  const install=structuredClone(request);
  install.name='Topic audit fixture';
  const manifest=install.connection_config.connector_plugin.manifest;
  manifest.id='topic-fixture';
  for(const name of ['second_read','other_read']) {
    const tool=structuredClone(manifest.tools[0]);tool.name=name;manifest.tools.push(tool);
  }
  manifest.presentation={topic_groups:[
    {id:'records',name:'Records',tools:['plugin_echo','second_read']},
    {id:'other',name:'Other records',tools:['other_read']},
  ]};
  const server=(await call('POST','/admin/mcp-servers',install,201)).json();
  const base=`/admin/mcp-servers/${server.id}`;
  const update=base+'/topic-groups/records';
  const flags=()=>JSON.parse(sql(`SELECT json_object_agg("originalName",enabled) FROM mcp_tools WHERE "serverId"=${quote(server.id)}`));
  let trigger=false;
  try {
    await call('PUT',update,{enabled:false},401,null);
    await call('PUT',update,{enabled:false},403,member);
    await call('PUT',`/admin/mcp-servers/${randomUUID()}/topic-groups/records`,{enabled:false},403,member);
    await call('PUT',base+'/topic-groups/missing',{enabled:false},404);
    await call('PUT',update,{enabled:false,tool_ids:[randomUUID()]},422);
    check('topic audit: unauthorized, unknown and caller-supplied membership writes are refused',flags(),{plugin_echo:true,second_read:true,other_read:true});
    const disabled=await call('PUT',update,{enabled:false});
    check('topic audit: group disable changes all members and no unrelated tools',flags(),{plugin_echo:false,second_read:false,other_read:true});
    check('topic audit: response names every changed tool',[disabled.json().group.state,disabled.json().tools.length,disabled.json().group.tool_ids.length],['disabled',2,2]);
    check('topic audit: group mutation stays private and no-store',disabled.headers.get('cache-control'),'private, no-store');
    check('topic audit: one security audit event covers the whole group',Number(sql(`SELECT count(*) FROM auth_audit_events WHERE event='mcp.tool_group_enabled_changed' AND payload->>'server_id'=${quote(server.id)}`)),1);
    const tool=disabled.json().group.tool_ids[0];
    await call('PUT',base+`/tools/${tool}`,{enabled:true});
    check('topic audit: individual switch yields a mixed group',(await call('GET',base+'/overview')).json().topic_groups.find(g=>g.id==='records').state,'mixed');
    const before=flags();
    sql(`CREATE FUNCTION audit_topic_failure() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF NEW.event='mcp.tool_group_enabled_changed' THEN RAISE EXCEPTION 'audit fixture failure'; END IF; RETURN NEW; END $$; CREATE TRIGGER audit_topic_failure BEFORE INSERT ON auth_audit_events FOR EACH ROW EXECUTE FUNCTION audit_topic_failure()`);trigger=true;
    await call('PUT',update,{enabled:false},503);
    check('topic audit: audit failure rolls back every member',flags(),before);
    sql('DROP TRIGGER audit_topic_failure ON auth_audit_events; DROP FUNCTION audit_topic_failure()');trigger=false;
    await call('PUT',update,{enabled:true});
    check('topic audit: group re-enable is complete',flags(),{plugin_echo:true,second_read:true,other_read:true});
    await call('PUT',base,{default_access:'admin_only'});
    check('topic audit: enabling a group does not grant a member server access',(await call('GET',`/mcp/tools?server_id=${server.id}`,undefined,200,member)).json().total,0);
    assert.equal(sql(`SELECT bool_and("isReadOnly" AND "inheritAccessFromServer") FROM mcp_tools WHERE "serverId"=${quote(server.id)}`),'t');
    check('topic audit: group changes preserve read-only and access-policy flags',true,true);
  } finally {
    if(trigger)sql('DROP TRIGGER audit_topic_failure ON auth_audit_events; DROP FUNCTION audit_topic_failure()');
    await call('DELETE',base,undefined,204);
  }
}
