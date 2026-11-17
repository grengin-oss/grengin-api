import assert from 'node:assert/strict';
import test from 'node:test';
import {jiraSite,releaseVersion} from './live-release.mjs';

const cargo = version => `[package]\nname = "grengin-api"\nversion = "${version}"\n\n[dependencies]\nserde = { version = "1" }\n`;

test('the expected release follows the checkout across a version bump', () => {
  assert.equal(releaseVersion(cargo('0.9.8')), '0.9.8');
  assert.equal(releaseVersion(cargo('0.9.10')), '0.9.10');
  assert.notEqual(releaseVersion(cargo('0.9.10')), '0.9.8');
});

test('dependency versions are never mistaken for the release', () => {
  assert.equal(releaseVersion(`[dependencies]\nserde = "1.0.0"\n\n[package]\nversion = "0.9.10"\n`), '0.9.10');
  assert.throws(() => releaseVersion('[dependencies]\nversion = "1.0.0"\n'));
  assert.throws(() => releaseVersion(cargo('next')));
});

const site = (id, scopes = ['read:jira-work']) => ({ id, scopes });

test('a rejected accessible-resources request is an incomplete Jira run', () => {
  assert.deepEqual(jiraSite(403, null), { blocked: 'accessible resources HTTP 403' });
  assert.equal(jiraSite(500, [site('a')]).site, undefined);
});

test('zero authorized Jira sites is an incomplete run', () => {
  assert.match(jiraSite(200, []).blocked, /0 accessible/);
  assert.match(jiraSite(200, [site('a', ['read:confluence-content.all'])]).blocked, /0 accessible/);
});

test('several Jira sites is an incomplete run until one is chosen', () => {
  assert.match(jiraSite(200, [site('a'), site('b')]).blocked, /2 accessible/);
});

test('exactly one authorized Jira site is selected, even when listed twice', () => {
  assert.deepEqual(jiraSite(200, [site('a'), site('a')]), { site: site('a') });
});
