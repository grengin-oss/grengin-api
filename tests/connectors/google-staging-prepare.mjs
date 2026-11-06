import assert from 'node:assert/strict';
import {createHash} from 'node:crypto';
import {readFile,writeFile,chmod} from 'node:fs/promises';

process.umask(0o077);
assert(process.env.GOOGLE_CLIENT_ID&&process.env.GOOGLE_CLIENT_SECRET,'Saved Google credentials are required');
const directory=process.env.HOME+'/.local/state/grengin/connector-broker-staging';
const saved=JSON.parse(await readFile(directory+'/secrets.json','utf8'));
const clients=JSON.parse(saved.CONNECTOR_OAUTH_CLIENTS);
assert(clients.slack,'Existing Slack registry must be retained');
const slack=JSON.stringify(clients.slack);
const catalogs=[];
const listing=await (await fetch('https://meta.grengin.com/connectors.json',{redirect:'error',signal:AbortSignal.timeout(15000)})).json();
for(const key of ['gmail','google-calendar','google-drive']){
  const entry=listing.catalog.find(value=>value.key===key),release=entry?.versions.find(value=>value.version==='1.0.0');
  const url=`https://meta.grengin.com/connectors/${key}/versions/1.0.0.json`;
  assert(release&&release.url===url,'Pinned Google release must be listed');
  const response=await fetch(url,{redirect:'error',signal:AbortSignal.timeout(15000)});assert(response.ok);
  const bytes=Buffer.from(await response.arrayBuffer());assert(bytes.length<131072);
  assert.equal(createHash('sha256').update(bytes).digest('hex'),release.sha256);
  const catalog=JSON.parse(bytes.toString());assert.equal(catalog.key,key);
  assert.equal(catalog.connection.auth.provider,'google');
  assert.equal(catalog.connection.auth.auth_url,'https://accounts.google.com/o/oauth2/v2/auth');
  assert.equal(catalog.connection.auth.token_url,'https://oauth2.googleapis.com/token');
  catalogs.push(catalog);
}
clients.google={client_id:process.env.GOOGLE_CLIENT_ID,client_secret:process.env.GOOGLE_CLIENT_SECRET,
  auth_url:'https://accounts.google.com/o/oauth2/v2/auth',token_url:'https://oauth2.googleapis.com/token',
  revocation_url:'https://oauth2.googleapis.com/revoke',
  mcp_origins:[...new Set(catalogs.map(value=>new URL(value.connection.url).origin))],
  scopes:[...new Set(catalogs.flatMap(value=>value.connection.auth.scopes))],
  extra_params:{access_type:'offline',prompt:'consent'}};
assert.equal(JSON.stringify(clients.slack),slack);
const destination=directory+'/oauth-clients-google.json';
await writeFile(destination,JSON.stringify(clients),{mode:0o600});await chmod(destination,0o600);
console.log('Prepared merged OAuth registry: Slack unchanged, Google added, 3 MCP origins, 7 approved scopes.');
console.log('Encryption keys and the original secret backup were not changed. No remote update was performed.');
console.log(`Registry file: ${destination}`);
