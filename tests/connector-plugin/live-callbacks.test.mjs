import assert from 'node:assert/strict';
import test from 'node:test';
import {callbackFor,callbackPaths} from './live-callbacks.mjs';

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
