#!/usr/bin/env bash
set -euo pipefail
set +x
umask 077
cd "$(dirname "$0")/../.."
set -a
source /home/anurag/work/secrets/grengin.sh
source .grengin_auth_token
set +a
node <<'JS'
const fs=require('node:fs');const {execFileSync}=require('node:child_process');
const state=process.env.HOME+'/.local/state/grengin/slack-consent';
if(fs.existsSync(state+'/real-db-ready')){console.log('Existing real-account local database copy retained.');process.exit(0);}
const config=JSON.parse(fs.readFileSync(state+'/session.json','utf8'));
const source=new URL(process.env.DATABASE_URL);
if(!['localhost','127.0.0.1','::1'].includes(source.hostname))throw new Error('Refusing to copy a remote database');
const env={...process.env,PGHOST:source.hostname,PGPORT:source.port||'5432',PGUSER:decodeURIComponent(source.username),
 PGPASSWORD:decodeURIComponent(source.password),PGDATABASE:source.pathname.slice(1)};
const dump=state+'/development-backup.dump';const fd=fs.openSync(dump,'w',0o600);
try{execFileSync('pg_dump',['--format=custom','--no-owner','--no-privileges'],{env,stdio:['ignore',fd,'pipe']});}finally{fs.closeSync(fd);}
execFileSync('docker',['exec','grengin-slack-consent','dropdb','--if-exists','-U','local','grengin_slack_real'],{stdio:['ignore','pipe','pipe']});
execFileSync('docker',['exec','grengin-slack-consent','createdb','-U','local','grengin_slack_real'],{stdio:['ignore','pipe','pipe']});
const input=fs.openSync(dump,'r');
try{execFileSync('docker',['exec','-i','grengin-slack-consent','pg_restore','--no-owner','--no-privileges','--exit-on-error','-U','local','-d','grengin_slack_real'],{stdio:[input,'pipe','pipe']});}
finally{fs.closeSync(input);}
fs.writeFileSync(state+'/real-db-ready','Read-only snapshot of local development database.\n',{mode:0o600});
console.log('Copied real local accounts and settings; original development database unchanged.');
JS
