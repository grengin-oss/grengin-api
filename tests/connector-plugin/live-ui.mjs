import assert from 'node:assert/strict';
import {pathToFileURL} from 'node:url';

export async function runLiveUi({saved,bearer,request,selected}){
  const {chromium}=await import(pathToFileURL(process.env.GRENGIN_PLAYWRIGHT_MODULE??`${process.env.HOME}/.cache/codex-runtimes/codex-primary-runtime/dependencies/node/node_modules/playwright/index.mjs`));
  const servers=(await request('GET','/admin/mcp-servers')).servers;
  const browser=await chromium.launch({headless:true});
  try{
    const context=await browser.newContext({viewport:{width:1366,height:1000}});
    await context.addInitScript(({user,token})=>{
      localStorage.setItem('grengin_access_token',token);
      localStorage.setItem('grengin_user',JSON.stringify(user));
    },{user:saved.owner,token:bearer()});
    const page=await context.newPage();
    let errors=0;page.on('pageerror',()=>errors++);
    await page.goto('http://localhost:5173/settings?tab=integrations');
    for(const [key,id]of Object.entries(saved.servers)){
      if(selected.size&&!selected.has(key))continue;
      const server=servers.find(row=>row.id===id);assert(server?.enabled,`${key}: server must be enabled`);
      const card=page.locator('.int-card').filter({has:page.getByTitle(server.name,{exact:true})});
      await card.waitFor({state:'visible',timeout:15000});
      const button=card.getByRole('button',{name:/^(?:Connect|Reconnect)$/i});
      assert.equal(await button.count(),1,`${key}: missing real Connect/Reconnect button`);
      assert(await button.isEnabled(),`${key}: real Connect/Reconnect button is disabled`);
      console.log(`PASS ${key}: real authenticated integrations page shows an enabled connection button`);
    }
    assert.equal(errors,0,'Integrations page reported a browser runtime error');
    console.log('Real UI check passed without mocking API responses, clicking consent or changing connections.');
  }finally{await browser.close();}
}
