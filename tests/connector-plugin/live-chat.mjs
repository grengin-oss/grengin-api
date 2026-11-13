import assert from 'node:assert/strict';
import {randomBytes} from 'node:crypto';
import {readFile} from 'node:fs/promises';
import {createServer} from 'node:http';
import {join} from 'node:path';

export async function runLiveChats({root,saved,request,pg,api,bearer,selected=new Set()}){
  const key='native-live-chat-'+randomBytes(6).toString('hex');
  const model='local-connector-fixture';
  let active,installed=false,fixtureFailures=0,passed=0,failed=0;
  const tests=[['jira','get_me'],['github','get_me'],['slack','get_team'],['gmail','list_labels'],['google-calendar','list_calendars'],['google-drive','list_files'],['outlook-mail','list_messages'],['outlook-calendar','list_calendars'],['onedrive','get_drive'],['sharepoint','get_root_site'],['microsoft-teams','list_joined_teams'],['microsoft-work-iq','get_me']].filter(([id])=>Object.hasOwn(saved.servers,id)&&(!selected.size||selected.has(id)));
  assert(tests.length,'No selected chat tool tests');
  const fixture=createServer(async(req,res)=>{
    try{
      assert.equal(req.method,'POST');
      assert.equal(req.headers.authorization,'Bearer fixture-no-paid-calls');
      assert(new URL(req.url,'http://localhost').pathname.endsWith('/chat/completions'));
      let size=0;const chunks=[];
      for await(const chunk of req){size+=chunk.length;assert(size<=2*1024*1024);chunks.push(chunk);}
      const body=JSON.parse(Buffer.concat(chunks).toString());
      if(body.stream!==true){
        res.writeHead(200,{'Content-Type':'application/json'});
        res.end(JSON.stringify({choices:[{message:{role:'assistant',content:'Native connector live smoke test'},finish_reason:'stop'}]}));return;
      }
      assert(active,'No live test is active');
      const toolResult=body.messages.find(message=>message.role==='tool');
      const mcp=(body.tools??[]).filter(item=>item.function?.name.startsWith('mcp__'));
      if(!toolResult&&!mcp.length){
        res.writeHead(200,{'Content-Type':'text/event-stream','Cache-Control':'no-store'});
        res.write(`data: ${JSON.stringify({choices:[{delta:{content:'Native connector live smoke test'}}]})}\n\n`);
        res.write(`data: ${JSON.stringify({choices:[{delta:{},finish_reason:'stop'}]})}\n\n`);
        res.end('data: [DONE]\n\n');return;
      }
      active.requests++;
      let events;
      if(toolResult){
        assert.equal(toolResult.tool_call_id,active.callId);assert(toolResult.content);
        active.receivedResult=true;
        events=[{choices:[{delta:{content:'Local fixture received the real connector tool result.'}}]},{choices:[{delta:{},finish_reason:'stop'}],usage:{prompt_tokens:1,completion_tokens:1,total_tokens:2}}];
      }else{
        const prefix=`mcp__${active.server.replaceAll('-','').slice(0,8)}__`;
        assert(mcp.length&&mcp.every(item=>item.function.name.startsWith(prefix)),'Chat exposed another selected server');
        const tool=mcp.find(item=>item.function.name.includes(`__${active.tool}__`));
        assert(tool,'Expected native read tool was not provided to model');
        active.functionName=tool.function.name;
        events=[{choices:[{delta:{tool_calls:[{index:0,id:active.callId,type:'function',function:{name:tool.function.name,arguments:JSON.stringify(active.args)}}]}}]},{choices:[{delta:{},finish_reason:'tool_calls'}]}];
      }
      res.writeHead(200,{'Content-Type':'text/event-stream','Cache-Control':'no-store'});
      for(const event of events)res.write(`data: ${JSON.stringify(event)}\n\n`);
      res.end('data: [DONE]\n\n');
    }catch{
      fixtureFailures++;
      if(!res.headersSent)res.writeHead(400,{'Content-Type':'application/json'});
      res.end(JSON.stringify({error:{message:'Local fixture request rejected'}}));
    }
  });
  await new Promise(resolve=>fixture.listen(0,'127.0.0.1',resolve));
  const origin=`http://127.0.0.1:${fixture.address().port}`;
  const connections=(await request('GET','/mcp/connections')).connections;
  const servers=(await request('GET','/admin/mcp-servers')).servers;
  try{
    const provider=JSON.parse(await readFile(join(root,'llm-plugin/examples/openai-compatible.provider.json'),'utf8'));
    provider.id=key;provider.name='Local live connector test fixture';delete provider.operations.listModels;
    provider.models=[{id:model,name:'Local connector fixture (no paid LLM)',modelType:'text_generator',capabilities:{chat:provider.capabilities.chat},metadata:{inputTokenRate:0,outputTokenRate:0}}];
    await request('POST','/admin/ai-engines',{api_key:'fixture-no-paid-calls',is_enabled:true,whitelisted_models:[model],default_model:model,plugin_config:{manifest:provider,configuration:{},baseUrlOverride:origin+'/',allowInsecureHttp:true,allowPrivateNetwork:true}});
    installed=true;
    for(const [connector,tool]of tests){
      const server=saved.servers[connector];
      assert(connections.some(row=>row.server_id===server&&row.connected),`${connector}: user connection is missing`);
      const definition=servers.find(row=>row.id===server).connection_config.connector_plugin.manifest.tools.find(item=>item.name===tool);
      assert(definition.read_only,`${connector}: refusing a write tool`);
      const args={};for(const name of ['limit','per_page','page_size','max_results'])if(definition.input_schema.properties?.[name])args[name]=1;
      active={server,tool,args,callId:'native-'+randomBytes(8).toString('hex'),requests:0,receivedResult:false};
      try{
        const response=await fetch(api+'/chat/stream',{method:'POST',headers:{Authorization:`Bearer ${bearer()}`,'Content-Type':'application/json'},body:JSON.stringify({provider:key,model_name:model,selected_mcp_servers:[server],messages:[{role:'user',content:`Live read-only connector smoke test: call ${tool} once.`,files:[]}]}),signal:AbortSignal.timeout(60000)});
        assert.equal(response.status,200,`${connector}: chat HTTP status`);
        let size=0;const chunks=[];
        for await(const chunk of response.body){size+=chunk.length;assert(size<=2*1024*1024,'Chat stream exceeded test bound');chunks.push(chunk);}
        const events=Buffer.concat(chunks).toString().split(/\r?\n\r?\n/).filter(Boolean).map(block=>{
          let name=block.split('\n').find(line=>line.startsWith('event:'))?.slice(6).trim()??'';
          try{name=JSON.parse(name);}catch{}
          const data=block.split('\n').filter(line=>line.startsWith('data:')).map(line=>line.slice(5).trimStart()).join('\n');
          return {name,data:JSON.parse(data||'{}')};
        });
        assert(!events.some(event=>event.name==='ai_error'||event.name==='mcp_oauth_required'),`${connector}: chat reported an error or missing OAuth`);
        const call=events.find(event=>event.name==='tool_call'&&event.data.tool_call?.tool_id===active.callId);
        const result=events.find(event=>event.name==='tool_result'&&event.data.tool_result?.tool_id===active.callId);
        assert(call&&result,`${connector}: missing tool_call/tool_result SSE`);
        assert.equal(result.data.tool_result.status,'success',`${connector}: tool result failed`);
        assert.equal(result.data.tool_result.tool_name,active.functionName);
        assert(active.receivedResult,`${connector}: provider did not receive the tool result`);
        assert.equal(active.requests,2,`${connector}: unexpected model round count`);
        assert(events.some(event=>event.name==='done'),`${connector}: missing done event`);
        const chat=events.find(event=>event.name==='conversation')?.data.id;
        assert(/^[a-f0-9-]{36}$/.test(chat),'Missing conversation UUID');
        const quote=value=>`'${String(value).replaceAll("'","''")}'`;
        const rows=JSON.parse(pg(`SELECT coalesce(json_agg(json_build_object('server',"serverId",'tool',"toolName",'user',"userId",'email',"userEmail",'error',"isError",'duration',"durationMs",'timestamp',"executedAt")),'[]'::json) FROM mcp_executions WHERE "conversationId"=${quote(chat)}`,saved.database));
        assert.equal(rows.length,1,`${connector}: expected one execution audit row`);
        assert.equal(rows[0].server,server);assert.equal(rows[0].tool,tool);assert.equal(rows[0].user,saved.owner.id);assert.equal(rows[0].email,saved.owner.email);assert.equal(rows[0].error,false);assert(rows[0].timestamp&&rows[0].duration>=0);
        passed++;
        console.log(`PASS ${connector}/${tool}: real /chat/stream tool call and result, model resumed, execution audit recorded`);
      }catch(error){
        failed++;
        console.log(`CHECK ${connector}/${tool}: ${error instanceof assert.AssertionError?error.message:'chat test failed; response contents withheld'}`);
      }
    }
    assert.equal(fixtureFailures,0,'Local model fixture rejected a request');
    console.log(`Chat tool calling: ${passed} passed; ${failed} failed. Vendor APIs and OAuth tokens are real; only the LLM is a local fixture.`);
    if(failed)process.exitCode=1;
  }finally{
    try{if(installed)await request('DELETE',`/admin/ai-engines/${key}`);}
    finally{fixture.closeAllConnections();await new Promise(resolve=>fixture.close(resolve));}
  }
}
