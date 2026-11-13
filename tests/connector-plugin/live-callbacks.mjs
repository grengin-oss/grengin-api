export const microsoftConnectorIds=Object.freeze([
  'outlook-mail','outlook-calendar','onedrive','sharepoint','microsoft-teams','microsoft-work-iq',
]);

export const callbackPaths=Object.freeze({
  jira:'/v1/connectors/callback/jira',
  github:'/v1/connectors/callback/github',
  slack:'/mcp/oauth/callback',
  gmail:'/v1/connectors/callback/gmail',
  'google-calendar':'/v1/connectors/callback/google-calendar',
  'google-drive':'/v1/connectors/callback//google-drive',
  ...Object.fromEntries(microsoftConnectorIds.map(id=>[id,`/v1/connectors/callback/${id}`])),
});

export function callbackFor(id){
  if(!Object.hasOwn(callbackPaths,id))throw new Error('Unknown live-test connector');
  return 'http://localhost:5173'+callbackPaths[id];
}
