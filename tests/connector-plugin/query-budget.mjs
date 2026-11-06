import assert from 'node:assert/strict';
import {randomUUID} from 'node:crypto';
import {spawnSync} from 'node:child_process';

function selectCount() {
  const container=process.env.GRENGIN_PLUGIN_TEST_DB_CONTAINER;
  assert(/^grengin-plugin-test-\d+$/.test(container),'Query counting requires the disposable test container');
  const result=spawnSync('docker',['logs',container],{encoding:'utf8',maxBuffer:16*1024*1024});
  assert.equal(result.status,0,'Cannot read disposable PostgreSQL query log');
  const text=result.stdout+result.stderr;
  return (text.match(/^contract-sql:connector-contract: LOG:\s+(?:execute [^:]+:|statement:)\s+SELECT\b/gm)??[]).length;
}

export async function runQueryBudget({call,check,sql,quote,server,member}) {
  const parent=randomUUID(),child=randomUUID();
  const tool=sql(`SELECT id FROM mcp_tools WHERE "serverId"=${quote(server.id)}`);
  const clones=[];
  const policyIds=[];
  const addPolicy=(target,id)=>{
    const policy=randomUUID();policyIds.push(policy);
    sql(`INSERT INTO mcp_access_policies(id,"targetType","serverId","toolId","accessType",permission,"departmentId","inheritDepartments","createdAt")
      VALUES (${quote(policy)},${quote(target)},${target==='server'?quote(id):'NULL'},${target==='tool'?quote(id):'NULL'},'department','read_only',${quote(parent)},true,now())`);
  };
  const measure=async path=>{
    // Warm connection/auth caches before comparing SQL round trips, not response time.
    await call('GET',path,undefined,200,member);
    const before=selectCount();
    const response=await call('GET',path,undefined,200,member);
    const count=selectCount()-before;
    assert(count>0,'PostgreSQL query logging did not capture SELECT statements');
    return {count,body:response.json()};
  };
  try {
    for(const [id,path,depth,parentId] of [[parent,'budgetroot',0,null],[child,'budgetroot.child',1,parent]]) {
      sql(`INSERT INTO departments(id,name,description,"parentId",depth,path,"budgetAllocated","budgetAvailable","budgetPeriod","actionOnExceed","createdAt","updatedAt")
        VALUES (${quote(id)},${quote(path)},'',${parentId?quote(parentId):'NULL'},${depth},${quote(path)}::ltree,0,0,'monthly','warn',now(),now())`);
    }
    sql(`UPDATE users SET "departmentId"=${quote(child)} WHERE id=${quote(member)};
      UPDATE mcp_servers SET "defaultAccess"='explicit_only' WHERE id=${quote(server.id)};
      UPDATE mcp_tools SET "inheritAccessFromServer"=false WHERE id=${quote(tool)}`);
    addPolicy('server',server.id);addPolicy('tool',tool);
    const oneServer=await measure('/mcp-servers');
    const oneTool=await measure('/mcp/tools');
    for(let index=0;index<12;index++) {
      const id=randomUUID(),toolId=randomUUID();clones.push(id);
      sql(`INSERT INTO mcp_servers SELECT (jsonb_populate_record(NULL::mcp_servers,to_jsonb(s)||jsonb_build_object('id',${quote(id)},'name',${quote(`Budget fixture ${index}`)}))).* FROM mcp_servers s WHERE id=${quote(server.id)};
        INSERT INTO mcp_tools SELECT (jsonb_populate_record(NULL::mcp_tools,to_jsonb(t)||jsonb_build_object('id',${quote(toolId)},'serverId',${quote(id)}))).* FROM mcp_tools t WHERE id=${quote(tool)}`);
      addPolicy('server',id);addPolicy('tool',toolId);
    }
    const manyServers=await measure('/mcp-servers');
    const manyTools=await measure('/mcp/tools');
    check('inherited department grants apply across all listed servers',manyServers.body.servers.length,13);
    check('inherited per-tool department grants apply across all listed tools',manyTools.body.total,13);
    check('picker SELECT budget is constant for 1 versus 13 department-scoped servers',manyServers.count,oneServer.count);
    check('tool-list SELECT budget is constant for 1 versus 13 department-scoped servers',manyTools.count,oneTool.count);
    console.log(`Query budgets: picker=${manyServers.count}, tools=${manyTools.count}; unchanged as connectors grew from 1 to 13.`);
  } finally {
    sql(`UPDATE users SET "departmentId"=NULL WHERE id=${quote(member)};
      UPDATE mcp_servers SET "defaultAccess"='all_users' WHERE id=${quote(server.id)};
      UPDATE mcp_tools SET "inheritAccessFromServer"=true WHERE id=${quote(tool)}`);
    if(policyIds.length)sql(`DELETE FROM mcp_access_policies WHERE id IN (${policyIds.map(quote).join(',')})`);
    if(clones.length)sql(`DELETE FROM mcp_tools WHERE "serverId" IN (${clones.map(quote).join(',')}); DELETE FROM mcp_servers WHERE id IN (${clones.map(quote).join(',')})`);
    sql(`DELETE FROM departments WHERE id=${quote(child)}; DELETE FROM departments WHERE id=${quote(parent)}`);
  }
}
