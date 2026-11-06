import assert from 'node:assert/strict';
import test from 'node:test';
import {callbackFor,callbackPaths,microsoftConnectorIds} from './live-callbacks.mjs';

for(const [id,uri]of Object.entries({
  gmail:'http://localhost:5173/v1/connectors/callback/gmail',
  'google-calendar':'http://localhost:5173/v1/connectors/callback/google-calendar',
  'google-drive':'http://localhost:5173/v1/connectors/callback//google-drive',
})){
  test(`${id} preserves its exact registered callback`,()=>{
    assert.equal(callbackFor(id),uri);
    assert.equal(new URL(callbackFor(id)).pathname,callbackPaths[id]);
  });
}

test('unknown callback identifiers are rejected',()=>{
  for(const id of ['../gmail','Google-Drive','unknown','__proto__'])assert.throws(()=>callbackFor(id));
});

for(const id of microsoftConnectorIds){
  test(`${id} uses its own registered Microsoft callback`,()=>{
    const uri=`http://localhost:5173/v1/connectors/callback/${id}`;
    assert.equal(callbackFor(id),uri);
    assert.equal(new URL(callbackFor(id)).pathname,callbackPaths[id]);
  });
}

test('Microsoft callbacks are distinct and do not change existing provider callbacks',()=>{
  assert.equal(new Set(microsoftConnectorIds.map(callbackFor)).size,6);
  assert.equal(callbackFor('slack'),'http://localhost:5173/mcp/oauth/callback');
  assert.equal(callbackFor('google-drive'),'http://localhost:5173/v1/connectors/callback//google-drive');
});
