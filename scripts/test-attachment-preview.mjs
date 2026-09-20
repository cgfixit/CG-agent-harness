// Real console functions in a bounded fixture: preview must reuse uploads and
// must never carry a previous account/session's files into a new session.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const html = readFileSync(new URL('../assets/static/harness.html', import.meta.url), 'utf8');
const source = html.slice(html.indexOf('async function previewChatPrompt('), html.indexOf('\nfunction isLoopStopCommand('));
const calls = [];
let uploads = 0, switchDuringUpload = false, failPreview = false;
const input = {files: ['a.md', 'b.md', 'c.md'], get value(){return '';}, set value(v){if(v==='')this.files=[];}};
const context = vm.createContext({
  sessionRevision: 1, currentSession: 'one', retryAttachmentIds: null, attachInput: input,
  sendBtn: {disabled:false}, sys(){}, awaitStyleWrites:async()=>{},
  releaseSendGate(){context.sendBtn.disabled=false;},
  uploadChatAttachments:async files=>{
    uploads++; assert.equal(files.length,3);
    if(switchDuringUpload){context.sessionRevision++;context.currentSession='two';context.retryAttachmentIds=null;}
    return ['id-a','id-b','id-c'];
  },
  api:async(path,method,body)=>{
    calls.push(JSON.parse(JSON.stringify({path,method,body})));
    if(failPreview)throw new Error('preview failed');
    return {prompt:'fixture'};
  }
});
vm.runInContext(source,context);
await context.previewChatPrompt();
assert.equal(uploads,1);
assert.deepEqual(calls[0].body.attachment_ids,['id-a','id-b','id-c']);
await context.previewChatPrompt({soul_content:'candidate'});
assert.equal(uploads,1,'repeated preview must not re-upload');
assert.equal(calls[1].body.soul_content,'candidate');
assert.deepEqual(calls[1].body.attachment_ids,['id-a','id-b','id-c']);
failPreview=true;
await assert.rejects(context.previewChatPrompt(),/preview failed/);
assert.equal(context.retryAttachmentIds.ids.length,3,'preview error must retain files for Send');
assert.equal(context.sendBtn.disabled,false);
failPreview=false;
context.currentSession='two';context.sessionRevision++;
await context.previewChatPrompt();
assert.deepEqual(calls.at(-1).body.attachment_ids,[],'another session cannot reuse ids');
context.sendBtn.disabled=true;
await assert.rejects(context.previewChatPrompt(),/wait for/);
context.sendBtn.disabled=false;input.files=['a.md','b.md','c.md'];switchDuringUpload=true;
const before=calls.length;
await assert.rejects(context.previewChatPrompt(),/session changed/);
assert.equal(calls.length,before,'stale upload must not reach preview');
assert.equal(context.retryAttachmentIds,null);
assert.equal(context.sendBtn.disabled,true,'new session owns send gate');
console.log('attachment preview: reuse, pending files, errors, busy and stale-session checks passed');
